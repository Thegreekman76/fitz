//! SMTP outbound — built-in `smtp` module.
//!
//! Mini-tanda SMTP builtin (2026-06-19). Detectada como deuda explícita
//! durante el desarrollo de **fitzwatch** (status page open-source escrito
//! en Fitz puro), que necesita despachar notificaciones por email cuando
//! se abre/cierra un incident. Workaround actual era webhook + servicio
//! externo (n8n/ifttt/zapier traducen webhook → SMTP). Este módulo
//! convierte SMTP outbound en ciudadano de primera del lenguaje.
//!
//! # API target
//!
//! El builtin se expone como `Value::Module { name: "smtp" }` en
//! `evaluator::register_builtins`. El único builtin actual es async y
//! devuelve `Future<Result<SmtpResult>>`:
//!
//! - `smtp.send(opts: Map)` — manda un mail. `opts` lleva keys
//!   `to` / `from` / `subject` (todas required, `Str`) y AL MENOS UNO
//!   de `body` / `body_text` / `body_html` (`Str`).
//!
//! Cuando vienen `body_text` + `body_html` juntos, lettre arma un
//! `multipart/alternative` (cliente moderno: gana HTML, fallback texto).
//! `body` solo (sin sufijos) es alias de `body_text`.
//!
//! # Configuración
//!
//! Por env vars al boot (paralelo a `db.connect`):
//!
//! - `SMTP_HOST` — required (sin default).
//! - `SMTP_PORT` — default según TLS (587 starttls, 465 implicit, 25 none).
//! - `SMTP_USER` + `SMTP_PASSWORD` — auth opcional (ambos juntos o ninguno).
//! - `SMTP_FROM` — default `From` si la opts no especifica.
//! - `SMTP_TLS` — `"starttls"` (default), `"implicit"`, `"none"`.
//!
//! `smtp.configure({...})` queda como deuda menor (post-MVP). Por ahora
//! solo env vars.
//!
//! # Modelo de errores
//!
//! Paralelo a `http.X` / `jwt.encode` / `db.connect`:
//! `Result::Err(Str)` con prefijo `"smtp: ..."` para errores de transporte
//! (DNS, conexión, auth fail, TLS handshake, server reject, address parse
//! inválido, missing config). Status 5xx del servidor SMTP cuenta como
//! Err (no hay `r.delivered = false` con éxito parcial; lettre acepta el
//! relay sólo si el server responde 250). El user maneja con `match` o
//! `?` igual que el resto.
//!
//! # Backend
//!
//! `lettre = "0.11"` con features `tokio1-rustls-tls` + `builder` +
//! `smtp-transport` + `pool` + `hostname`. Linkeado estático sin openssl
//! (rustls), consistente con `reqwest`/`tokio-rustls`/driver Postgres.
//!
//! Connection pool del `AsyncSmtpTransport` reusa la conexión entre
//! sends (caso típico: handler HTTP o cron job que despacha varios
//! mails seguidos).

use std::sync::LazyLock;
use std::time::Instant;

use lettre::message::{header::ContentType, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::{FitzFuture, ResultVariant, Value};

/// Default timeout per send. SMTP suele resolver < 1s pero handshake +
/// TLS + auth puede tardar más en redes lentas. 30s matchea el default
/// del HTTP client.
const _DEFAULT_TIMEOUT_MS: u64 = 30_000;

// ---------------------------------------------------------------------------
// SMTP transport config (env-var based) + shared pool.
// ---------------------------------------------------------------------------

/// Resolved + validated SMTP config from env vars at first send.
struct SmtpConfig {
    host: String,
    port: u16,
    user: Option<String>,
    password: Option<String>,
    from_default: Option<String>,
    tls: TlsMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TlsMode {
    StartTls,
    Implicit,
    None,
}

impl TlsMode {
    fn parse(s: &str) -> Result<TlsMode, String> {
        match s.to_ascii_lowercase().as_str() {
            "starttls" | "" => Ok(TlsMode::StartTls),
            "implicit" | "tls" | "ssl" => Ok(TlsMode::Implicit),
            "none" | "plain" => Ok(TlsMode::None),
            other => Err(format!(
                "SMTP_TLS must be one of `starttls`, `implicit`, `none`, received `{}`",
                other
            )),
        }
    }

    fn default_port(self) -> u16 {
        match self {
            TlsMode::StartTls => 587,
            TlsMode::Implicit => 465,
            TlsMode::None => 25,
        }
    }
}

fn load_config_from_env() -> Result<SmtpConfig, String> {
    let host = std::env::var("SMTP_HOST")
        .map_err(|_| "SMTP_HOST is not set (required for `smtp.send`)".to_string())?;
    if host.trim().is_empty() {
        return Err("SMTP_HOST is empty".to_string());
    }

    let tls = TlsMode::parse(&std::env::var("SMTP_TLS").unwrap_or_default())?;

    let port = match std::env::var("SMTP_PORT") {
        Ok(s) if !s.trim().is_empty() => s
            .trim()
            .parse::<u16>()
            .map_err(|_| format!("SMTP_PORT is not a valid u16: `{}`", s))?,
        _ => tls.default_port(),
    };

    let user = std::env::var("SMTP_USER").ok().filter(|s| !s.is_empty());
    let password = std::env::var("SMTP_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty());

    // SMTP_USER + SMTP_PASSWORD must arrive together. Allowing one without
    // the other is the more frequent silent misconfig of this kind.
    match (&user, &password) {
        (Some(_), None) => {
            return Err(
                "SMTP_USER is set but SMTP_PASSWORD is not — both required for auth".into(),
            );
        }
        (None, Some(_)) => {
            return Err(
                "SMTP_PASSWORD is set but SMTP_USER is not — both required for auth".into(),
            );
        }
        _ => {}
    }

    let from_default = std::env::var("SMTP_FROM").ok().filter(|s| !s.is_empty());

    Ok(SmtpConfig {
        host,
        port,
        user,
        password,
        from_default,
        tls,
    })
}

/// Build the async SMTP transport (with pool) from a config. Returns a
/// `String` error on failure so the caller wraps in `Result::Err(Str)`.
fn build_transport(cfg: &SmtpConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let builder = match cfg.tls {
        TlsMode::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .map_err(|e| format!("could not build TLS relay for `{}`: {}", cfg.host, e))?,
        TlsMode::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
            .map_err(|e| format!("could not build TLS relay for `{}`: {}", cfg.host, e))?,
        TlsMode::None => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host).tls(Tls::None)
        }
    };

    let builder = builder.port(cfg.port);

    let builder = match (&cfg.user, &cfg.password) {
        (Some(u), Some(p)) => builder.credentials(Credentials::new(u.clone(), p.clone())),
        _ => builder,
    };

    Ok(builder.build())
}

/// Shared transport handle. Lazy-initialized at first `smtp.send`. If
/// the config is broken, every send returns `Err(Str)`.
///
/// `LazyLock<Result<...>>` so the error message is preserved across
/// calls (subsequent sends report the same config error, not a fresh
/// re-read of env vars — the config is captured at first call).
struct TransportCache {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from_default: Option<String>,
}

static SHARED_TRANSPORT: LazyLock<Result<TransportCache, String>> = LazyLock::new(|| {
    let cfg = load_config_from_env()?;
    let transport = build_transport(&cfg)?;
    Ok(TransportCache {
        transport,
        from_default: cfg.from_default,
    })
});

// ---------------------------------------------------------------------------
// Public entry point — `smtp.send(opts: Map)`.
// ---------------------------------------------------------------------------

pub fn builtin_smtp_send(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_count_err(
            "`smtp.send(opts: Map)` expects 1 argument (Map of options)",
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
                    "`smtp.send(opts)` expects Map, received `{}`",
                    other.type_name()
                ),
            ));
        }
    };
    let opts = parse_send_opts(pairs)?;
    Ok(Value::new_future(do_send(opts)))
}

// ---------------------------------------------------------------------------
// Options for `smtp.send(opts: Map)`.
// ---------------------------------------------------------------------------

/// Parsed + validated send options. Required keys validated early; body
/// shape ("body" alone vs "body_text" + "body_html") resolved in the
/// async block (it picks the right lettre builder).
struct SendOpts {
    to: String,
    from: Option<String>,
    subject: String,
    body_text: Option<String>,
    body_html: Option<String>,
}

fn parse_send_opts(pairs: Vec<(Value, Value)>) -> FitzResult<SendOpts> {
    let mut to: Option<String> = None;
    let mut from: Option<String> = None;
    let mut subject: Option<String> = None;
    let mut body: Option<String> = None;
    let mut body_text: Option<String> = None;
    let mut body_html: Option<String> = None;

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
                        "`smtp.send(opts)` Map keys must be Str, found `{}`",
                        other.type_name()
                    ),
                ));
            }
        };

        match key.as_str() {
            "to" => to = Some(extract_str(&v, "to")?),
            "from" => from = Some(extract_str(&v, "from")?),
            "subject" => subject = Some(extract_str(&v, "subject")?),
            "body" => body = Some(extract_str(&v, "body")?),
            "body_text" => body_text = Some(extract_str(&v, "body_text")?),
            "body_html" => body_html = Some(extract_str(&v, "body_html")?),
            "attachments" => {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    "`smtp.send(opts)` `attachments` not supported in MVP \
                     (deuda menor — refinable si entra demanda)"
                        .to_string(),
                ));
            }
            other => {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!(
                        "`smtp.send(opts)` unrecognised key `{}` — expected: to, from, subject, body, body_text, body_html",
                        other
                    ),
                ));
            }
        }
    }

    let to = to.ok_or_else(|| missing_key_err("to"))?;
    let subject = subject.ok_or_else(|| missing_key_err("subject"))?;

    // `body` alone is alias for `body_text` (cómodo para el caso 90%).
    if let Some(b) = body {
        if body_text.is_some() {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                "`smtp.send(opts)` cannot pass both `body` and `body_text` — `body` is alias for `body_text`"
                    .to_string(),
            ));
        }
        body_text = Some(b);
    }

    // At least one of body_text/body_html required.
    if body_text.is_none() && body_html.is_none() {
        return Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            "`smtp.send(opts)` requires at least one of: `body`, `body_text`, `body_html`"
                .to_string(),
        ));
    }

    Ok(SendOpts {
        to,
        from,
        subject,
        body_text,
        body_html,
    })
}

fn extract_str(value: &Value, field: &str) -> FitzResult<String> {
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
                "`smtp.send(opts)` expects `{}` to be Str, received `{}`",
                field,
                other.type_name()
            ),
        )),
    }
}

fn missing_key_err(name: &str) -> FitzError {
    FitzError::new(
        ErrorKind::TypeError,
        0,
        0,
        format!("`smtp.send(opts)` missing required key `{}`", name),
    )
}

fn arg_count_err(msg: &str, expected: usize, found: usize) -> FitzError {
    FitzError::new(
        ErrorKind::WrongArgCount { expected, found },
        0,
        0,
        format!("{}, received {}", msg, found),
    )
}

// ---------------------------------------------------------------------------
// Async core — build the Message, send via shared transport, wrap result.
// ---------------------------------------------------------------------------

fn do_send(opts: SendOpts) -> FitzFuture {
    Box::pin(async move {
        let started = Instant::now();

        // Acquire the shared transport once. If config is broken, every
        // call returns the same error (LazyLock captures it).
        let cache = match &*SHARED_TRANSPORT {
            Ok(c) => c,
            Err(e) => return Ok(err_result(format!("smtp: {}", e))),
        };

        // Resolve `from`: opts.from > SMTP_FROM env var > error.
        let from_str = match opts
            .from
            .or_else(|| cache.from_default.clone())
            .filter(|s| !s.is_empty())
        {
            Some(s) => s,
            None => {
                return Ok(err_result(
                    "smtp: no `from` provided and SMTP_FROM env var is not set".to_string(),
                ));
            }
        };

        // Parse addresses. Invalid → Result::Err.
        let from_mbx = match from_str.parse::<Mailbox>() {
            Ok(m) => m,
            Err(e) => {
                return Ok(err_result(format!(
                    "smtp: invalid `from` address `{}`: {}",
                    from_str, e
                )));
            }
        };
        let to_mbx = match opts.to.parse::<Mailbox>() {
            Ok(m) => m,
            Err(e) => {
                return Ok(err_result(format!(
                    "smtp: invalid `to` address `{}`: {}",
                    opts.to, e
                )));
            }
        };

        // Build the Message. lettre auto-generates a Message-ID per
        // build; we read it back from the headers to return it in
        // SmtpResult.
        let builder = Message::builder()
            .from(from_mbx)
            .to(to_mbx)
            .subject(&opts.subject);

        let message = match (opts.body_text, opts.body_html) {
            (Some(text), Some(html)) => builder.multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html),
                    ),
            ),
            (Some(text), None) => builder.body(text),
            (None, Some(html)) => builder.header(ContentType::TEXT_HTML).body(html),
            (None, None) => unreachable!("parse_send_opts enforces at least one body"),
        };

        let message = match message {
            Ok(m) => m,
            Err(e) => {
                return Ok(err_result(format!("smtp: could not build message: {}", e)));
            }
        };

        let message_id = message
            .headers()
            .get_raw("Message-ID")
            .map(|s| s.to_string())
            .unwrap_or_default();
        // Strip surrounding `<>` to expose the bare ID, paralelo a la
        // convención de la mayoría de SMTP libs.
        let message_id = message_id
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string();

        match cache.transport.send(message).await {
            Ok(_response) => {
                let duration_ms = started.elapsed().as_millis() as i64;
                Ok(ok_result(build_smtp_result_instance(
                    true,
                    message_id,
                    duration_ms,
                )))
            }
            Err(e) => Ok(err_result(format_smtp_error(&e))),
        }
    })
}

// ---------------------------------------------------------------------------
// SmtpResult instance + helpers.
// ---------------------------------------------------------------------------

fn build_smtp_result_instance(delivered: bool, message_id: String, duration_ms: i64) -> Value {
    Value::new_instance(
        "SmtpResult".to_string(),
        vec![
            ("delivered".to_string(), Value::Bool(delivered)),
            ("message_id".to_string(), Value::Str(message_id)),
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

/// Distinguish transport-level errors with informative prefixes.
/// Lettre's SMTP error API gives us the response code + class; we
/// surface them so the user can pattern-match on the prefix if they
/// want.
fn format_smtp_error(e: &lettre::transport::smtp::Error) -> String {
    let msg = e.to_string();
    if e.is_response() {
        format!("smtp: server rejected mail: {}", msg)
    } else if e.is_client() {
        format!("smtp: client error: {}", msg)
    } else if e.is_transient() {
        format!("smtp: transient error: {}", msg)
    } else if e.is_permanent() {
        format!("smtp: permanent error: {}", msg)
    } else {
        format!("smtp: {}", msg)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::shared;

    fn pairs_to_map(pairs: Vec<(Value, Value)>) -> Value {
        Value::Map(shared(pairs))
    }

    // -----------------------------------------------------------------
    // Argument shape validation (synchronous, no network).
    // -----------------------------------------------------------------

    #[test]
    fn smtp_send_requires_one_arg() {
        let err = builtin_smtp_send(&[]).unwrap_err();
        assert!(err.message.contains("1 argument"));
        let err = builtin_smtp_send(&[Value::Map(shared(vec![])), Value::Map(shared(vec![]))])
            .unwrap_err();
        assert!(err.message.contains("1 argument"));
    }

    #[test]
    fn smtp_send_requires_map_opts() {
        let err = builtin_smtp_send(&[Value::Str("not a map".into())]).unwrap_err();
        assert!(err.message.contains("Map"));
    }

    #[test]
    fn smtp_send_rejects_unknown_key() {
        let pairs = vec![
            (Value::Str("to".into()), Value::Str("a@b.com".into())),
            (Value::Str("subject".into()), Value::Str("Hi".into())),
            (Value::Str("body".into()), Value::Str("Hola".into())),
            (Value::Str("frobnicate".into()), Value::Bool(true)),
        ];
        let err = builtin_smtp_send(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("frobnicate"));
        assert!(err.message.contains("unrecognised key"));
    }

    #[test]
    fn smtp_send_requires_to_and_subject() {
        let pairs = vec![(Value::Str("subject".into()), Value::Str("Hi".into()))];
        let err = builtin_smtp_send(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("`to`"));

        let pairs = vec![(Value::Str("to".into()), Value::Str("a@b.com".into()))];
        let err = builtin_smtp_send(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("`subject`"));
    }

    #[test]
    fn smtp_send_requires_some_body() {
        let pairs = vec![
            (Value::Str("to".into()), Value::Str("a@b.com".into())),
            (Value::Str("subject".into()), Value::Str("Hi".into())),
        ];
        let err = builtin_smtp_send(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("body"));
        assert!(err.message.contains("body_text"));
        assert!(err.message.contains("body_html"));
    }

    #[test]
    fn smtp_send_rejects_body_plus_body_text() {
        let pairs = vec![
            (Value::Str("to".into()), Value::Str("a@b.com".into())),
            (Value::Str("subject".into()), Value::Str("Hi".into())),
            (Value::Str("body".into()), Value::Str("foo".into())),
            (Value::Str("body_text".into()), Value::Str("bar".into())),
        ];
        let err = builtin_smtp_send(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("alias"));
    }

    #[test]
    fn smtp_send_rejects_non_str_to() {
        let pairs = vec![
            (Value::Str("to".into()), Value::Int(42)),
            (Value::Str("subject".into()), Value::Str("Hi".into())),
            (Value::Str("body".into()), Value::Str("Hola".into())),
        ];
        let err = builtin_smtp_send(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("Str"));
        assert!(err.message.contains("`to`"));
    }

    #[test]
    fn smtp_send_attachments_rejected_with_clear_message() {
        let pairs = vec![
            (Value::Str("to".into()), Value::Str("a@b.com".into())),
            (Value::Str("subject".into()), Value::Str("Hi".into())),
            (Value::Str("body".into()), Value::Str("Hola".into())),
            (Value::Str("attachments".into()), Value::new_list(vec![])),
        ];
        let err = builtin_smtp_send(&[pairs_to_map(pairs)]).unwrap_err();
        assert!(err.message.contains("attachments"));
        assert!(err.message.contains("MVP"));
    }

    // -----------------------------------------------------------------
    // TLS parsing
    // -----------------------------------------------------------------

    #[test]
    fn tls_mode_parse_accepts_canonical_values() {
        assert_eq!(TlsMode::parse("starttls").unwrap(), TlsMode::StartTls);
        assert_eq!(TlsMode::parse("STARTTLS").unwrap(), TlsMode::StartTls);
        assert_eq!(TlsMode::parse("implicit").unwrap(), TlsMode::Implicit);
        assert_eq!(TlsMode::parse("none").unwrap(), TlsMode::None);
        assert_eq!(TlsMode::parse("").unwrap(), TlsMode::StartTls);
    }

    #[test]
    fn tls_mode_parse_accepts_aliases() {
        // `tls`/`ssl` are common aliases for implicit TLS (port 465).
        assert_eq!(TlsMode::parse("tls").unwrap(), TlsMode::Implicit);
        assert_eq!(TlsMode::parse("ssl").unwrap(), TlsMode::Implicit);
        assert_eq!(TlsMode::parse("plain").unwrap(), TlsMode::None);
    }

    #[test]
    fn tls_mode_parse_rejects_unknown() {
        let err = TlsMode::parse("frobnicate").unwrap_err();
        assert!(err.contains("frobnicate"));
        assert!(err.contains("starttls"));
    }

    #[test]
    fn tls_mode_default_port() {
        assert_eq!(TlsMode::StartTls.default_port(), 587);
        assert_eq!(TlsMode::Implicit.default_port(), 465);
        assert_eq!(TlsMode::None.default_port(), 25);
    }

    // -----------------------------------------------------------------
    // SmtpResult instance shape.
    // -----------------------------------------------------------------

    #[test]
    fn smtp_result_instance_has_three_fields_in_canonical_order() {
        let v = build_smtp_result_instance(true, "abc@host".into(), 42);
        match v {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "SmtpResult");
                let f = fields.lock();
                assert_eq!(f.len(), 3);
                assert_eq!(f[0].0, "delivered");
                assert_eq!(f[1].0, "message_id");
                assert_eq!(f[2].0, "duration_ms");
                match &f[0].1 {
                    Value::Bool(b) => assert!(b),
                    other => panic!("expected Bool, got {:?}", other),
                }
                match &f[1].1 {
                    Value::Str(s) => assert_eq!(s, "abc@host"),
                    other => panic!("expected Str, got {:?}", other),
                }
                match &f[2].1 {
                    Value::Int(n) => assert_eq!(*n, 42),
                    other => panic!("expected Int, got {:?}", other),
                }
            }
            other => panic!("expected Instance, got {:?}", other),
        }
    }
}
