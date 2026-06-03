//! Fase 12.3.a.2 — Structured logging built-in (output JSON real con
//! tracing-subscriber + TTY detection + Secret redaction).
//!
//! Implementación del sink real de los 4 builtins (`log.info`/`log.warn`/
//! `log.error`/`log.debug`) registrados en el evaluator en 12.3.a.1. El
//! módulo expone `emit_log_record(level, msg, kvs)` que:
//!
//! 1. Pasa por el level gate (`tracing::enabled!(target: "fitz::log", L)`
//!    contra el `EnvFilter` instalado al boot via `init_logging()`).
//!    Default level = `INFO` si `RUST_LOG` no se setea.
//! 2. Determina el format (`Json` vs `Pretty`):
//!    - Override explícito vía `FITZ_LOG_FORMAT=json|pretty`.
//!    - Auto-detect: stderr es TTY → `Pretty`; sino → `Json` (containers/
//!      CI/redirección).
//! 3. Emite el registro a stderr (no contamina stdout del programa
//!    Fitz, donde van los `print(...)` del user).
//! 4. Redacta automáticamente `Value::Secret(_)` como `"<redacted>"` —
//!    en kwargs directos y dentro de List/Map (preview de 12.3.c).
//!
//! Approach híbrido con tracing (decisión 12.3.a.2): el filter de nivel
//! se delega a `tracing` (via `EnvFilter` + `tracing::enabled!`), pero
//! el JSON output lo emite Fitz manual con `serde_json` porque los
//! kwargs heterogéneos runtime no se modelan limpios con las macros
//! `event!` que esperan field names en compile-time. El subscriber
//! queda instalado igual para 12.3.b (auto-trace HTTP + spans +
//! correlación `trace_id` en logs).
//!
//! No reemplaza el helper `emit_log_record` del evaluator — lo
//! reexporta. El evaluator solo importa esta fn pública y la llama.

use std::io::{IsTerminal, Stderr, Write};
use std::sync::{Mutex, OnceLock};

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::value::Value;

/// Sink global de stderr. `Mutex` porque varios threads pueden emitir
/// concurrente (handler HTTP + cron + background) — necesitamos escritura
/// atómica por registro para que las líneas no se intercalen.
///
/// `OnceLock` para inicialización lazy thread-safe: el primer call a
/// `emit_log_record` (o a `init_logging`) lo construye.
static STDERR_LOCK: OnceLock<Mutex<Stderr>> = OnceLock::new();

fn stderr_lock() -> &'static Mutex<Stderr> {
    STDERR_LOCK.get_or_init(|| Mutex::new(std::io::stderr()))
}

/// Format del output. La elección por default depende de TTY detection
/// + override `FITZ_LOG_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// JSON flat — un objeto JSON por línea con `timestamp`, `level`,
    /// `msg` y los kwargs al mismo nivel. Default cuando stderr NO es
    /// TTY (containers/CI/redirección).
    Json,
    /// Pretty — `<ts> <LEVEL> <msg> k1=v1 k2=v2` con ANSI colors por
    /// level. Default cuando stderr es TTY (dev local).
    Pretty,
}

/// Detecta el format a usar según TTY + `FITZ_LOG_FORMAT`. Override
/// gana sobre auto-detect; valor inválido cae silencioso a auto-detect
/// (no quiero abortar el programa por una env var mal seteada).
pub fn detect_format() -> LogFormat {
    if let Ok(v) = std::env::var("FITZ_LOG_FORMAT") {
        match v.to_lowercase().as_str() {
            "json" => return LogFormat::Json,
            "pretty" => return LogFormat::Pretty,
            // Valor desconocido — fallback silencioso a auto-detect.
            // No imprimimos warning porque el sink mismo del warning
            // dependería de esta var y bootstrapearía mal.
            _ => {}
        }
    }
    if std::io::stderr().is_terminal() {
        LogFormat::Pretty
    } else {
        LogFormat::Json
    }
}

/// Inicializa el subscriber `tracing` al boot del binario `fitz`. Se
/// llama una vez desde `main.rs` ANTES de ejecutar el programa user.
/// Idempotente: si ya está inicializado, no-op.
///
/// Razón de instalar el subscriber aunque el output JSON lo emitamos
/// manual: para que `tracing::enabled!(target: "fitz::log", LEVEL)`
/// respete `RUST_LOG`. El `EnvFilter::try_from_default_env()` lee la
/// env var; si no está, default a `info` (más verboso que el default
/// estándar del crate, que es `error` — para Fitz `info` es más útil).
///
/// El layer instalado es no-op (`with_writer(std::io::sink)`) — no
/// emite nada. Está solo para satisfacer la API de `tracing_subscriber::
/// registry()` y mantener el filter activo. En 12.3.b sumamos layers
/// que emiten spans HTTP (auto-trace).
pub fn init_logging() {
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter};

    // Idempotente: si el subscriber global ya está set, set_global_default
    // falla — lo ignoramos.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let noop_layer = fmt::layer().with_writer(std::io::sink);

    // try_init falla si ya hay subscriber instalado — perfect para
    // idempotencia (tests, REPL, fitz dev re-ejecutando, etc.).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(noop_layer)
        .try_init();
}

/// API pública del módulo. Llamada por el evaluator desde
/// `dispatch_builtin_kwargs` (path con kwargs) y desde
/// `builtin_log_<level>` (path positional-only). Implementa el gate
/// de nivel + format detection + emit a stderr.
///
/// `level_str` es uno de `"info"`/`"warn"`/`"error"`/`"debug"`
/// (lowercase, viene del dispatch de Fitz). Internamente se mapea al
/// `tracing::Level` correspondiente para el gate.
pub fn emit_log_record(level_str: &str, msg: &str, kvs: &[(String, Value)]) {
    use tracing::Level;
    let level = match level_str {
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        "debug" => Level::DEBUG,
        // Defensive: nivel desconocido, lo dejamos pasar como INFO.
        // El dispatch del evaluator ya valida el nombre del método.
        _ => Level::INFO,
    };
    // Gate de level via tracing — respeta RUST_LOG.
    // tracing::enabled! es macro: el level debe ser const, despachamos
    // por match manual.
    let enabled = match level {
        Level::ERROR => tracing::enabled!(target: "fitz::log", Level::ERROR),
        Level::WARN => tracing::enabled!(target: "fitz::log", Level::WARN),
        Level::INFO => tracing::enabled!(target: "fitz::log", Level::INFO),
        Level::DEBUG => tracing::enabled!(target: "fitz::log", Level::DEBUG),
        Level::TRACE => tracing::enabled!(target: "fitz::log", Level::TRACE),
    };
    if !enabled {
        return;
    }

    let format = detect_format();
    let line = match format {
        LogFormat::Json => format_json(level_str, msg, kvs),
        LogFormat::Pretty => format_pretty(level_str, msg, kvs),
    };

    // Lock + write + flush atómico por línea. Ignoramos errores de
    // escritura (stderr cerrado, pipe roto) — el sink de logs no debe
    // tirar el programa abajo.
    if let Ok(mut stderr) = stderr_lock().lock() {
        let _ = writeln!(stderr, "{}", line);
        let _ = stderr.flush();
    }
}

/// Construye el JSON flat: `{"timestamp": "...", "level": "INFO",
/// "msg": "...", "trace_id": "...", "span_id": "...", <kwargs>}`.
/// Reservados (`level`/`msg`/`timestamp`/`trace_id`/`span_id`) ya
/// fueron rechazados en el evaluator — no hay riesgo de colisión.
///
/// Fase 12.3.b.1 — `trace_id`/`span_id` se insertan automáticamente
/// si hay span context activo via `with_span_context(...)`. Sin
/// span activo, esos fields se omiten (no se emiten como `null`
/// para no contaminar el shape).
fn format_json(level_str: &str, msg: &str, kvs: &[(String, Value)]) -> String {
    let span_ctx = current_span_context();
    let extra = span_ctx.as_ref().map(|_| 2).unwrap_or(0);
    let mut obj = JsonMap::with_capacity(3 + extra + kvs.len());
    obj.insert("timestamp".into(), JsonValue::String(now_rfc3339()));
    obj.insert("level".into(), JsonValue::String(level_str.to_uppercase()));
    obj.insert("msg".into(), JsonValue::String(msg.to_string()));
    if let Some(ctx) = &span_ctx {
        obj.insert("trace_id".into(), JsonValue::String(ctx.trace_id.clone()));
        obj.insert("span_id".into(), JsonValue::String(ctx.span_id.clone()));
    }
    for (k, v) in kvs {
        obj.insert(k.clone(), value_to_json_redacted(v));
    }
    // `serde_json::to_string` sobre `JsonValue::Object` — preserve_order
    // está activo en nuestra dep (feature `preserve_order`), así que el
    // shape sale en orden de inserción: timestamp, level, msg, kwargs.
    serde_json::to_string(&JsonValue::Object(obj)).unwrap_or_else(|_| {
        format!(
            "{{\"level\":\"{}\",\"msg\":\"<serialize_error>\"}}",
            level_str.to_uppercase()
        )
    })
}

/// Construye la línea pretty: `<ts> <LEVEL> <msg> trace=xxx span=yyy
/// k=v k="str" k=null` con ANSI colors por level. La detección de
/// color usa el mismo TTY que detect_format — si llegamos acá es
/// porque format == Pretty, que implica TTY o override pretty.
///
/// Fase 12.3.b.1 — `trace=xxx span=yyy` se inyecta automáticamente
/// si hay span context activo via `with_span_context(...)`. Sin span
/// activo, los fields se omiten (paralelo a JSON). Forma dim para no
/// dominar visualmente sobre el msg + kwargs del user.
fn format_pretty(level_str: &str, msg: &str, kvs: &[(String, Value)]) -> String {
    let level_upper = level_str.to_uppercase();
    let level_colored = colorize_level(&level_upper);
    let ts = now_rfc3339();
    let span_part = match current_span_context() {
        Some(ctx) => format!(" \x1b[2mtrace={} span={}\x1b[0m", ctx.trace_id, ctx.span_id),
        None => String::new(),
    };
    let kvs_part = if kvs.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = kvs
            .iter()
            .map(|(k, v)| format!("{}={}", k, value_to_pretty(v)))
            .collect();
        format!(" {}", parts.join(" "))
    };
    // ANSI dim sobre el timestamp para que el ojo vaya al level + msg
    // primero. Sin colors si stdout no soporta — el detect_format ya
    // garantiza Pretty solo en TTY o override explícito.
    format!(
        "\x1b[2m{}\x1b[0m {} {}{}{}",
        ts, level_colored, msg, span_part, kvs_part
    )
}

/// ANSI color por level. Convención bunyan/pino/uvicorn:
/// DEBUG=magenta, INFO=green, WARN=yellow, ERROR=red, bold.
fn colorize_level(level_upper: &str) -> String {
    let (code, padding) = match level_upper {
        "DEBUG" => ("\x1b[1;35m", "DEBUG"), // bold magenta
        "INFO" => ("\x1b[1;32m", "INFO "),  // bold green; pad to 5
        "WARN" => ("\x1b[1;33m", "WARN "),  // bold yellow; pad to 5
        "ERROR" => ("\x1b[1;31m", "ERROR"), // bold red
        other => ("", other),
    };
    format!("{}{}\x1b[0m", code, padding)
}

/// Timestamp ISO 8601 / RFC 3339 con milisegundos en UTC. Ejemplo:
/// `2026-06-02T14:23:01.123Z`. Compatible con queries Loki/Datadog
/// y con la mayoría de los parsers de log timestamps.
fn now_rfc3339() -> String {
    use chrono::SecondsFormat;
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Convierte `Value` Fitz a `serde_json::Value` para JSON output con
/// **redacción recursiva** de `Value::Secret`. Versión específica del
/// logger (no usa `http::value_to_json` porque ese rechaza Secret con
/// error — para logs queremos auto-redacción silenciosa).
fn value_to_json_redacted(v: &Value) -> JsonValue {
    match v {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::Int(n) => JsonValue::Number((*n).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            // NaN/Infinity no son JSON-válidos — emitimos como string.
            .unwrap_or_else(|| JsonValue::String(format!("{}", f))),
        Value::Str(s) => JsonValue::String(s.clone()),
        Value::Secret(_) => JsonValue::String("<redacted>".to_string()),
        Value::List(items) => {
            let guard = items.lock();
            let arr: Vec<JsonValue> = guard.iter().map(value_to_json_redacted).collect();
            JsonValue::Array(arr)
        }
        Value::Map(pairs) => {
            let guard = pairs.lock();
            let mut obj = JsonMap::with_capacity(guard.len());
            for (k, v) in guard.iter() {
                let key = match k {
                    Value::Str(s) => s.clone(),
                    other => format!("{}", other),
                };
                obj.insert(key, value_to_json_redacted(v));
            }
            JsonValue::Object(obj)
        }
        // Resto: el evaluator ya rechazó tipos no serializables en el
        // dispatch (Function/Type/Module/DbConn/etc). Si llega algo
        // raro acá es bug — emitimos `null` defensive en lugar de
        // panicar el sink de logs.
        _ => JsonValue::Null,
    }
}

/// Format pretty de un value para `k=v` inline. Strings con comillas
/// dobles (consistente con `print(...)` adentro de containers),
/// Secret redactado, List/Map con shape compacto JSON-like.
fn value_to_pretty(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{:.1}", f)
            } else {
                format!("{}", f)
            }
        }
        Value::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Value::Secret(_) => "<redacted>".to_string(),
        Value::List(items) => {
            let guard = items.lock();
            let parts: Vec<String> = guard.iter().map(value_to_pretty).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Map(pairs) => {
            let guard = pairs.lock();
            let parts: Vec<String> = guard
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::Str(s) => format!("\"{}\"", s),
                        other => format!("{}", other),
                    };
                    format!("{}: {}", key, value_to_pretty(v))
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        _ => "<?>".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Fase 12.3.b.1 — SpanContext + correlación trace_id/span_id en logs.
//
// Infraestructura del span context para correlacionar logs emitidos
// adentro de un mismo request HTTP (o cualquier scope que los wrap-ee).
// Cuando hay un span activo via `with_span_context(ctx, fut)`, todos los
// `log.info/warn/error/debug` emitidos adentro suman automáticamente
// `trace_id` (32 hex chars, compartido por toda la cadena) y `span_id`
// (16 hex chars, único por span) al output JSON / pretty.
//
// Approach: storage propio con `tokio::task_local!` (vs tracing nativo
// `Span::extensions`). Razones:
// - Simplicidad — sin Subscriber/Layer custom de tracing.
// - Control total del shape del SpanContext (compatible con OTel sin
//   forzar la jerarquía de tracing).
// - `task_local!` atraviesa thread boundaries del runtime tokio
//   multi-thread (handlers HTTP saltan workers).
//
// En 12.3.b.2 viene el wrapper HTTP que abre el span automático antes
// de invocar el handler; en 12.3.c migramos al SpanContext de
// OpenTelemetry sin cambiar la API pública.
//
// IDs OTel-compatibles:
// - trace_id = 32 hex chars (16 bytes random sin guiones).
// - span_id = 16 hex chars (8 bytes random sin guiones).
// Generados con `uuid::Uuid::new_v4()` (ya en deps no opcional).

/// Contexto de span activo para correlacionar logs. Inmutable —
/// nuevos spans se construyen con `new_child()` que clona el `trace_id`
/// y genera un `span_id` nuevo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanContext {
    /// 32 hex chars. Constante a lo largo de toda la cadena de spans
    /// nested (un request HTTP entero comparte el mismo trace_id).
    pub trace_id: String,
    /// 16 hex chars. Único por span — cada `new_child()` genera uno
    /// nuevo.
    pub span_id: String,
    /// `span_id` del padre si este es un span anidado. `None` para
    /// spans root (típicamente el del handler HTTP inicial).
    pub parent_span_id: Option<String>,
}

impl SpanContext {
    /// Crea un span root (sin parent). Genera `trace_id` y `span_id`
    /// fresh. Llamado típicamente por el wrapper HTTP al recibir un
    /// request (12.3.b.2).
    pub fn new_root() -> Self {
        Self {
            trace_id: generate_trace_id(),
            span_id: generate_span_id(),
            parent_span_id: None,
        }
    }

    /// Crea un span hijo del actual. Comparte el `trace_id`, genera
    /// `span_id` nuevo, registra el `span_id` del padre. Usado para
    /// spans anidados (e.g. `db.query` adentro de un handler HTTP).
    pub fn new_child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: generate_span_id(),
            parent_span_id: Some(self.span_id.clone()),
        }
    }
}

/// Genera un trace_id = 32 hex chars (16 bytes random). Formato
/// OTel-compatible: las cadenas downstream (Datadog/Jaeger/Tempo) lo
/// aceptan tal cual sin transformación.
fn generate_trace_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    // hex de 16 bytes = 32 chars
    let mut s = String::with_capacity(32);
    for b in bytes.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Genera un span_id = 16 hex chars (8 bytes random). Tomamos los
/// primeros 8 bytes de un Uuid v4 — suficiente entropía para
/// uniqueness práctica (2^64 espacio).
fn generate_span_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    let mut s = String::with_capacity(16);
    for b in bytes.iter().take(8) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

tokio::task_local! {
    /// Span context activo del task actual. Set por
    /// `with_span_context(ctx, fut)`. Leído por `current_span_context()`.
    /// Sin set, las consultas devuelven `None`.
    static SPAN_CONTEXT: SpanContext;
}

/// Devuelve el SpanContext activo del task tokio actual, si hay uno
/// instalado. Llamado por `emit_log_record` para inyectar trace_id/
/// span_id al output. Sin span activo → `None`.
pub fn current_span_context() -> Option<SpanContext> {
    SPAN_CONTEXT.try_with(|ctx| ctx.clone()).ok()
}

/// Ejecuta el future `fut` con `ctx` como span context activo.
/// Wrapper sobre `LocalKey::scope`. Llamado típicamente por el
/// wrapper HTTP al boot del request (12.3.b.2).
///
/// Adentro del scope, `current_span_context()` devuelve `Some(ctx)`.
/// Spans anidados se crean con `ctx.new_child()` + nueva llamada a
/// `with_span_context(child, ...)`.
pub async fn with_span_context<F, Fut, T>(ctx: SpanContext, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    SPAN_CONTEXT.scope(ctx, f()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::SecretInner;
    use parking_lot::Mutex as PlMutex;
    use std::sync::Arc;

    /// Helper: construye un `Value::Map` con `Vec<(Value, Value)>`
    /// adentro de `Arc<parking_lot::Mutex<...>>` (shape post-F17).
    fn make_map(pairs: Vec<(Value, Value)>) -> Value {
        Value::Map(Arc::new(PlMutex::new(pairs)))
    }

    fn make_list(items: Vec<Value>) -> Value {
        Value::List(Arc::new(PlMutex::new(items)))
    }

    fn make_secret(inner: Value) -> Value {
        Value::Secret(SecretInner(Box::new(inner)))
    }

    #[test]
    fn format_json_shape_flat_con_kwargs_basicos() {
        let kvs = vec![
            ("user_id".into(), Value::Int(42)),
            ("role".into(), Value::Str("admin".into())),
            ("active".into(), Value::Bool(true)),
        ];
        let line = format_json("info", "login ok", &kvs);
        // Parseamos para no depender del exacto whitespace.
        let parsed: JsonValue = serde_json::from_str(&line).expect("debería ser JSON válido");
        let obj = parsed.as_object().expect("Object esperado");
        assert_eq!(obj.get("level"), Some(&JsonValue::String("INFO".into())));
        assert_eq!(obj.get("msg"), Some(&JsonValue::String("login ok".into())));
        assert_eq!(obj.get("user_id"), Some(&JsonValue::Number(42.into())));
        assert_eq!(obj.get("role"), Some(&JsonValue::String("admin".into())));
        assert_eq!(obj.get("active"), Some(&JsonValue::Bool(true)));
        // timestamp es ISO 8601 que termina en `Z`.
        let ts = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .expect("timestamp string esperado");
        assert!(
            ts.ends_with('Z'),
            "timestamp debería terminar en Z, fue {}",
            ts
        );
    }

    #[test]
    fn format_json_secret_directo_se_redacta() {
        let kvs = vec![(
            "token".into(),
            make_secret(Value::Str("super-secret".into())),
        )];
        let line = format_json("warn", "auth call", &kvs);
        let parsed: JsonValue = serde_json::from_str(&line).unwrap();
        let token = parsed
            .as_object()
            .and_then(|o| o.get("token"))
            .and_then(|v| v.as_str())
            .expect("token field esperado");
        assert_eq!(token, "<redacted>");
        // El secret real NUNCA debe filtrarse en el output.
        assert!(
            !line.contains("super-secret"),
            "el secret se filtró: {}",
            line
        );
    }

    #[test]
    fn format_json_secret_adentro_de_list_se_redacta_recursivo() {
        let kvs = vec![(
            "tokens".into(),
            make_list(vec![
                Value::Str("first".into()),
                make_secret(Value::Str("hidden-token".into())),
                Value::Str("third".into()),
            ]),
        )];
        let line = format_json("info", "rotating", &kvs);
        // El secret no se debe filtrar ni siquiera adentro de la lista.
        assert!(!line.contains("hidden-token"), "filtración: {}", line);
        assert!(
            line.contains("<redacted>"),
            "esperaba redacted en: {}",
            line
        );
        assert!(line.contains("first"));
        assert!(line.contains("third"));
    }

    #[test]
    fn format_json_secret_adentro_de_map_se_redacta_recursivo() {
        let kvs = vec![(
            "config".into(),
            make_map(vec![
                (
                    Value::Str("db_url".into()),
                    Value::Str("postgres://...".into()),
                ),
                (
                    Value::Str("api_key".into()),
                    make_secret(Value::Str("sk-live-12345".into())),
                ),
            ]),
        )];
        let line = format_json("info", "starting", &kvs);
        assert!(!line.contains("sk-live-12345"), "filtración: {}", line);
        assert!(line.contains("<redacted>"));
        assert!(line.contains("postgres://..."));
    }

    #[test]
    fn format_json_orden_de_campos_estable_timestamp_level_msg_kwargs() {
        let kvs = vec![("a".into(), Value::Int(1)), ("b".into(), Value::Int(2))];
        let line = format_json("error", "fail", &kvs);
        // serde_json con preserve_order respeta orden de inserción.
        let ts_pos = line.find("\"timestamp\"").expect("timestamp");
        let level_pos = line.find("\"level\"").expect("level");
        let msg_pos = line.find("\"msg\"").expect("msg");
        let a_pos = line.find("\"a\"").expect("a");
        let b_pos = line.find("\"b\"").expect("b");
        assert!(ts_pos < level_pos);
        assert!(level_pos < msg_pos);
        assert!(msg_pos < a_pos);
        assert!(a_pos < b_pos);
    }

    #[test]
    fn format_pretty_contiene_level_uppercase_y_kwargs_inline() {
        let kvs = vec![
            ("user_id".into(), Value::Int(42)),
            ("active".into(), Value::Bool(true)),
        ];
        let line = format_pretty("info", "login ok", &kvs);
        // Strip ANSI para comparar el contenido textual.
        let stripped = strip_ansi(&line);
        assert!(stripped.contains("INFO"));
        assert!(stripped.contains("login ok"));
        assert!(stripped.contains("user_id=42"));
        assert!(stripped.contains("active=true"));
    }

    #[test]
    fn format_pretty_secret_se_redacta() {
        let kvs = vec![("token".into(), make_secret(Value::Str("secret-x".into())))];
        let line = format_pretty("warn", "auth", &kvs);
        let stripped = strip_ansi(&line);
        assert!(stripped.contains("token=<redacted>"));
        assert!(!stripped.contains("secret-x"));
    }

    #[test]
    fn format_pretty_strings_con_comillas_dobles() {
        let kvs = vec![("role".into(), Value::Str("admin".into()))];
        let line = format_pretty("info", "ok", &kvs);
        let stripped = strip_ansi(&line);
        assert!(stripped.contains("role=\"admin\""));
    }

    #[test]
    fn value_to_json_redacted_float_normal() {
        let v = Value::Float(std::f64::consts::PI);
        let json = value_to_json_redacted(&v);
        assert!(json.is_number());
    }

    #[test]
    fn value_to_json_redacted_float_nan_fallback_a_string() {
        let v = Value::Float(f64::NAN);
        let json = value_to_json_redacted(&v);
        assert!(json.is_string());
    }

    #[test]
    fn detect_format_respeta_override_env_json() {
        // Salvamos el valor previo para no contaminar otros tests.
        let prev = std::env::var("FITZ_LOG_FORMAT").ok();
        unsafe {
            std::env::set_var("FITZ_LOG_FORMAT", "json");
        }
        let fmt = detect_format();
        // Restaurar antes de assertear.
        match prev {
            Some(v) => unsafe { std::env::set_var("FITZ_LOG_FORMAT", v) },
            None => unsafe { std::env::remove_var("FITZ_LOG_FORMAT") },
        }
        assert_eq!(fmt, LogFormat::Json);
    }

    #[test]
    fn detect_format_respeta_override_env_pretty() {
        let prev = std::env::var("FITZ_LOG_FORMAT").ok();
        unsafe {
            std::env::set_var("FITZ_LOG_FORMAT", "pretty");
        }
        let fmt = detect_format();
        match prev {
            Some(v) => unsafe { std::env::set_var("FITZ_LOG_FORMAT", v) },
            None => unsafe { std::env::remove_var("FITZ_LOG_FORMAT") },
        }
        assert_eq!(fmt, LogFormat::Pretty);
    }

    #[test]
    fn detect_format_override_invalido_cae_a_auto_detect() {
        let prev = std::env::var("FITZ_LOG_FORMAT").ok();
        unsafe {
            std::env::set_var("FITZ_LOG_FORMAT", "yaml-no-existe");
        }
        let fmt = detect_format();
        match prev {
            Some(v) => unsafe { std::env::set_var("FITZ_LOG_FORMAT", v) },
            None => unsafe { std::env::remove_var("FITZ_LOG_FORMAT") },
        }
        // Auto-detect: depende del TTY del runner — en CI no es TTY,
        // así que esperamos Json. Si en local con TTY queda Pretty, OK
        // — la assertion es que NO crashea ni devuelve un format
        // inválido, no qué valor concreto da.
        assert!(matches!(fmt, LogFormat::Json | LogFormat::Pretty));
    }

    /// Pequeño stripper de secuencias ANSI escape para tests de format
    /// pretty. Maneja `\x1b[...m` (los códigos que usamos en
    /// `colorize_level` y el `\x1b[2m...\x1b[0m` del timestamp).
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next(); // skip `[`
                              // Consumir hasta `m` inclusive.
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn strip_ansi_test_helper_quita_secuencias() {
        // Self-check del helper: si la lógica anti-ANSI rompe, todos
        // los tests pretty pueden pasar falsos.
        let s = "\x1b[1;32mINFO\x1b[0m hola";
        assert_eq!(strip_ansi(s), "INFO hola");
    }

    // ---- Fase 12.3.b.1 — SpanContext + correlación trace_id en logs ----

    #[test]
    fn span_context_new_root_genera_ids_otel_compatible() {
        let ctx = SpanContext::new_root();
        // trace_id = 32 hex chars (16 bytes × 2).
        assert_eq!(ctx.trace_id.len(), 32);
        assert!(ctx.trace_id.chars().all(|c| c.is_ascii_hexdigit()));
        // span_id = 16 hex chars (8 bytes × 2).
        assert_eq!(ctx.span_id.len(), 16);
        assert!(ctx.span_id.chars().all(|c| c.is_ascii_hexdigit()));
        // Root no tiene parent.
        assert_eq!(ctx.parent_span_id, None);
    }

    #[test]
    fn span_context_new_root_genera_ids_distintos_entre_calls() {
        let a = SpanContext::new_root();
        let b = SpanContext::new_root();
        assert_ne!(a.trace_id, b.trace_id, "trace_id debería ser único");
        assert_ne!(a.span_id, b.span_id, "span_id debería ser único");
    }

    #[test]
    fn span_context_new_child_hereda_trace_id_y_registra_parent() {
        let parent = SpanContext::new_root();
        let child = parent.new_child();
        assert_eq!(
            child.trace_id, parent.trace_id,
            "child hereda trace_id del parent"
        );
        assert_ne!(child.span_id, parent.span_id, "child tiene span_id nuevo");
        assert_eq!(
            child.parent_span_id,
            Some(parent.span_id.clone()),
            "child registra parent_span_id"
        );
    }

    #[test]
    fn span_context_grandchild_mantiene_trace_id_y_actualiza_parent() {
        let root = SpanContext::new_root();
        let child = root.new_child();
        let grandchild = child.new_child();
        // El trace_id es estable across la cadena entera.
        assert_eq!(grandchild.trace_id, root.trace_id);
        // El parent del grandchild es el child, NO el root.
        assert_eq!(grandchild.parent_span_id, Some(child.span_id.clone()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn current_span_context_devuelve_none_fuera_de_scope() {
        // Sin `with_span_context(...)`, el TaskLocal no está set.
        assert!(current_span_context().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn current_span_context_devuelve_some_adentro_de_with_span_context() {
        let ctx = SpanContext::new_root();
        let trace_id_expected = ctx.trace_id.clone();
        let span_id_expected = ctx.span_id.clone();
        with_span_context(ctx, || async move {
            let observed = current_span_context().expect("debería estar set");
            assert_eq!(observed.trace_id, trace_id_expected);
            assert_eq!(observed.span_id, span_id_expected);
        })
        .await;
        // Después del scope, el TaskLocal queda fuera de scope.
        assert!(current_span_context().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_json_sin_span_omite_trace_id_y_span_id() {
        // Sin span activo, el JSON NO incluye trace_id ni span_id.
        let line = format_json("info", "test", &[]);
        let parsed: JsonValue = serde_json::from_str(&line).unwrap();
        let obj = parsed.as_object().unwrap();
        assert!(obj.get("trace_id").is_none(), "trace_id no debería estar");
        assert!(obj.get("span_id").is_none(), "span_id no debería estar");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_json_con_span_activo_incluye_trace_id_y_span_id() {
        let ctx = SpanContext::new_root();
        let trace_id_expected = ctx.trace_id.clone();
        let span_id_expected = ctx.span_id.clone();
        with_span_context(ctx, || async move {
            let line = format_json("info", "login ok", &[("user_id".into(), Value::Int(42))]);
            let parsed: JsonValue = serde_json::from_str(&line).unwrap();
            let obj = parsed.as_object().unwrap();
            assert_eq!(
                obj.get("trace_id").and_then(|v| v.as_str()),
                Some(trace_id_expected.as_str())
            );
            assert_eq!(
                obj.get("span_id").and_then(|v| v.as_str()),
                Some(span_id_expected.as_str())
            );
            // Los kwargs del user siguen presentes.
            assert_eq!(obj.get("user_id"), Some(&JsonValue::Number(42.into())));
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_json_orden_es_timestamp_level_msg_trace_span_kwargs() {
        // El orden es importante para queryability — trace_id/span_id
        // van entre msg y kwargs para que herramientas downstream los
        // encuentren con un pattern fijo.
        let ctx = SpanContext::new_root();
        with_span_context(ctx, || async move {
            let line = format_json("info", "x", &[("user_id".into(), Value::Int(1))]);
            let ts_pos = line.find("\"timestamp\"").unwrap();
            let level_pos = line.find("\"level\"").unwrap();
            let msg_pos = line.find("\"msg\"").unwrap();
            let trace_pos = line.find("\"trace_id\"").unwrap();
            let span_pos = line.find("\"span_id\"").unwrap();
            let user_pos = line.find("\"user_id\"").unwrap();
            assert!(ts_pos < level_pos);
            assert!(level_pos < msg_pos);
            assert!(msg_pos < trace_pos);
            assert!(trace_pos < span_pos);
            assert!(span_pos < user_pos);
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_pretty_sin_span_no_emite_trace_part() {
        let line = format_pretty("info", "test", &[]);
        let stripped = strip_ansi(&line);
        assert!(
            !stripped.contains("trace="),
            "trace= no debería estar sin span"
        );
        assert!(
            !stripped.contains("span="),
            "span= no debería estar sin span"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_pretty_con_span_activo_emite_trace_y_span_dim() {
        let ctx = SpanContext::new_root();
        let trace_id_expected = ctx.trace_id.clone();
        let span_id_expected = ctx.span_id.clone();
        with_span_context(ctx, || async move {
            let line = format_pretty("info", "login", &[]);
            let stripped = strip_ansi(&line);
            assert!(
                stripped.contains(&format!("trace={}", trace_id_expected)),
                "esperaba trace=<id> en pretty: {}",
                stripped
            );
            assert!(
                stripped.contains(&format!("span={}", span_id_expected)),
                "esperaba span=<id> en pretty: {}",
                stripped
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_pretty_con_span_e_inclusion_de_kwargs() {
        // El orden visual: ts LEVEL msg trace= span= k=v
        // Validamos que ambos están presentes y que los kwargs van
        // después del span info.
        let ctx = SpanContext::new_root();
        with_span_context(ctx, || async move {
            let line = format_pretty("info", "msg", &[("user_id".into(), Value::Int(42))]);
            let stripped = strip_ansi(&line);
            let trace_pos = stripped.find("trace=").expect("trace= debería estar");
            let user_pos = stripped.find("user_id=").expect("user_id= debería estar");
            assert!(
                trace_pos < user_pos,
                "trace= debería ir antes que kwargs del user: {}",
                stripped
            );
        })
        .await;
    }
}
