//! HTTP client outbound — built-in `http` module.
//!
//! Mini-fase HTTP client (2026-06-18). Detectado como deuda explícita
//! durante el desarrollo de **fitzwatch** (status page open-source
//! escrito en Fitz puro), que necesita hacer GET/HEAD outbound para
//! chequear si las URLs monitoreadas responden 200. El roadmap
//! completo vive en `docs/http-client-roadmap.md`.
//!
//! # API target
//!
//! Los 6 builtins se exponen como `Value::Module { name: "http" }` en
//! `evaluator::register_builtins`. Todos son async y devuelven
//! `Future<Result<HttpClientResponse>>`:
//!
//! - `http.get(url: Str)`
//! - `http.head(url: Str)`
//! - `http.post(url: Str, body)`
//! - `http.put(url: Str, body)`
//! - `http.delete(url: Str)`
//! - `http.request(opts: Map)` — low-level con
//!   `method`/`url`/`timeout_ms`/`headers`/`body`/`follow_redirects`.
//!
//! El body acepta tres shapes:
//!
//! - `Value::Str` → enviado as-is sin tocar headers.
//! - `Value::Map<Str, Any>` → serializa con `value_to_json` y agrega
//!   `Content-Type: application/json`.
//! - `Value::Bytes` → enviado as-is sin tocar headers.
//!
//! # Modelo de errores
//!
//! - Status 4xx/5xx → `Result::Ok` con `r.status` (los chequea el
//!   user; no son errores de transporte).
//! - Timeout / DNS fail / conexión rechazada / TLS handshake fail /
//!   URL inválida / body no-marshalleable → `Result::Err(Str)` con
//!   prefijo identificable (`"timeout después de Nms"`,
//!   `"DNS no resuelve: <host>"`, etc).
//!
//! # Paralelismo
//!
//! El `Client` se comparte via `LazyLock` (clones son baratos —
//! adentro lleva un `Arc`). Cada request usa una clone del client.
//! Para `follow_redirects=false` armamos un Client one-shot ad-hoc
//! (no se cachea — caso raro).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use reqwest::Method;

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::{shared, FitzFuture, ResultVariant, Value};

/// Default per-request timeout cuando el user no pasa `timeout_ms`.
/// 30 segundos matchea el default de `reqwest::Client`.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Shared `reqwest::Client`. `LazyLock` lo inicializa al primer
/// acceso y lo mantiene vivo para todo el proceso. Configurado con
/// `follow_redirects=true` (default reqwest) y timeout default de
/// 30s (lo override-amos per-request con `RequestBuilder::timeout`).
///
/// **Crash policy**: si el client falla al inicializar (config de
/// rustls inválida del entorno), abortamos el proceso con un mensaje
/// claro — es un bug del binario / del entorno, no un runtime error
/// recuperable del usuario.
static SHARED_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .expect("could not build reqwest::Client — broken TLS / rustls config")
});

// ---------------------------------------------------------------------------
// Public entry points — builtin fn pointers para `register_builtins`.
// ---------------------------------------------------------------------------

pub fn builtin_http_get(args: &[Value]) -> FitzResult<Value> {
    let url = extract_single_url("http.get", args)?;
    Ok(Value::new_future(do_simple_request(Method::GET, url, None)))
}

pub fn builtin_http_head(args: &[Value]) -> FitzResult<Value> {
    let url = extract_single_url("http.head", args)?;
    Ok(Value::new_future(do_simple_request(
        Method::HEAD,
        url,
        None,
    )))
}

pub fn builtin_http_delete(args: &[Value]) -> FitzResult<Value> {
    let url = extract_single_url("http.delete", args)?;
    Ok(Value::new_future(do_simple_request(
        Method::DELETE,
        url,
        None,
    )))
}

pub fn builtin_http_post(args: &[Value]) -> FitzResult<Value> {
    let (url, body) = extract_url_and_body("http.post", args)?;
    Ok(Value::new_future(do_simple_request(
        Method::POST,
        url,
        Some(body),
    )))
}

pub fn builtin_http_put(args: &[Value]) -> FitzResult<Value> {
    let (url, body) = extract_url_and_body("http.put", args)?;
    Ok(Value::new_future(do_simple_request(
        Method::PUT,
        url,
        Some(body),
    )))
}

pub fn builtin_http_request(args: &[Value]) -> FitzResult<Value> {
    // Single positional `opts: Map`. Resolvemos todas las keys
    // ANTES de armar el Future (los errores de shape deben ser
    // FitzError aborting, no Result::Err diferido).
    if args.len() != 1 {
        return Err(arg_count_err(
            "http.request",
            "`http.request(opts: Map)` expects 1 argument (Map of options)",
            1,
            args.len(),
        ));
    }
    let pairs = match &args[0] {
        Value::Map(m) => m.lock().clone(),
        other => {
            return Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Map".into(),
                    found: other.type_name().into(),
                },
                0,
                0,
                format!(
                    "`http.request(opts)` expects Map, received `{}`",
                    other.type_name()
                ),
            ));
        }
    };
    let opts = parse_request_opts(pairs)?;
    Ok(Value::new_future(do_full_request(opts)))
}

// ---------------------------------------------------------------------------
// Internal — argument extraction helpers (validate shape eagerly).
// ---------------------------------------------------------------------------

fn extract_single_url(builtin_name: &str, args: &[Value]) -> FitzResult<String> {
    if args.len() != 1 {
        return Err(arg_count_err(
            builtin_name,
            &format!("`{}` expects 1 argument (url: Str)", builtin_name),
            1,
            args.len(),
        ));
    }
    extract_str_arg(builtin_name, &args[0], "url")
}

fn extract_url_and_body(builtin_name: &str, args: &[Value]) -> FitzResult<(String, Value)> {
    if args.len() != 2 {
        return Err(arg_count_err(
            builtin_name,
            &format!(
                "`{}` expects 2 arguments (url: Str, body: Str|Map|Bytes)",
                builtin_name
            ),
            2,
            args.len(),
        ));
    }
    let url = extract_str_arg(builtin_name, &args[0], "url")?;
    // Body shape se valida adentro del Future (consistencia con
    // marshalling Map → JSON que también vive ahí).
    Ok((url, args[1].clone()))
}

fn extract_str_arg(builtin_name: &str, value: &Value, field: &str) -> FitzResult<String> {
    match value {
        Value::Str(s) => Ok(s.clone()),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Str".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!(
                "`{}` expects `{}` to be Str, received `{}`",
                builtin_name,
                field,
                other.type_name()
            ),
        )),
    }
}

fn arg_count_err(_builtin_name: &str, msg: &str, expected: usize, found: usize) -> FitzError {
    FitzError::new(
        ErrorKind::WrongArgCount { expected, found },
        0,
        0,
        format!("{}, received {}", msg, found),
    )
}

// ---------------------------------------------------------------------------
// Options for `http.request(opts: Map)`.
// ---------------------------------------------------------------------------

/// Parsed + validated options para `http.request`. Construido por
/// `parse_request_opts` desde un `Vec<(Value, Value)>` (el Map shape
/// de Fitz). Errores de shape se reportan eagerly via `FitzError`.
struct RequestOpts {
    method: Method,
    url: String,
    timeout_ms: u64,
    headers: Vec<(String, String)>,
    body: Option<Value>,
    follow_redirects: bool,
}

fn parse_request_opts(pairs: Vec<(Value, Value)>) -> FitzResult<RequestOpts> {
    // Inicialmente sin defaults — los completamos al final desde lo
    // recolectado. Sólo `method` y `url` son obligatorios.
    let mut method_str: Option<String> = None;
    let mut url: Option<String> = None;
    let mut timeout_ms: Option<u64> = None;
    let mut headers: Option<Vec<(String, String)>> = None;
    let mut body: Option<Value> = None;
    let mut follow_redirects: Option<bool> = None;

    for (k, v) in pairs.into_iter() {
        let key = match k {
            Value::Str(s) => s,
            other => {
                return Err(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Str".into(),
                        found: other.type_name().into(),
                    },
                    0,
                    0,
                    format!(
                        "`http.request(opts)` Map keys must be Str, found `{}`",
                        other.type_name()
                    ),
                ));
            }
        };

        match key.as_str() {
            "method" => method_str = Some(extract_str_arg("http.request", &v, "method")?),
            "url" => url = Some(extract_str_arg("http.request", &v, "url")?),
            "timeout_ms" => match v {
                Value::Int(n) if n > 0 => timeout_ms = Some(n as u64),
                other => {
                    return Err(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Int (> 0)".into(),
                            found: other.type_name().into(),
                        },
                        0,
                        0,
                        format!(
                            "`http.request(opts)` expects `timeout_ms` to be Int > 0, received `{}`",
                            other.type_name()
                        ),
                    ));
                }
            },
            "headers" => headers = Some(parse_headers_map(&v)?),
            "body" => body = Some(v),
            "follow_redirects" => match v {
                Value::Bool(b) => follow_redirects = Some(b),
                other => {
                    return Err(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            found: other.type_name().into(),
                        },
                        0,
                        0,
                        format!(
                            "`http.request(opts)` expects `follow_redirects` to be Bool, received `{}`",
                            other.type_name()
                        ),
                    ));
                }
            },
            other => {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!(
                        "`http.request(opts)` unrecognised key `{}` — expected: method, url, timeout_ms, headers, body, follow_redirects",
                        other
                    ),
                ));
            }
        }
    }

    let method_str = method_str.ok_or_else(|| {
        FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            "`http.request(opts)` missing required key `method`".to_string(),
        )
    })?;
    let url = url.ok_or_else(|| {
        FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            "`http.request(opts)` missing required key `url`".to_string(),
        )
    })?;

    let method = Method::from_bytes(method_str.to_uppercase().as_bytes()).map_err(|_| {
        FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "`http.request(opts)` `method` must be a valid HTTP method (GET/POST/...), received `{}`",
                method_str
            ),
        )
    })?;

    Ok(RequestOpts {
        method,
        url,
        timeout_ms: timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
        headers: headers.unwrap_or_default(),
        body,
        follow_redirects: follow_redirects.unwrap_or(true),
    })
}

fn parse_headers_map(value: &Value) -> FitzResult<Vec<(String, String)>> {
    let pairs = match value {
        Value::Map(m) => m.lock().clone(),
        other => {
            return Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Map<Str, Str>".into(),
                    found: other.type_name().into(),
                },
                0,
                0,
                format!(
                    "`http.request(opts)` expects `headers` to be Map<Str, Str>, received `{}`",
                    other.type_name()
                ),
            ));
        }
    };

    let mut out = Vec::with_capacity(pairs.len());
    for (k, v) in pairs.into_iter() {
        let name = match k {
            Value::Str(s) => s,
            other => {
                return Err(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Str".into(),
                        found: other.type_name().into(),
                    },
                    0,
                    0,
                    format!("header name must be Str, received `{}`", other.type_name()),
                ));
            }
        };
        let val = match v {
            Value::Str(s) => s,
            other => {
                return Err(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Str".into(),
                        found: other.type_name().into(),
                    },
                    0,
                    0,
                    format!(
                        "header value for `{}` must be Str, received `{}`",
                        name,
                        other.type_name()
                    ),
                ));
            }
        };
        out.push((name, val));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Async core — common dispatch.
// ---------------------------------------------------------------------------

/// Simple methods (`get/head/post/put/delete`) sin opts. `body=None`
/// para los que no llevan body (GET/HEAD/DELETE).
fn do_simple_request(method: Method, url: String, body: Option<Value>) -> FitzFuture {
    let opts = RequestOpts {
        method,
        url,
        timeout_ms: DEFAULT_TIMEOUT_MS,
        headers: Vec::new(),
        body,
        follow_redirects: true,
    };
    do_full_request(opts)
}

fn do_full_request(opts: RequestOpts) -> FitzFuture {
    Box::pin(async move {
        let started = Instant::now();

        // Body marshalling (Str/Map/Bytes → bytes + opt Content-Type).
        // El fallo del marshalling se reporta como `Result::Err` para
        // que el user lo maneje con `match` o `?` — consistente con
        // el resto del modelo (transport-level errors).
        let (body_bytes, auto_content_type): (Option<Vec<u8>>, Option<&'static str>) =
            match opts.body {
                None => (None, None),
                Some(ref v) => match body_to_payload(v) {
                    Ok(p) => p,
                    Err(msg) => return Ok(err_result(format!("http: {}", msg))),
                },
            };

        // Client selection: shared para follow_redirects=true (caso
        // 90%), one-shot ad-hoc para follow_redirects=false.
        let client = if opts.follow_redirects {
            SHARED_CLIENT.clone()
        } else {
            match reqwest::Client::builder()
                .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
                .redirect(reqwest::redirect::Policy::none())
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    return Ok(err_result(format!(
                        "http: could not build one-shot client: {}",
                        e
                    )));
                }
            }
        };

        let mut req = client
            .request(opts.method, &opts.url)
            .timeout(Duration::from_millis(opts.timeout_ms));

        // User-provided headers ANTES del auto Content-Type, así si
        // el user explícitamente setea Content-Type prevalece.
        for (name, value) in &opts.headers {
            req = req.header(name.as_str(), value.as_str());
        }

        // Body application — el dispatch ya validó shape.
        if let Some(bytes) = body_bytes {
            req = req.body(bytes);
            if let Some(ct) = auto_content_type {
                // Solo agregamos auto Content-Type si el user NO lo
                // sobreescribió en `headers`.
                let user_set_ct = opts
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
                if !user_set_ct {
                    req = req.header("content-type", ct);
                }
            }
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return Ok(err_result(format_reqwest_error(&e, &opts.url))),
        };

        let status = resp.status().as_u16() as i64;
        let resp_headers = headers_to_value_map(resp.headers());
        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return Ok(err_result(format!("http: reading response body: {}", e))),
        };
        let duration_ms = started.elapsed().as_millis() as i64;

        Ok(ok_result(build_http_response_instance(
            status,
            body_text,
            resp_headers,
            duration_ms,
        )))
    })
}

// ---------------------------------------------------------------------------
// Body marshalling: Value → bytes + optional auto Content-Type.
// ---------------------------------------------------------------------------

/// Convierte un `Value` body a `(bytes, optional auto Content-Type)`.
/// Solo acepta `Str`/`Map`/`Bytes`. Cualquier otra cosa → `Err(msg)`.
fn body_to_payload(value: &Value) -> Result<(Option<Vec<u8>>, Option<&'static str>), String> {
    match value {
        Value::Null => Ok((None, None)),
        Value::Str(s) => Ok((Some(s.clone().into_bytes()), None)),
        Value::Bytes(b) => Ok((Some(b.clone()), None)),
        Value::Map(_) => {
            // Reusamos `value_to_json` del runtime HTTP server-side
            // (`src/http.rs`). Hace todo el marshalling Map/List/
            // Instance/primitivos recursivo y respeta orden de
            // inserción. Si rebota (claves no-Str, NaN, etc.) lo
            // propagamos al Err.
            let json = crate::http::value_to_json(value)?;
            let bytes = serde_json::to_vec(&json)
                .map_err(|e| format!("could not serialise body to JSON: {}", e))?;
            Ok((Some(bytes), Some("application/json")))
        }
        other => Err(format!(
            "body must be Str, Map<Str, Any> or Bytes — received `{}`",
            other.type_name()
        )),
    }
}

// ---------------------------------------------------------------------------
// Response building.
// ---------------------------------------------------------------------------

/// Convierte los headers de la response (`reqwest::header::HeaderMap`)
/// a un `Value::Map<Str, Str>` con orden de inserción preservado.
/// Si una header viene multi-valuada, gana la última (decisión MVP
/// — refinable si entra demanda real).
fn headers_to_value_map(headers: &HeaderMap) -> Value {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (name, value) in headers.iter() {
        let n = name.as_str().to_string();
        // `to_str` puede fallar si el header no es ASCII válido —
        // muy raro en respuestas reales. Si pasa, dejamos el bytes
        // como `<binary>` placeholder.
        let v = value.to_str().unwrap_or("<binary>").to_string();
        if seen.insert(n.clone(), v).is_none() {
            order.push(n);
        } else if let Some(existing_idx) = order.iter().position(|x| x == &n) {
            // Push hacia atrás del orden — gana la última.
            order.remove(existing_idx);
            order.push(n);
        }
    }
    let pairs: Vec<(Value, Value)> = order
        .into_iter()
        .map(|k| {
            let v = seen.remove(&k).unwrap_or_default();
            (Value::Str(k), Value::Str(v))
        })
        .collect();
    Value::Map(shared(pairs))
}

/// Build a `Value::Instance` con type_name `"HttpClientResponse"` y
/// los 4 fields en el orden declarado por
/// `types::register_http_builtin_types`. El orden ES estable y
/// observable por el usuario (Display recorre fields en orden).
fn build_http_response_instance(
    status: i64,
    body: String,
    headers: Value,
    duration_ms: i64,
) -> Value {
    Value::new_instance(
        "HttpClientResponse".to_string(),
        vec![
            ("status".to_string(), Value::Int(status)),
            ("body".to_string(), Value::Str(body)),
            ("headers".to_string(), headers),
            ("duration_ms".to_string(), Value::Int(duration_ms)),
        ],
    )
}

fn ok_result(v: Value) -> Value {
    Value::Result(ResultVariant::Ok(Box::new(v)))
}

fn err_result(msg: String) -> Value {
    Value::Result(ResultVariant::Err(Box::new(Value::Str(msg))))
}

// ---------------------------------------------------------------------------
// Reqwest error → Fitz error message.
// ---------------------------------------------------------------------------

/// Map `reqwest::Error` a un mensaje user-facing con prefijo
/// identificable. Distingue timeout / DNS fail / connect refused /
/// builder/parse errors / TLS handshake. La idea es que el user
/// pueda parsear el prefix con `starts_with` o regex si quiere.
fn format_reqwest_error(e: &reqwest::Error, url: &str) -> String {
    if e.is_timeout() {
        // El message de reqwest dice "operation timed out" — agregamos
        // el target para hacer debug obvious.
        return format!("http: timeout calling `{}`: {}", url, e);
    }
    if e.is_connect() {
        return format!("http: could not connect to `{}`: {}", url, e);
    }
    if e.is_builder() {
        return format!("http: invalid request to `{}`: {}", url, e);
    }
    if e.is_request() {
        return format!("http: request failed to `{}`: {}", url, e);
    }
    // Default — catches body/decode/redirect/status errors.
    format!("http: error calling `{}`: {}", url, e)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs_to_map(pairs: Vec<(Value, Value)>) -> Value {
        Value::Map(shared(pairs))
    }

    fn unwrap_str(v: Value) -> String {
        match v {
            Value::Str(s) => s,
            other => panic!("expected Str, got {:?}", other),
        }
    }

    fn unwrap_int(v: Value) -> i64 {
        match v {
            Value::Int(n) => n,
            other => panic!("expected Int, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------
    // Argument shape validation (synchronous, no network).
    // -----------------------------------------------------------------

    #[test]
    fn http_get_requires_one_arg() {
        let err = builtin_http_get(&[]).unwrap_err();
        assert!(err.message.contains("1 argument"));
        let err = builtin_http_get(&[Value::Str("a".into()), Value::Str("b".into())]).unwrap_err();
        assert!(err.message.contains("1 argument"));
    }

    #[test]
    fn http_get_requires_str_url() {
        let err = builtin_http_get(&[Value::Int(42)]).unwrap_err();
        assert!(err.message.contains("url"));
        assert!(err.message.contains("Str"));
    }

    #[test]
    fn http_post_requires_two_args() {
        let err = builtin_http_post(&[Value::Str("http://x".into())]).unwrap_err();
        assert!(err.message.contains("2 arguments"));
    }

    #[test]
    fn http_request_requires_map_opts() {
        let err = builtin_http_request(&[Value::Str("not a map".into())]).unwrap_err();
        assert!(err.message.contains("Map"));
    }

    #[test]
    fn http_request_rejects_unknown_key() {
        let pairs = vec![
            (Value::Str("method".into()), Value::Str("GET".into())),
            (Value::Str("url".into()), Value::Str("http://x".into())),
            (Value::Str("frobnicate".into()), Value::Bool(true)),
        ];
        let err = builtin_http_request(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("frobnicate"));
        assert!(err.message.contains("unrecognised key"));
    }

    #[test]
    fn http_request_requires_method_and_url() {
        let pairs = vec![(Value::Str("url".into()), Value::Str("http://x".into()))];
        let err = builtin_http_request(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("method"));

        let pairs = vec![(Value::Str("method".into()), Value::Str("GET".into()))];
        let err = builtin_http_request(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("url"));
    }

    #[test]
    fn http_request_rejects_invalid_method() {
        let pairs = vec![
            (Value::Str("method".into()), Value::Str("FROB BAR".into())),
            (Value::Str("url".into()), Value::Str("http://x".into())),
        ];
        let err = builtin_http_request(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("HTTP method"));
    }

    #[test]
    fn http_request_rejects_non_int_timeout() {
        let pairs = vec![
            (Value::Str("method".into()), Value::Str("GET".into())),
            (Value::Str("url".into()), Value::Str("http://x".into())),
            (Value::Str("timeout_ms".into()), Value::Str("5000".into())),
        ];
        let err = builtin_http_request(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("timeout_ms"));
    }

    #[test]
    fn http_request_rejects_zero_or_negative_timeout() {
        let pairs = vec![
            (Value::Str("method".into()), Value::Str("GET".into())),
            (Value::Str("url".into()), Value::Str("http://x".into())),
            (Value::Str("timeout_ms".into()), Value::Int(0)),
        ];
        let err = builtin_http_request(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("timeout_ms"));
    }

    // -----------------------------------------------------------------
    // Body marshalling.
    // -----------------------------------------------------------------

    #[test]
    fn body_to_payload_str_keeps_as_is() {
        let (bytes, ct) = body_to_payload(&Value::Str("hello".into())).unwrap();
        assert_eq!(bytes.unwrap(), b"hello");
        assert!(ct.is_none());
    }

    #[test]
    fn body_to_payload_bytes_keeps_as_is() {
        let (bytes, ct) = body_to_payload(&Value::Bytes(vec![1, 2, 3])).unwrap();
        assert_eq!(bytes.unwrap(), vec![1, 2, 3]);
        assert!(ct.is_none());
    }

    #[test]
    fn body_to_payload_map_serialises_to_json() {
        let pairs = vec![
            (Value::Str("k".into()), Value::Int(42)),
            (Value::Str("v".into()), Value::Str("x".into())),
        ];
        let (bytes, ct) = body_to_payload(&pairs_to_map(pairs)).unwrap();
        assert_eq!(ct, Some("application/json"));
        let json: serde_json::Value = serde_json::from_slice(&bytes.unwrap()).unwrap();
        assert_eq!(json["k"], 42);
        assert_eq!(json["v"], "x");
    }

    #[test]
    fn body_to_payload_null_means_no_body() {
        let (bytes, ct) = body_to_payload(&Value::Null).unwrap();
        assert!(bytes.is_none());
        assert!(ct.is_none());
    }

    #[test]
    fn body_to_payload_rejects_int() {
        let err = body_to_payload(&Value::Int(42)).unwrap_err();
        assert!(err.contains("body must be Str, Map"));
        assert!(err.contains("Int"));
    }

    // -----------------------------------------------------------------
    // Header marshalling.
    // -----------------------------------------------------------------

    #[test]
    fn parse_headers_map_accepts_str_str() {
        let pairs = vec![
            (Value::Str("X-A".into()), Value::Str("1".into())),
            (Value::Str("X-B".into()), Value::Str("2".into())),
        ];
        let parsed = parse_headers_map(&pairs_to_map(pairs)).unwrap();
        assert_eq!(
            parsed,
            vec![
                ("X-A".to_string(), "1".to_string()),
                ("X-B".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn parse_headers_map_rejects_non_str_value() {
        let pairs = vec![(Value::Str("X-A".into()), Value::Int(42))];
        let err = parse_headers_map(&pairs_to_map(pairs)).unwrap_err();
        assert!(err.message.contains("header value"));
    }

    #[test]
    fn parse_headers_map_rejects_non_map() {
        let err = parse_headers_map(&Value::Str("not a map".into())).unwrap_err();
        assert!(err.message.contains("Map"));
    }

    // -----------------------------------------------------------------
    // Response shape.
    // -----------------------------------------------------------------

    #[test]
    fn build_http_response_instance_has_4_fields_in_order() {
        let headers = pairs_to_map(vec![]);
        let v = build_http_response_instance(200, "body".into(), headers, 42);
        match v {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "HttpClientResponse");
                let fields = fields.lock();
                assert_eq!(fields.len(), 4);
                assert_eq!(fields[0].0, "status");
                assert_eq!(fields[1].0, "body");
                assert_eq!(fields[2].0, "headers");
                assert_eq!(fields[3].0, "duration_ms");
                assert_eq!(unwrap_int(fields[0].1.clone()), 200);
                assert_eq!(unwrap_str(fields[1].1.clone()), "body");
                assert_eq!(unwrap_int(fields[3].1.clone()), 42);
            }
            other => panic!("expected Instance, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------
    // End-to-end against local axum server.
    // -----------------------------------------------------------------

    use axum::http::{HeaderMap as AxumHeaderMap, StatusCode};
    use axum::routing::{any, get};
    use axum::Router;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    /// Spawn a tiny axum test server in the background. Returns the
    /// base URL (`http://127.0.0.1:<port>`) + a shutdown sender. Bind
    /// to port 0 → OS picks a free port (no test cross-talk).
    async fn spawn_test_server(router: Router) -> (String, oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);
        let (tx, rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        (url, tx)
    }

    async fn await_future_to_value(v: Value) -> Value {
        match v {
            Value::Future(cell) => {
                let fut = cell.0.lock().take().expect("future already awaited");
                fut.await.expect("future returned FitzError")
            }
            other => panic!("expected Future, got {:?}", other),
        }
    }

    fn extract_response_instance(v: Value) -> Vec<(String, Value)> {
        match v {
            Value::Result(ResultVariant::Ok(inner)) => match *inner {
                Value::Instance { fields, .. } => fields.lock().clone(),
                other => panic!("expected HttpClientResponse, got {:?}", other),
            },
            Value::Result(ResultVariant::Err(e)) => {
                panic!("expected Ok, got Err: {:?}", e);
            }
            other => panic!("expected Result, got {:?}", other),
        }
    }

    fn extract_err_message(v: Value) -> String {
        match v {
            Value::Result(ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => s,
                other => panic!("expected Str err, got {:?}", other),
            },
            other => panic!("expected Err, got {:?}", other),
        }
    }

    fn field<'a>(fields: &'a [(String, Value)], name: &str) -> &'a Value {
        &fields
            .iter()
            .find(|(k, _)| k == name)
            .unwrap_or_else(|| panic!("field `{}` not found", name))
            .1
    }

    #[tokio::test]
    async fn http_get_against_local_server_returns_ok_200() {
        let router = Router::new().route("/", get(|| async { "hola fitz" }));
        let (base, shutdown) = spawn_test_server(router).await;

        let v = builtin_http_get(&[Value::Str(base.clone())]).unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);

        assert_eq!(unwrap_int(field(&fields, "status").clone()), 200);
        assert_eq!(unwrap_str(field(&fields, "body").clone()), "hola fitz");
        assert!(unwrap_int(field(&fields, "duration_ms").clone()) >= 0);

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_head_returns_empty_body() {
        let router = Router::new().route("/", get(|| async { "with body" }));
        let (base, shutdown) = spawn_test_server(router).await;

        let v = builtin_http_head(&[Value::Str(base.clone())]).unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);

        assert_eq!(unwrap_int(field(&fields, "status").clone()), 200);
        assert_eq!(unwrap_str(field(&fields, "body").clone()), "");

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_post_with_map_body_sends_json_content_type() {
        let router = Router::new().route(
            "/echo",
            any(|headers: AxumHeaderMap, body: String| async move {
                let ct = headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                // Echo content-type + body to verify auto-JSON.
                format!("{}|{}", ct, body)
            }),
        );
        let (base, shutdown) = spawn_test_server(router).await;

        let body_pairs = vec![
            (Value::Str("k".into()), Value::Int(42)),
            (Value::Str("v".into()), Value::Str("x".into())),
        ];
        let v = builtin_http_post(&[
            Value::Str(format!("{}/echo", base)),
            pairs_to_map(body_pairs),
        ])
        .unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);
        let body = unwrap_str(field(&fields, "body").clone());
        assert!(
            body.starts_with("application/json"),
            "expected application/json, got: {}",
            body
        );
        // The echo splits the body part after "|"; it MUST contain the JSON-encoded map.
        let parts: Vec<&str> = body.splitn(2, '|').collect();
        assert_eq!(parts.len(), 2);
        let parsed: serde_json::Value = serde_json::from_str(parts[1]).unwrap();
        assert_eq!(parsed["k"], 42);
        assert_eq!(parsed["v"], "x");

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_post_with_str_body_keeps_as_is_no_content_type() {
        let router = Router::new().route(
            "/echo",
            any(|headers: AxumHeaderMap, body: String| async move {
                let ct = headers
                    .get("content-type")
                    .map(|v| v.to_str().unwrap_or("").to_string())
                    .unwrap_or_default();
                format!("{}|{}", ct, body)
            }),
        );
        let (base, shutdown) = spawn_test_server(router).await;

        let v = builtin_http_post(&[
            Value::Str(format!("{}/echo", base)),
            Value::Str("raw text".into()),
        ])
        .unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);
        let body = unwrap_str(field(&fields, "body").clone());
        // Sin auto Content-Type (Str body); puede llegar vacío.
        assert!(body.ends_with("|raw text"), "got: {}", body);

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_status_4xx_is_ok_not_err() {
        let router = Router::new().route(
            "/notfound",
            any(|| async { (StatusCode::NOT_FOUND, "missing") }),
        );
        let (base, shutdown) = spawn_test_server(router).await;

        let v = builtin_http_get(&[Value::Str(format!("{}/notfound", base))]).unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);
        assert_eq!(unwrap_int(field(&fields, "status").clone()), 404);
        assert_eq!(unwrap_str(field(&fields, "body").clone()), "missing");

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_request_with_headers_sends_them() {
        let router = Router::new().route(
            "/echo-headers",
            any(|headers: AxumHeaderMap| async move {
                headers
                    .get("x-fitz-token")
                    .map(|v| v.to_str().unwrap_or("").to_string())
                    .unwrap_or_else(|| "<absent>".to_string())
            }),
        );
        let (base, shutdown) = spawn_test_server(router).await;

        let headers_pairs = vec![(
            Value::Str("X-Fitz-Token".into()),
            Value::Str("secret-token".into()),
        )];
        let opts = vec![
            (Value::Str("method".into()), Value::Str("GET".into())),
            (
                Value::Str("url".into()),
                Value::Str(format!("{}/echo-headers", base)),
            ),
            (Value::Str("headers".into()), pairs_to_map(headers_pairs)),
        ];
        let v = builtin_http_request(&[pairs_to_map(opts)]).unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);
        assert_eq!(unwrap_str(field(&fields, "body").clone()), "secret-token");

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_request_user_content_type_overrides_auto() {
        let router = Router::new().route(
            "/ct",
            any(|headers: AxumHeaderMap| async move {
                headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            }),
        );
        let (base, shutdown) = spawn_test_server(router).await;

        let user_headers = vec![(
            Value::Str("content-type".into()),
            Value::Str("application/xml".into()),
        )];
        // Map body would normally auto-set application/json — but
        // user setting content-type explicitly must win.
        let body_pairs = vec![(Value::Str("k".into()), Value::Int(1))];
        let opts = vec![
            (Value::Str("method".into()), Value::Str("POST".into())),
            (Value::Str("url".into()), Value::Str(format!("{}/ct", base))),
            (Value::Str("headers".into()), pairs_to_map(user_headers)),
            (Value::Str("body".into()), pairs_to_map(body_pairs)),
        ];
        let v = builtin_http_request(&[pairs_to_map(opts)]).unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);
        assert_eq!(
            unwrap_str(field(&fields, "body").clone()),
            "application/xml"
        );

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_request_timeout_propagates_as_err() {
        // Slow server: handler sleeps 500ms. Request with timeout=50ms
        // must fail with timeout-prefixed Err.
        let router = Router::new().route(
            "/slow",
            any(|| async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                "should not see this"
            }),
        );
        let (base, shutdown) = spawn_test_server(router).await;

        let opts = vec![
            (Value::Str("method".into()), Value::Str("GET".into())),
            (
                Value::Str("url".into()),
                Value::Str(format!("{}/slow", base)),
            ),
            (Value::Str("timeout_ms".into()), Value::Int(50)),
        ];
        let v = builtin_http_request(&[pairs_to_map(opts)]).unwrap();
        let resolved = await_future_to_value(v).await;
        let msg = extract_err_message(resolved);
        assert!(
            msg.contains("timeout"),
            "expected timeout in err, got: {}",
            msg
        );

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn http_request_invalid_url_returns_err() {
        // Garbage URL — no scheme, illegal chars.
        let v = builtin_http_get(&[Value::Str("not a url at all".into())]).unwrap();
        let resolved = await_future_to_value(v).await;
        let msg = extract_err_message(resolved);
        assert!(
            msg.starts_with("http:"),
            "expected http: prefix, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn http_request_body_int_returns_err() {
        // body=42 (Int) — not a valid shape, must be Result::Err NOT
        // FitzError (consistent with transport errors).
        let v = builtin_http_post(&[Value::Str("http://x".into()), Value::Int(42)]).unwrap();
        let resolved = await_future_to_value(v).await;
        let msg = extract_err_message(resolved);
        assert!(msg.contains("body must be Str"), "got: {}", msg);
    }

    #[tokio::test]
    async fn http_request_no_follow_redirects_returns_301() {
        let router = Router::new().route(
            "/redirect",
            any(|| async {
                (
                    StatusCode::MOVED_PERMANENTLY,
                    [("location", "/elsewhere")],
                    "",
                )
            }),
        );
        let (base, shutdown) = spawn_test_server(router).await;

        let opts = vec![
            (Value::Str("method".into()), Value::Str("GET".into())),
            (
                Value::Str("url".into()),
                Value::Str(format!("{}/redirect", base)),
            ),
            (Value::Str("follow_redirects".into()), Value::Bool(false)),
        ];
        let v = builtin_http_request(&[pairs_to_map(opts)]).unwrap();
        let resolved = await_future_to_value(v).await;
        let fields = extract_response_instance(resolved);
        assert_eq!(unwrap_int(field(&fields, "status").clone()), 301);
        // Headers map contains Location.
        let headers = field(&fields, "headers").clone();
        match headers {
            Value::Map(m) => {
                let has_location = m
                    .lock()
                    .iter()
                    .any(|(k, _)| matches!(k, Value::Str(s) if s.eq_ignore_ascii_case("location")));
                assert!(has_location, "Location header missing");
            }
            other => panic!("expected Map, got {:?}", other),
        }

        let _ = shutdown.send(());
    }
}
