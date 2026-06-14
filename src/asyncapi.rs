// asyncapi.rs — Phase 9.w.2.d
//
// AsyncAPI 3.0 generator for `@ws("/path")` handlers. Parallel to
// `openapi.rs` but for the WebSockets contract:
//
//   - `OpenAPI 3.1` documents HTTP request/response endpoints — it
//     does not capture the shape of bidirectional WS messages.
//   - `AsyncAPI 3.0` is the industry standard for event-driven and
//     streaming APIs: WebSockets, MQTT, Kafka, etc.
//
// Fitz emits both (without the user having to do anything):
// `/openapi.json` for `@get`/`@post`/etc., `/asyncapi.json` for
// `@ws`. Each `@ws` handler contributes a channel + two operations
// (receive and send), with the `T` type of `WsConn<T>` serialized
// to JSON Schema. Auth (`@authenticated`/`@admin`) is documented
// with the bearer security scheme (parallel to OpenAPI 9.w.1.e).
//
// Design decisions:
//
//   - **Unconditional emit when there are @ws handlers**: if the
//     program has at least one `@ws`, `/asyncapi.json` is served.
//     Programs without WS don't expose the route — parallel to
//     `/openapi.json` which is only emitted when there are HTTP
//     handlers.
//
//   - **Channel per `@ws("/path")`**: each declared path is a
//     channel. The channel name is the path itself (`/chat`).
//
//   - **One message per channel**: the `T` type of `WsConn<T>`
//     defines the shape (same schema in both directions — receive
//     and send). If different typed directions are supported in the
//     future (`WsConn<In, Out>`), they are split here.
//
//   - **receive + send operations**: two operations per channel,
//     one in each direction. `receive` = client → server
//     (`conn.recv()`); `send` = server → client (`conn.send()` /
//     `conn.broadcast()`). Same message ref for both.
//
//   - **In-line embedded schema per nominal T** (not `$ref`):
//     `components/schemas` is OpenAPI-specific. AsyncAPI 3.0
//     supports them too, but for initial simplicity we emit the
//     schema directly under `messages.<name>.payload`. A future
//     sub-step may unify both via cross-spec `$ref`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::ast::{Decorator, Expr, Program, Stmt, TypeExpr};
use crate::http::{AuthSpec, HttpRegistry, RouteSpec};
use crate::openapi::{headers_from_decorators, type_expr_to_schema};

/// 9.w.2-asyncapi-ui — Scalar-style embedded UI for the AsyncAPI 3.0
/// schema. Loads the `@asyncapi/react-component` bundle via CDN and
/// points it at `/asyncapi.json`. Same model as openapi's
/// `SCALAR_HTML` (lightweight load, first time requires network; the
/// browser caches the bundle afterwards).
pub const ASYNCAPI_HTML: &str = include_str!("templates/asyncapi.html");

/// Lightweight view of a `@ws("/path")` endpoint for the AsyncAPI
/// generator. Parallel to `OpenApiRouteInfo` but only for WS fields.
#[derive(Debug, Clone)]
pub struct AsyncApiChannelInfo {
    /// Endpoint path (e.g. `"/chat"`).
    pub path: String,
    /// Name of the Fitz handler — serves as the base for operation
    /// IDs (`receive<Handler>` / `send<Handler>`).
    pub handler_name: String,
    /// `TypeExpr` of the `T` in `WsConn<T>` (or `In` in
    /// `WsConn<In, Out>`). Schema used for the `receive` operation
    /// (client → server). `None` only in malformed builds where the
    /// evaluator could not identify T.
    pub msg_type: Option<TypeExpr>,
    /// 9.w.2-wsconn-bidir (v0.9.38): `TypeExpr` of the `Out` in
    /// `WsConn<In, Out>`. Schema used for the `send` operation
    /// (server → client). For symmetric `WsConn<T>`, it's the same
    /// as `msg_type`.
    pub send_type: Option<TypeExpr>,
    /// Route's auth policy. `Authenticated`/`Admin` → security
    /// requirement with bearer on the channel.
    pub auth: AuthSpec,
}

/// Adapter from the runtime `HttpRegistry` to the schema. Filters
/// only the WS routes (`is_ws == true`); HTTP ones are ignored
/// (they go through OpenAPI).
pub fn channels_from_registry(reg: &HttpRegistry) -> Vec<AsyncApiChannelInfo> {
    reg.routes
        .iter()
        .filter(|r| r.is_ws)
        .map(channel_info_from_spec)
        .collect()
}

fn channel_info_from_spec(s: &RouteSpec) -> AsyncApiChannelInfo {
    AsyncApiChannelInfo {
        path: s.path.clone(),
        handler_name: s.handler_name.clone(),
        msg_type: s.ws_msg_type.clone(),
        send_type: s.ws_send_type.clone(),
        auth: s.auth,
    }
}

/// Adapter from the AST (build-time, `fitz build`) for building the
/// views without needing to evaluate the program. Mirror of
/// `openapi::pseudo_routes_from_ast` but filters `@ws` only.
pub fn pseudo_channels_from_ast(
    program: &Program,
) -> Result<Vec<AsyncApiChannelInfo>, crate::error::FitzError> {
    pseudo_channels_from_program_and_modules(program, &[])
}

/// 10.8.6 (v0.10.8) — cross-module aware variant of
/// `pseudo_channels_from_ast`. Combines the `@ws` handlers from the
/// `program` (main) and from the `module_ws_stmts` (slices captured
/// by 10.8.6 in `LoadedModule.ws_fn_stmts`, parallel to W16).
/// Result: the emitted AsyncAPI 3.0 schema contains ALL WS channels,
/// including those from imported modules.
///
/// Before the v0.10.8 fix #4, `pseudo_channels_from_ast` only looked
/// at main → empty schema and `/asyncapi.json` was not emitted when
/// the `@ws` lived cross-module (404 on handshake).
pub fn pseudo_channels_from_program_and_modules(
    program: &Program,
    module_ws_stmts: &[&[Stmt]],
) -> Result<Vec<AsyncApiChannelInfo>, crate::error::FitzError> {
    // Concatenate all stmts: main first, then the modules in load
    // order.
    let mut all_stmts: Vec<&Stmt> = program.iter().collect();
    for module_stmts in module_ws_stmts {
        for s in *module_stmts {
            all_stmts.push(s);
        }
    }
    let mut out = Vec::new();
    for s in all_stmts {
        let Stmt::FnDef {
            name,
            params,
            decorators,
            ..
        } = s
        else {
            continue;
        };
        for d in decorators {
            if d.name != "ws" {
                continue;
            }
            let path_arg = match d.args.first() {
                Some(e) => e,
                None => continue,
            };
            let path = match path_arg {
                Expr::Str(s, _) => s.clone(),
                _ => continue, // checker already rejected this
            };
            // Detect the WsConn<T> or WsConn<In, Out> param to
            // extract recv (msg_type) and send (send_type).
            let (msg_type, send_type) = params
                .iter()
                .find_map(|p| match &p.type_ {
                    Some(TypeExpr::Generic { name: n, args })
                        if n == "WsConn" && (args.len() == 1 || args.len() == 2) =>
                    {
                        let recv = args[0].clone();
                        let send = if args.len() == 2 {
                            args[1].clone()
                        } else {
                            recv.clone()
                        };
                        Some((Some(recv), Some(send)))
                    }
                    _ => None,
                })
                .unwrap_or((None, None));
            // Auth from decorators.
            let mut auth = AuthSpec::None;
            for dd in decorators {
                match dd.name.as_str() {
                    "authenticated" if auth == AuthSpec::None => {
                        auth = AuthSpec::Authenticated;
                    }
                    "admin" => auth = AuthSpec::Admin,
                    _ => {}
                }
            }
            // `header_params` and others don't apply to `@ws`
            // (`@header` is not declared on WS handlers); we keep
            // openapi's helper available for future consistency.
            let _ = headers_from_decorators(decorators, params);
            out.push(AsyncApiChannelInfo {
                path,
                handler_name: name.clone(),
                msg_type,
                send_type,
                auth,
            });
        }
    }
    Ok(out)
}

/// Generates the full AsyncAPI 3.0 schema. Output ready for
/// `serde_json::to_string` → `/asyncapi.json`.
///
/// Emitted structure:
///
/// ```json
/// {
///   "asyncapi": "3.0.0",
///   "info": { "title": "Fitz API", "version": "0.1.0" },
///   "servers": { "fitz": { "host": "{host}:{port}", "protocol": "ws" } },
///   "channels": {
///     "<path>": {
///       "address": "<path>",
///       "messages": {
///         "msg": { "name": "<TypeName>", "payload": <schema-T> }
///       }
///     }
///   },
///   "operations": {
///     "receive<Handler>": { "action": "receive", "channel": ..., "messages": [...] },
///     "send<Handler>": { "action": "send", "channel": ..., "messages": [...] }
///   },
///   "components": {
///     "securitySchemes": { "bearerAuth": { "type": "http", "scheme": "bearer" } }
///   }
/// }
/// ```
pub fn generate_asyncapi(channels: &[AsyncApiChannelInfo], program: &Program) -> Value {
    generate_asyncapi_with_version(channels, program, None)
}

/// Variant that accepts an `info.version` override. The runtime and
/// codegen read `@server(api_version=...)` the same way they do for
/// OpenAPI.
pub fn generate_asyncapi_with_version(
    channels: &[AsyncApiChannelInfo],
    program: &Program,
    version: Option<&str>,
) -> Value {
    let _ = program;
    let mut channels_obj: Map<String, Value> = Map::new();
    let mut operations_obj: Map<String, Value> = Map::new();

    // Ordered map to emit channels in deterministic order
    // (ascending path — parallel to declaration order for UX reuse).
    let mut sorted: BTreeMap<&str, &AsyncApiChannelInfo> = BTreeMap::new();
    for c in channels {
        sorted.insert(c.path.as_str(), c);
    }

    for (path, ch) in &sorted {
        // 9.w.2-wsconn-bidir: when recv != send, we emit TWO
        // messages (`msg_in` for receive, `msg_out` for send) with
        // their respective schemas. When they are equal (symmetric
        // or single-type channel), a single message `msg`.
        let symmetric = match (&ch.msg_type, &ch.send_type) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };

        let recv_schema = ch
            .msg_type
            .as_ref()
            .map(type_expr_to_schema)
            .unwrap_or_else(|| json!({}));
        let recv_name = ch
            .msg_type
            .as_ref()
            .map(|t| t.display_name())
            .unwrap_or_else(|| "AnyMessage".to_string());
        let recv_is_bytes = matches!(
            ch.msg_type.as_ref(),
            Some(TypeExpr::Named(n)) if n == "Bytes"
        );
        let recv_content_type = if recv_is_bytes {
            "application/octet-stream"
        } else {
            "application/json"
        };
        let recv_summary = if recv_is_bytes {
            "Raw binary frame ↔ Fitz `Bytes`".to_string()
        } else {
            format!("Frame text JSON ↔ Fitz `{}`", recv_name)
        };

        let mut messages = Map::new();
        if symmetric {
            messages.insert(
                "msg".into(),
                json!({
                    "name": recv_name,
                    "title": format!("Message of `{}`", ch.handler_name),
                    "summary": recv_summary,
                    "contentType": recv_content_type,
                    "payload": recv_schema,
                }),
            );
        } else {
            // recv != send — two distinct messages.
            let send_schema = ch
                .send_type
                .as_ref()
                .map(type_expr_to_schema)
                .unwrap_or_else(|| json!({}));
            let send_name = ch
                .send_type
                .as_ref()
                .map(|t| t.display_name())
                .unwrap_or_else(|| "AnyMessage".to_string());
            let send_is_bytes = matches!(
                ch.send_type.as_ref(),
                Some(TypeExpr::Named(n)) if n == "Bytes"
            );
            let send_content_type = if send_is_bytes {
                "application/octet-stream"
            } else {
                "application/json"
            };
            let send_summary = if send_is_bytes {
                "Raw binary frame ↔ Fitz `Bytes`".to_string()
            } else {
                format!("Frame text JSON ↔ Fitz `{}`", send_name)
            };
            messages.insert(
                "msg_in".into(),
                json!({
                    "name": recv_name,
                    "title": format!("Message IN (client → server) of `{}`", ch.handler_name),
                    "summary": recv_summary,
                    "contentType": recv_content_type,
                    "payload": recv_schema,
                }),
            );
            messages.insert(
                "msg_out".into(),
                json!({
                    "name": send_name,
                    "title": format!("Message OUT (server → client) of `{}`", ch.handler_name),
                    "summary": send_summary,
                    "contentType": send_content_type,
                    "payload": send_schema,
                }),
            );
        }
        channels_obj.insert(
            (*path).to_string(),
            json!({
                "address": path,
                "title": format!("WebSocket channel `{}`", ch.handler_name),
                "description": match ch.auth {
                    AuthSpec::None => "No auth — open to any client.".to_string(),
                    AuthSpec::Authenticated => "Requires bearer token (validated pre-upgrade by the `@auth_provider`).".to_string(),
                    AuthSpec::Admin => "Requires bearer token + admin role (validated pre-upgrade).".to_string(),
                },
                "messages": messages,
            }),
        );

        // Operations: receive (client→server) + send (server→client).
        // In AsyncAPI 3.0 $ref uses JSON Pointer; `/` is escaped as
        // `~1` and `~` as `~0`. The Fitz path typically has no `~`,
        // but the leading `/` does need escaping.
        let path_ref = path.replace('~', "~0").replace('/', "~1");
        let channel_ref = format!("#/channels/{}", path_ref);
        let recv_msg_ref = if symmetric {
            format!("#/channels/{}/messages/msg", path_ref)
        } else {
            format!("#/channels/{}/messages/msg_in", path_ref)
        };
        let send_msg_ref = if symmetric {
            format!("#/channels/{}/messages/msg", path_ref)
        } else {
            format!("#/channels/{}/messages/msg_out", path_ref)
        };

        let mut receive_op = Map::new();
        receive_op.insert("action".into(), json!("receive"));
        receive_op.insert("channel".into(), json!({"$ref": channel_ref}));
        receive_op.insert("messages".into(), json!([{"$ref": recv_msg_ref}]));
        receive_op.insert(
            "summary".into(),
            json!(format!(
                "The handler `{}` receives messages from the client via `conn.recv()`.",
                ch.handler_name
            )),
        );
        if ch.auth != AuthSpec::None {
            receive_op.insert("security".into(), json!([{"bearerAuth": []}]));
        }
        operations_obj.insert(
            format!("receive{}", capitalize(&ch.handler_name)),
            Value::Object(receive_op),
        );

        let mut send_op = Map::new();
        send_op.insert("action".into(), json!("send"));
        send_op.insert("channel".into(), json!({"$ref": channel_ref}));
        send_op.insert("messages".into(), json!([{"$ref": send_msg_ref}]));
        send_op.insert(
            "summary".into(),
            json!(format!(
                "The handler `{}` sends messages to the client via `conn.send()` or `conn.broadcast()`.",
                ch.handler_name
            )),
        );
        if ch.auth != AuthSpec::None {
            send_op.insert("security".into(), json!([{"bearerAuth": []}]));
        }
        operations_obj.insert(
            format!("send{}", capitalize(&ch.handler_name)),
            Value::Object(send_op),
        );
    }

    // Components: securitySchemes (bearerAuth) if at least one
    // channel has auth.
    let has_auth = channels.iter().any(|c| c.auth != AuthSpec::None);
    let mut components = Map::new();
    if has_auth {
        components.insert(
            "securitySchemes".into(),
            json!({
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "Token JWT validado pre-upgrade por el `@auth_provider`."
                }
            }),
        );
    }

    let mut root = Map::new();
    root.insert("asyncapi".into(), json!("3.0.0"));
    root.insert(
        "info".into(),
        json!({
            "title": "Fitz API",
            "version": version.unwrap_or("0.1.0"),
            "description": "WebSocket channels auto-generated from `@ws(\"/path\")` decorators in the Fitz program.",
        }),
    );
    root.insert("channels".into(), Value::Object(channels_obj));
    root.insert("operations".into(), Value::Object(operations_obj));
    if !components.is_empty() {
        root.insert("components".into(), Value::Object(components));
    }
    Value::Object(root)
}

/// Capitalizes the first letter of the handler's name to build
/// consistent operation IDs (`receivechat` → `receiveChat`).
/// Naïve — ASCII only; names with non-ASCII characters pass through
/// as-is (very rare case in handler names).
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Decorator/Expr unused-import shims — we reference
// `crate::ast::Decorator` when iterating `decorators` inside
// `pseudo_channels_from_ast`, but the visible binding lives only
// within `for d in decorators`. The explicit import is here for
// future use (e.g. heartbeat docs in 9.w.2.e).
#[allow(dead_code)]
const _USE_DECORATOR: fn(&Decorator) = |_| {};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn schema_for(src: &str) -> Value {
        let program = parse(tokenize(src).expect("lex")).expect("parse");
        let channels = pseudo_channels_from_ast(&program).expect("channels");
        generate_asyncapi(&channels, &program)
    }

    #[test]
    fn asyncapi_emits_version_3_0_0() {
        let src = "@ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        assert_eq!(s["asyncapi"], json!("3.0.0"));
    }

    #[test]
    fn asyncapi_simple_channel_with_str() {
        let src = "@ws(\"/echo\")\n\
                   async fn echo(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        let ch = &s["channels"]["/echo"];
        assert_eq!(ch["address"], json!("/echo"));
        // The msg's schema is type=string.
        let payload = &ch["messages"]["msg"]["payload"];
        assert_eq!(payload["type"], json!("string"));
    }

    #[test]
    fn asyncapi_generates_two_operations_per_channel() {
        let src = "@ws(\"/echo\")\n\
                   async fn echo(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        let ops = s["operations"].as_object().expect("operations object");
        assert!(ops.contains_key("receiveEcho"), "expected receiveEcho");
        assert!(ops.contains_key("sendEcho"), "expected sendEcho");
        let recv = &ops["receiveEcho"];
        assert_eq!(recv["action"], json!("receive"));
        let send = &ops["sendEcho"];
        assert_eq!(send["action"], json!("send"));
    }

    #[test]
    fn asyncapi_custom_type_emits_payload_object() {
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<ChatMsg>) -> Null { return null }";
        let s = schema_for(src);
        let payload = &s["channels"]["/chat"]["messages"]["msg"]["payload"];
        // Nominal type → $ref to components/schemas.
        assert!(
            payload["$ref"].is_string(),
            "expected $ref, was: {:?}",
            payload
        );
    }

    #[test]
    fn asyncapi_without_ws_emits_empty_channels() {
        let src = "@get(\"/x\")\nfn x() -> Str => \"y\"";
        let s = schema_for(src);
        assert!(s["channels"].as_object().unwrap().is_empty());
        assert!(s["operations"].as_object().unwrap().is_empty());
    }

    #[test]
    fn asyncapi_authenticated_handler_emits_security() {
        let src = "type User { id: Int, name: Str, role: Str }\n\
                   @auth_provider\n\
                   fn check(h: Map<Str, Str>) -> Result<User> { return Err(\"x\") }\n\
                   @authenticated\n\
                   @ws(\"/me\")\n\
                   async fn me(conn: WsConn<Str>, user: User) -> Null { return null }";
        let s = schema_for(src);
        // components.securitySchemes.bearerAuth present.
        let scheme = &s["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(scheme["type"], json!("http"));
        assert_eq!(scheme["scheme"], json!("bearer"));
        // Operation carries security.
        let recv = &s["operations"]["receiveMe"];
        let sec = recv["security"].as_array().expect("security array");
        assert_eq!(sec.len(), 1);
        assert!(sec[0]["bearerAuth"].is_array());
    }

    #[test]
    fn asyncapi_program_without_auth_does_not_emit_security_schemes() {
        let src = "@ws(\"/x\")\n\
                   async fn x(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        // Without auth, components may not be present.
        let comp = s.get("components");
        assert!(comp.is_none() || comp.unwrap().get("securitySchemes").is_none());
    }

    // ---- 9.w.2-wsconn-bidir — `WsConn<In, Out>` AsyncAPI ----

    #[test]
    fn asyncapi_wsconn_bidir_emits_two_distinct_messages() {
        // 9.w.2-wsconn-bidir — asymmetric channel generates `msg_in`
        // (receive) and `msg_out` (send) instead of the single `msg`.
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/cmd\")\n\
                   async fn cmd(conn: WsConn<Str, ChatMsg>) -> Null { return null }";
        let s = schema_for(src);
        let messages = &s["channels"]["/cmd"]["messages"];
        // Two messages: msg_in and msg_out, no single `msg`.
        assert!(messages["msg_in"].is_object());
        assert!(messages["msg_out"].is_object());
        assert!(
            messages["msg"].is_null(),
            "single `msg` should not exist in asymmetric bidir"
        );
        // msg_in payload is Str (recv).
        assert_eq!(messages["msg_in"]["payload"]["type"], json!("string"));
        // msg_out payload is ChatMsg (send, nominal $ref).
        assert!(messages["msg_out"]["payload"]["$ref"].is_string());
    }

    #[test]
    fn asyncapi_wsconn_bidir_operations_point_to_distinct_messages() {
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/cmd\")\n\
                   async fn cmd(conn: WsConn<Str, ChatMsg>) -> Null { return null }";
        let s = schema_for(src);
        let receive = &s["operations"]["receiveCmd"];
        let send = &s["operations"]["sendCmd"];
        let recv_msg_ref = receive["messages"][0]["$ref"].as_str().unwrap();
        let send_msg_ref = send["messages"][0]["$ref"].as_str().unwrap();
        assert!(
            recv_msg_ref.ends_with("/messages/msg_in"),
            "receive should point to msg_in, was: {}",
            recv_msg_ref
        );
        assert!(
            send_msg_ref.ends_with("/messages/msg_out"),
            "send should point to msg_out, was: {}",
            send_msg_ref
        );
    }

    #[test]
    fn asyncapi_wsconn_symmetric_keeps_emitting_single_msg() {
        // Compat: `WsConn<T>` (symmetric) still emits the single
        // `msg` (does not break existing consumers).
        let src = "@ws(\"/c\")\n\
                   async fn c(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        let messages = &s["channels"]["/c"]["messages"];
        assert!(messages["msg"].is_object());
        assert!(messages["msg_in"].is_null());
        assert!(messages["msg_out"].is_null());
    }

    // ---- 9.w.2-binary-frames — `WsConn<Bytes>` AsyncAPI ----

    #[test]
    fn asyncapi_wsconn_bytes_emits_payload_binary() {
        let src = "@ws(\"/raw\")\n\
                   async fn raw(conn: WsConn<Bytes>) -> Null { return null }";
        let s = schema_for(src);
        let msg = &s["channels"]["/raw"]["messages"]["msg"];
        assert_eq!(msg["contentType"], json!("application/octet-stream"));
        let payload = &msg["payload"];
        assert_eq!(payload["type"], json!("string"));
        assert_eq!(payload["format"], json!("binary"));
    }

    #[test]
    fn asyncapi_wsconn_bytes_summary_says_binary() {
        let src = "@ws(\"/raw\")\n\
                   async fn raw(conn: WsConn<Bytes>) -> Null { return null }";
        let s = schema_for(src);
        let summary = s["channels"]["/raw"]["messages"]["msg"]["summary"]
            .as_str()
            .expect("summary string");
        assert!(
            summary.contains("binary") || summary.contains("Bytes"),
            "summary should mention binary or Bytes, was: {}",
            summary
        );
    }

    #[test]
    fn asyncapi_wsconn_str_does_not_collide_with_bytes() {
        // Sanity: T = Str still emits `application/json`, doesn't
        // get contaminated by the Bytes adjustment.
        let src = "@ws(\"/c\")\n\
                   async fn c(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        let msg = &s["channels"]["/c"]["messages"]["msg"];
        assert_eq!(msg["contentType"], json!("application/json"));
        assert_eq!(msg["payload"]["type"], json!("string"));
        assert!(msg["payload"]["format"].is_null());
    }

    #[test]
    fn asyncapi_multiple_channels_are_sorted_by_path() {
        let src = "@ws(\"/zeta\")\n\
                   async fn z(conn: WsConn<Str>) -> Null { return null }\n\
                   @ws(\"/alpha\")\n\
                   async fn a(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        let keys: Vec<&str> = s["channels"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, vec!["/alpha", "/zeta"]);
    }
}
