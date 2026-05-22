// asyncapi.rs — Fase 9.w.2.d
//
// Generador AsyncAPI 3.0 para handlers `@ws("/path")`. Paralelo a
// `openapi.rs` pero para el contrato de WebSockets:
//
//   - `OpenAPI 3.1` documenta endpoints HTTP request/response — no
//     captura el shape de mensajes WS bidireccionales.
//   - `AsyncAPI 3.0` es el estándar de la industria para event-driven
//     y streaming APIs: WebSockets, MQTT, Kafka, etc.
//
// Fitz emite ambos (sin que el usuario haga nada): `/openapi.json`
// para los `@get`/`@post`/etc., `/asyncapi.json` para los `@ws`.
// Cada handler `@ws` contribuye un channel + dos operations (receive
// y send), con el tipo `T` del `WsConn<T>` serializado a JSON Schema.
// Auth (`@authenticated`/`@admin`) se documenta con security scheme
// bearer (paralelo a OpenAPI 9.w.1.e).
//
// Decisiones de diseño:
//
//   - **Emit unconditional cuando hay handlers @ws**: si el programa
//     tiene al menos un `@ws`, `/asyncapi.json` se sirve. Programas
//     sin WS no exponen la ruta — paralelo a `/openapi.json` que solo
//     se emite cuando hay handlers HTTP.
//
//   - **Channel por `@ws("/path")`**: cada path declarado es un
//     channel. El nombre del channel es el path mismo (`/chat`).
//
//   - **Una message por channel**: el tipo `T` del `WsConn<T>` define
//     el shape (mismo schema en ambas direcciones — receive y send).
//     Si en el futuro se soportan direcciones tipadas distintas
//     (`WsConn<In, Out>`), se separan acá.
//
//   - **Operations receive + send**: dos operations por channel, una
//     en cada dirección. `receive` = cliente → server (`conn.recv()`);
//     `send` = server → cliente (`conn.send()` / `conn.broadcast()`).
//     Mismo message ref para ambas.
//
//   - **Schema embebido in-line por T nominal** (no `$ref`): los
//     `components/schemas` son OpenAPI-specific. AsyncAPI 3.0 los
//     soporta también, pero para simplicidad inicial emitimos el
//     schema directo en `messages.<name>.payload`. Sub-paso futuro
//     puede unificar ambos via `$ref` cross-spec.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::ast::{Decorator, Expr, Program, Stmt, TypeExpr};
use crate::http::{AuthSpec, HttpRegistry, RouteSpec};
use crate::openapi::{headers_from_decorators, type_expr_to_schema};

/// Vista liviana de un endpoint `@ws("/path")` para el generador
/// AsyncAPI. Paralelo a `OpenApiRouteInfo` pero solo para campos WS.
#[derive(Debug, Clone)]
pub struct AsyncApiChannelInfo {
    /// Path del endpoint (e.g. `"/chat"`).
    pub path: String,
    /// Nombre del handler Fitz — sirve como base para los IDs de
    /// operations (`receive<Handler>` / `send<Handler>`).
    pub handler_name: String,
    /// `TypeExpr` del `T` en `WsConn<T>`. Su schema JSON va al
    /// `messages.<name>.payload`. `None` solo en builds malformados
    /// donde el evaluator no pudo identificar el T.
    pub msg_type: Option<TypeExpr>,
    /// Política de auth de la ruta. `Authenticated`/`Admin` →
    /// security requirement con bearer en el channel.
    pub auth: AuthSpec,
}

/// Adapter desde el `HttpRegistry` del runtime al schema. Filtra
/// solo las rutas WS (`is_ws == true`); las HTTP se ignoran (van por
/// OpenAPI).
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
        auth: s.auth,
    }
}

/// Adapter desde el AST (build-time, `fitz build`) para construir las
/// vistas sin necesidad de evaluar el programa. Espejo de
/// `openapi::pseudo_routes_from_ast` pero filtra `@ws` solamente.
pub fn pseudo_channels_from_ast(
    program: &Program,
) -> Result<Vec<AsyncApiChannelInfo>, crate::error::FitzError> {
    let mut out = Vec::new();
    for s in program {
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
                _ => continue, // checker ya rechazó esto
            };
            // Detectar el WsConn<T> param para extraer T.
            let msg_type = params.iter().find_map(|p| match &p.type_ {
                Some(TypeExpr::Generic { name: n, args }) if n == "WsConn" && args.len() == 1 => {
                    Some(args[0].clone())
                }
                _ => None,
            });
            // Auth desde decorators.
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
            // `header_params` y otros no aplican a `@ws` (no se
            // declaran `@header` sobre handlers WS); mantenemos el
            // helper de openapi disponible por consistencia futura.
            let _ = headers_from_decorators(decorators, params);
            let _ = msg_type.clone();
            out.push(AsyncApiChannelInfo {
                path,
                handler_name: name.clone(),
                msg_type,
                auth,
            });
        }
    }
    Ok(out)
}

/// Genera el schema AsyncAPI 3.0 completo. Output listo para
/// `serde_json::to_string` → `/asyncapi.json`.
///
/// Estructura emitida:
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

/// Variante que acepta `info.version` override. El runtime y el
/// codegen leen `@server(api_version=...)` igual que para OpenAPI.
pub fn generate_asyncapi_with_version(
    channels: &[AsyncApiChannelInfo],
    program: &Program,
    version: Option<&str>,
) -> Value {
    let _ = program;
    let mut channels_obj: Map<String, Value> = Map::new();
    let mut operations_obj: Map<String, Value> = Map::new();

    // Mapa ordenado para emitir channels en orden determinista (path
    // ascendente — paralelo al orden de declaración para reuso de UX).
    let mut sorted: BTreeMap<&str, &AsyncApiChannelInfo> = BTreeMap::new();
    for c in channels {
        sorted.insert(c.path.as_str(), c);
    }

    for (path, ch) in &sorted {
        let msg_schema = ch
            .msg_type
            .as_ref()
            .map(type_expr_to_schema)
            .unwrap_or_else(|| json!({}));
        let msg_name = ch
            .msg_type
            .as_ref()
            .map(|t| t.display_name())
            .unwrap_or_else(|| "AnyMessage".to_string());

        // Channel entry.
        let mut messages = Map::new();
        messages.insert(
            "msg".into(),
            json!({
                "name": msg_name,
                "title": format!("Mensaje de `{}`", ch.handler_name),
                "summary": format!("Frame text JSON ↔ Fitz `{}`", msg_name),
                "contentType": "application/json",
                "payload": msg_schema,
            }),
        );
        channels_obj.insert(
            (*path).to_string(),
            json!({
                "address": path,
                "title": format!("Canal WebSocket `{}`", ch.handler_name),
                "description": match ch.auth {
                    AuthSpec::None => "Sin auth — abierto a cualquier cliente.".to_string(),
                    AuthSpec::Authenticated => "Requiere bearer token (validado pre-upgrade por el `@auth_provider`).".to_string(),
                    AuthSpec::Admin => "Requiere bearer token + rol admin (validado pre-upgrade).".to_string(),
                },
                "messages": messages,
            }),
        );

        // Operations: receive (client→server) + send (server→client).
        // En AsyncAPI 3.0 los $ref usan JSON Pointer; `/` se escapa
        // como `~1` y `~` como `~0`. El path Fitz típicamente no tiene
        // `~`, pero el `/` inicial sí necesita el escape.
        let path_ref = path.replace('~', "~0").replace('/', "~1");
        let channel_ref = format!("#/channels/{}", path_ref);
        let msg_ref = format!("#/channels/{}/messages/msg", path_ref);

        let mut receive_op = Map::new();
        receive_op.insert("action".into(), json!("receive"));
        receive_op.insert("channel".into(), json!({"$ref": channel_ref}));
        receive_op.insert("messages".into(), json!([{"$ref": msg_ref}]));
        receive_op.insert(
            "summary".into(),
            json!(format!(
                "El handler `{}` recibe mensajes del cliente via `conn.recv()`.",
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
        send_op.insert("messages".into(), json!([{"$ref": msg_ref}]));
        send_op.insert(
            "summary".into(),
            json!(format!(
                "El handler `{}` envía mensajes al cliente via `conn.send()` o `conn.broadcast()`.",
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

    // Components: securitySchemes (bearerAuth) si al menos un channel
    // tiene auth.
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
            "description": "Canales WebSocket auto-generados desde decoradores `@ws(\"/path\")` del programa Fitz.",
        }),
    );
    root.insert("channels".into(), Value::Object(channels_obj));
    root.insert("operations".into(), Value::Object(operations_obj));
    if !components.is_empty() {
        root.insert("components".into(), Value::Object(components));
    }
    Value::Object(root)
}

/// Capitaliza la primera letra del nombre del handler para armar
/// IDs de operations consistentes (`receivechat` → `receiveChat`).
/// Naïve — solo ASCII; nombres con caracteres no-ASCII pasan tal cual
/// (caso muy raro en handler names).
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Decorator/Expr unused-import shims — `crate::ast::Decorator` lo
// referenciamos al iterar `decorators` adentro de `pseudo_channels_from_ast`,
// pero el binding visible vive solo dentro de `for d in decorators`. El
// import explícito está para futuro uso (e.g. heartbeat docs en 9.w.2.e).
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
    fn asyncapi_emite_version_3_0_0() {
        let src = "@ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        assert_eq!(s["asyncapi"], json!("3.0.0"));
    }

    #[test]
    fn asyncapi_channel_simple_con_str() {
        let src = "@ws(\"/echo\")\n\
                   async fn echo(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        let ch = &s["channels"]["/echo"];
        assert_eq!(ch["address"], json!("/echo"));
        // Schema del msg es type=string.
        let payload = &ch["messages"]["msg"]["payload"];
        assert_eq!(payload["type"], json!("string"));
    }

    #[test]
    fn asyncapi_genera_dos_operations_por_channel() {
        let src = "@ws(\"/echo\")\n\
                   async fn echo(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        let ops = s["operations"].as_object().expect("operations object");
        assert!(ops.contains_key("receiveEcho"), "esperaba receiveEcho");
        assert!(ops.contains_key("sendEcho"), "esperaba sendEcho");
        let recv = &ops["receiveEcho"];
        assert_eq!(recv["action"], json!("receive"));
        let send = &ops["sendEcho"];
        assert_eq!(send["action"], json!("send"));
    }

    #[test]
    fn asyncapi_tipo_custom_emite_payload_object() {
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<ChatMsg>) -> Null { return null }";
        let s = schema_for(src);
        let payload = &s["channels"]["/chat"]["messages"]["msg"]["payload"];
        // Tipo nominal → $ref a components/schemas.
        assert!(
            payload["$ref"].is_string(),
            "esperaba $ref, fue: {:?}",
            payload
        );
    }

    #[test]
    fn asyncapi_sin_ws_emite_channels_vacios() {
        let src = "@get(\"/x\")\nfn x() -> Str => \"y\"";
        let s = schema_for(src);
        assert!(s["channels"].as_object().unwrap().is_empty());
        assert!(s["operations"].as_object().unwrap().is_empty());
    }

    #[test]
    fn asyncapi_authenticated_handler_emite_security() {
        let src = "type User { id: Int, name: Str, role: Str }\n\
                   @auth_provider\n\
                   fn check(h: Map<Str, Str>) -> Result<User> { return Err(\"x\") }\n\
                   @authenticated\n\
                   @ws(\"/me\")\n\
                   async fn me(conn: WsConn<Str>, user: User) -> Null { return null }";
        let s = schema_for(src);
        // components.securitySchemes.bearerAuth presente.
        let scheme = &s["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(scheme["type"], json!("http"));
        assert_eq!(scheme["scheme"], json!("bearer"));
        // Operation lleva security.
        let recv = &s["operations"]["receiveMe"];
        let sec = recv["security"].as_array().expect("security array");
        assert_eq!(sec.len(), 1);
        assert!(sec[0]["bearerAuth"].is_array());
    }

    #[test]
    fn asyncapi_programa_sin_auth_no_emite_security_schemes() {
        let src = "@ws(\"/x\")\n\
                   async fn x(conn: WsConn<Str>) -> Null { return null }";
        let s = schema_for(src);
        // Sin auth, components puede no estar.
        let comp = s.get("components");
        assert!(comp.is_none() || comp.unwrap().get("securitySchemes").is_none());
    }

    #[test]
    fn asyncapi_multiples_channels_se_ordenan_por_path() {
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
