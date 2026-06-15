//! Phase 12.3.a.2 — Structured logging built-in (real JSON output with
//! tracing-subscriber + TTY detection + Secret redaction).
//!
//! Real sink implementation for the 4 builtins (`log.info`/`log.warn`/
//! `log.error`/`log.debug`) registered in the evaluator in 12.3.a.1. The
//! module exposes `emit_log_record(level, msg, kvs)` which:
//!
//! 1. Goes through the level gate (`tracing::enabled!(target: "fitz::log", L)`
//!    against the `EnvFilter` installed at boot via `init_logging()`).
//!    Default level = `INFO` if `RUST_LOG` is not set.
//! 2. Determines the format (`Json` vs `Pretty`):
//!    - Explicit override via `FITZ_LOG_FORMAT=json|pretty`.
//!    - Auto-detect: stderr is TTY → `Pretty`; otherwise → `Json`
//!      (containers/CI/redirection).
//! 3. Emits the record to stderr (does not contaminate the Fitz
//!    program's stdout, where the user's `print(...)` calls go).
//! 4. Automatically redacts `Value::Secret(_)` as `"<redacted>"` — in
//!    direct kwargs and inside List/Map (preview of 12.3.c).
//!
//! Hybrid approach with tracing (decision 12.3.a.2): the level filter
//! is delegated to `tracing` (via `EnvFilter` + `tracing::enabled!`),
//! but Fitz emits the JSON output manually with `serde_json` because
//! the heterogeneous runtime kwargs are not cleanly modeled with the
//! `event!` macros that expect field names at compile-time. The
//! subscriber is installed anyway for 12.3.b (HTTP auto-trace + spans +
//! `trace_id` correlation in logs).
//!
//! Does not replace the evaluator's `emit_log_record` helper — it
//! re-exports it. The evaluator only imports this public fn and calls it.

use std::io::{IsTerminal, Stderr, Write};
use std::sync::{Mutex, OnceLock};

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::value::Value;

/// Global stderr sink. `Mutex` because multiple threads can emit
/// concurrently (HTTP handler + cron + background) — we need atomic
/// per-record writes so lines don't interleave.
///
/// `OnceLock` for lazy thread-safe initialization: the first call to
/// `emit_log_record` (or to `init_logging`) constructs it.
static STDERR_LOCK: OnceLock<Mutex<Stderr>> = OnceLock::new();

fn stderr_lock() -> &'static Mutex<Stderr> {
    STDERR_LOCK.get_or_init(|| Mutex::new(std::io::stderr()))
}

/// Output format. The default choice depends on TTY detection +
/// `FITZ_LOG_FORMAT` override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Flat JSON — one JSON object per line with `timestamp`, `level`,
    /// `msg` and the kwargs at the same level. Default when stderr is
    /// NOT a TTY (containers/CI/redirection).
    Json,
    /// Pretty — `<ts> <LEVEL> <msg> k1=v1 k2=v2` with per-level ANSI
    /// colors. Default when stderr is a TTY (local dev).
    Pretty,
}

/// Detects the format to use based on TTY + `FITZ_LOG_FORMAT`. Override
/// wins over auto-detect; an invalid value silently falls back to
/// auto-detect (we don't want to abort the program over a misconfigured
/// env var).
pub fn detect_format() -> LogFormat {
    if let Ok(v) = std::env::var("FITZ_LOG_FORMAT") {
        match v.to_lowercase().as_str() {
            "json" => return LogFormat::Json,
            "pretty" => return LogFormat::Pretty,
            // Unknown value — silent fallback to auto-detect. We don't
            // print a warning because the warning's own sink would
            // depend on this var and would bootstrap badly.
            _ => {}
        }
    }
    if std::io::stderr().is_terminal() {
        LogFormat::Pretty
    } else {
        LogFormat::Json
    }
}

/// Initializes the `tracing` subscriber at boot of the `fitz` binary.
/// Called once from `main.rs` BEFORE executing the user program.
/// Idempotent: if already initialized, no-op.
///
/// Reason for installing the subscriber even though we emit the JSON
/// output manually: so that `tracing::enabled!(target: "fitz::log", LEVEL)`
/// respects `RUST_LOG`. `EnvFilter::try_from_default_env()` reads the
/// env var; if it's not set, defaults to `info` (more verbose than the
/// crate's standard default, which is `error` — for Fitz, `info` is
/// more useful).
///
/// The installed layer is no-op (`with_writer(std::io::sink)`) — it
/// emits nothing. It's only there to satisfy the
/// `tracing_subscriber::registry()` API and keep the filter active. In
/// 12.3.b we add layers that emit HTTP spans (auto-trace).
pub fn init_logging() {
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter};

    // Idempotent: if the global subscriber is already set,
    // set_global_default fails — we ignore it.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let noop_layer = fmt::layer().with_writer(std::io::sink);

    // try_init fails if there is already a subscriber installed —
    // perfect for idempotency (tests, REPL, fitz dev re-executing, etc.).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(noop_layer)
        .try_init();
}

/// Public API of the module. Called by the evaluator from
/// `dispatch_builtin_kwargs` (kwargs path) and from
/// `builtin_log_<level>` (positional-only path). Implements the level
/// gate + format detection + emit to stderr.
///
/// `level_str` is one of `"info"`/`"warn"`/`"error"`/`"debug"`
/// (lowercase, comes from the Fitz dispatch). Internally mapped to the
/// corresponding `tracing::Level` for the gate.
pub fn emit_log_record(level_str: &str, msg: &str, kvs: &[(String, Value)]) {
    use tracing::Level;
    let level = match level_str {
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        "debug" => Level::DEBUG,
        // Defensive: unknown level, we let it through as INFO. The
        // evaluator's dispatch already validates the method name.
        _ => Level::INFO,
    };
    // Level gate via tracing — respects RUST_LOG.
    // tracing::enabled! is a macro: the level must be const, so we
    // dispatch with a manual match.
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

    // Atomic lock + write + flush per line. We ignore write errors
    // (stderr closed, broken pipe) — the log sink must not tear the
    // program down.
    if let Ok(mut stderr) = stderr_lock().lock() {
        let _ = writeln!(stderr, "{}", line);
        let _ = stderr.flush();
    }

    // Phase 12.3.iter2.b — parallel emit to the OTel backend when the
    // provider is installed. Without OTel active, `logger_provider()`
    // returns `None` and this block is no-op (zero overhead). The logs
    // in stderr remain intact — the bridge is ADDITIVE, not a
    // replacement. The SDK's batch processor accumulates records and
    // sends them async via OTLP HTTP/proto to the `/v1/logs` endpoint.
    if crate::observability::is_otel_enabled() {
        emit_otel_log_record(level_str, msg, kvs);
    }
}

/// Phase 12.3.iter2.b — emits the log record to the global OTel
/// provider when it is installed. Called from `emit_log_record` after
/// the stderr write (ADDITIVE emit, not a replacement).
///
/// Exported LogRecord structure:
/// - `severity_number` + `severity_text`: 1:1 mapping from `level_str`
///   (`info`/`warn`/`error`/`debug` → `Severity::Info`/etc + uppercase
///   text).
/// - `body`: `AnyValue::String(msg)` — the raw message, same as stderr.
/// - `observed_timestamp`: `SystemTime::now()` — the SDK uses it when
///   the `timestamp` source is not set (typical case).
/// - `trace_context`: derived from the active `SpanContext` via
///   `current_span_context()`. When OTel is active, the SpanContext is
///   already synchronized with the OTel span (iter2.a closure), so
///   this produces automatic trace_id/span_id correlation between logs
///   and spans in the backend (Jaeger/Tempo/Datadog).
/// - `attributes`: each `(k, v)` from the Fitz kwargs → `Key` +
///   `AnyValue`. Complex values (List/Map/Instance) go as structured
///   `ListAny`/`Map` (preserves the shape). `Secret` values go as
///   `"***"` (redaction consistent with the stderr output).
fn emit_otel_log_record(level_str: &str, msg: &str, kvs: &[(String, Value)]) {
    use opentelemetry::logs::{AnyValue, LogRecord as _, Logger, LoggerProvider, Severity};
    use opentelemetry::trace::{SpanId, TraceId};
    use opentelemetry::Key;
    use std::time::SystemTime;

    let Some(provider) = crate::observability::logger_provider() else {
        return;
    };
    let logger = provider.logger("fitz");
    let mut record = logger.create_log_record();

    let (severity_number, severity_text) = match level_str {
        "error" => (Severity::Error, "ERROR"),
        "warn" => (Severity::Warn, "WARN"),
        "info" => (Severity::Info, "INFO"),
        "debug" => (Severity::Debug, "DEBUG"),
        _ => (Severity::Info, "INFO"),
    };
    record.set_severity_number(severity_number);
    record.set_severity_text(severity_text);
    record.set_body(AnyValue::String(msg.to_string().into()));
    record.set_observed_timestamp(SystemTime::now());

    // Trace context derived from the active SpanContext. The iter2.a
    // closure guarantees that the IDs are already those of the OTel
    // span when OTel is active — this produces automatic
    // logs↔spans correlation in the backend.
    if let Some(ctx) = current_span_context() {
        if let (Ok(trace_id), Ok(span_id)) = (
            TraceId::from_hex(&ctx.trace_id),
            SpanId::from_hex(&ctx.span_id),
        ) {
            record.set_trace_context(trace_id, span_id, None);
        }
    }

    for (k, v) in kvs {
        if let Some(av) = value_to_any_value(v) {
            record.add_attribute(Key::new(k.clone()), av);
        }
    }

    logger.emit(record);
}

/// Phase 12.3.iter2.b — converts a Fitz `Value` to an OTel `AnyValue`
/// for the LogRecord. Recursive for List/Map; primitives go through
/// directly. `Secret` is redacted to `"***"` (consistent with the
/// stderr output).
fn value_to_any_value(v: &Value) -> Option<opentelemetry::logs::AnyValue> {
    use opentelemetry::logs::AnyValue;
    use opentelemetry::Key;
    use std::collections::HashMap;
    match v {
        Value::Int(n) => Some(AnyValue::Int(*n)),
        Value::Float(f) => Some(AnyValue::Double(*f)),
        Value::Str(s) => Some(AnyValue::String(s.clone().into())),
        Value::Bool(b) => Some(AnyValue::Boolean(*b)),
        Value::Null => Some(AnyValue::String("null".to_string().into())),
        Value::Secret(_) => Some(AnyValue::String("***".to_string().into())),
        Value::List(items) => {
            let arr: Vec<AnyValue> = items.lock().iter().filter_map(value_to_any_value).collect();
            Some(AnyValue::ListAny(Box::new(arr)))
        }
        Value::Map(pairs) => {
            let mut map: HashMap<Key, AnyValue> = HashMap::new();
            for (k, v) in pairs.lock().iter() {
                let key_str = match k {
                    Value::Str(s) => s.clone(),
                    Value::Int(n) => n.to_string(),
                    _ => continue,
                };
                if let Some(av) = value_to_any_value(v) {
                    map.insert(Key::new(key_str), av);
                }
            }
            Some(AnyValue::Map(Box::new(map)))
        }
        // Non-trivial types (Instance, Function, Future, etc.) —
        // fallback to Fitz's Display so as not to lose info. The OTel
        // backend receives it as a serialized string.
        _ => Some(AnyValue::String(format!("{}", v).into())),
    }
}

/// Builds the flat JSON: `{"timestamp": "...", "level": "INFO",
/// "msg": "...", "trace_id": "...", "span_id": "...", <kwargs>}`.
/// Reserved names (`level`/`msg`/`timestamp`/`trace_id`/`span_id`)
/// were already rejected in the evaluator — no risk of collision.
///
/// Phase 12.3.b.1 — `trace_id`/`span_id` are inserted automatically if
/// there is an active span context via `with_span_context(...)`.
/// Without an active span, those fields are omitted (we don't emit
/// them as `null` so as not to pollute the shape).
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
    // `serde_json::to_string` over `JsonValue::Object` — preserve_order
    // is enabled in our dep (feature `preserve_order`), so the shape
    // comes out in insertion order: timestamp, level, msg, kwargs.
    serde_json::to_string(&JsonValue::Object(obj)).unwrap_or_else(|_| {
        format!(
            "{{\"level\":\"{}\",\"msg\":\"<serialize_error>\"}}",
            level_str.to_uppercase()
        )
    })
}

/// Builds the pretty line: `<ts> <LEVEL> <msg> trace=xxx span=yyy
/// k=v k="str" k=null` with per-level ANSI colors. Color detection
/// uses the same TTY as detect_format — if we got here it's because
/// format == Pretty, which implies TTY or explicit pretty override.
///
/// Phase 12.3.b.1 — `trace=xxx span=yyy` is injected automatically if
/// there is an active span context via `with_span_context(...)`.
/// Without an active span, the fields are omitted (parallel to JSON).
/// Dim form so it doesn't visually dominate over the user's msg +
/// kwargs.
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
    // ANSI dim over the timestamp so the eye goes to the level + msg
    // first. No colors if stdout doesn't support them — detect_format
    // already guarantees Pretty only on TTY or explicit override.
    format!(
        "\x1b[2m{}\x1b[0m {} {}{}{}",
        ts, level_colored, msg, span_part, kvs_part
    )
}

/// Per-level ANSI color. Convention from bunyan/pino/uvicorn:
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

/// ISO 8601 / RFC 3339 timestamp with milliseconds in UTC. Example:
/// `2026-06-02T14:23:01.123Z`. Compatible with Loki/Datadog queries
/// and with most log timestamp parsers.
fn now_rfc3339() -> String {
    use chrono::SecondsFormat;
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Converts a Fitz `Value` to `serde_json::Value` for JSON output with
/// **recursive redaction** of `Value::Secret`. Logger-specific version
/// (we don't use `http::value_to_json` because that one rejects Secret
/// with an error — for logs we want silent auto-redaction).
fn value_to_json_redacted(v: &Value) -> JsonValue {
    match v {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::Int(n) => JsonValue::Number((*n).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            // NaN/Infinity are not JSON-valid — we emit as string.
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
        // Rest: the evaluator already rejected non-serializable types
        // in the dispatch (Function/Type/Module/DbConn/etc). If
        // something odd reaches here it's a bug — we emit `null`
        // defensively instead of panicking the log sink.
        _ => JsonValue::Null,
    }
}

/// Pretty format of a value for inline `k=v`. Strings with double
/// quotes (consistent with `print(...)` inside containers), Secret
/// redacted, List/Map with compact JSON-like shape.
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
// Phase 12.3.b.1 — SpanContext + trace_id/span_id correlation in logs.
//
// Span context infrastructure for correlating logs emitted inside the
// same HTTP request (or any scope that wraps them). When there is an
// active span via `with_span_context(ctx, fut)`, all the
// `log.info/warn/error/debug` emitted inside automatically add
// `trace_id` (32 hex chars, shared by the whole chain) and `span_id`
// (16 hex chars, unique per span) to the JSON / pretty output.
//
// Approach: own storage with `tokio::task_local!` (vs. native tracing
// `Span::extensions`). Reasons:
// - Simplicity — no custom tracing Subscriber/Layer.
// - Full control over the SpanContext shape (OTel-compatible without
//   forcing the tracing hierarchy).
// - `task_local!` crosses thread boundaries of the multi-thread tokio
//   runtime (HTTP handlers jump workers).
//
// In 12.3.b.2 comes the HTTP wrapper that opens the span automatically
// before invoking the handler; in 12.3.c we migrate to the
// OpenTelemetry SpanContext without changing the public API.
//
// OTel-compatible IDs:
// - trace_id = 32 hex chars (16 random bytes, no hyphens).
// - span_id = 16 hex chars (8 random bytes, no hyphens).
// Generated with `uuid::Uuid::new_v4()` (already in non-optional deps).

/// Active span context for correlating logs. Immutable — new spans
/// are built with `new_child()` which clones the `trace_id` and
/// generates a new `span_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanContext {
    /// 32 hex chars. Constant throughout the whole chain of nested
    /// spans (an entire HTTP request shares the same trace_id).
    pub trace_id: String,
    /// 16 hex chars. Unique per span — each `new_child()` generates a
    /// new one.
    pub span_id: String,
    /// `span_id` of the parent if this is a nested span. `None` for
    /// root spans (typically that of the initial HTTP handler).
    pub parent_span_id: Option<String>,
}

impl SpanContext {
    /// Creates a root span (no parent). Generates a fresh `trace_id`
    /// and `span_id`. Typically called by the HTTP wrapper upon
    /// receiving a request (12.3.b.2).
    pub fn new_root() -> Self {
        Self {
            trace_id: generate_trace_id(),
            span_id: generate_span_id(),
            parent_span_id: None,
        }
    }

    /// Creates a child span of the current one. Shares the
    /// `trace_id`, generates a new `span_id`, records the parent's
    /// `span_id`. Used for nested spans (e.g. `db.query` inside an
    /// HTTP handler).
    pub fn new_child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: generate_span_id(),
            parent_span_id: Some(self.span_id.clone()),
        }
    }

    /// Builds a `SpanContext` with the provided `trace_id` and
    /// `span_id`. Typically called when there is an active OTel span
    /// and we want our `trace_id`/`span_id` (emitted in stderr/JSON
    /// logs) to match exactly those of the OTel span — enables
    /// **cross-pipeline queries**: the `trace_id` that the user sees
    /// in Jaeger/Tempo/Datadog/Honeycomb is THE SAME one that appears
    /// in the request's stderr/Loki logs.
    ///
    /// Phase 12.3.iter2.a — closes the debt "Fitz↔OTel trace_id
    /// correlation" derived from Phase 12.3.
    pub fn with_ids(trace_id: String, span_id: String) -> Self {
        Self {
            trace_id,
            span_id,
            parent_span_id: None,
        }
    }
}

/// Generates a trace_id = 32 hex chars (16 random bytes).
/// OTel-compatible format: downstream chains (Datadog/Jaeger/Tempo)
/// accept it as-is without transformation.
fn generate_trace_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    // hex of 16 bytes = 32 chars
    let mut s = String::with_capacity(32);
    for b in bytes.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Generates a span_id = 16 hex chars (8 random bytes). We take the
/// first 8 bytes of a Uuid v4 — enough entropy for practical
/// uniqueness (2^64 space).
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
    /// Active span context of the current task. Set by
    /// `with_span_context(ctx, fut)`. Read by `current_span_context()`.
    /// Without a set value, queries return `None`.
    static SPAN_CONTEXT: SpanContext;
}

/// Returns the active SpanContext of the current tokio task, if one
/// is installed. Called by `emit_log_record` to inject trace_id/
/// span_id into the output. Without an active span → `None`.
pub fn current_span_context() -> Option<SpanContext> {
    SPAN_CONTEXT.try_with(|ctx| ctx.clone()).ok()
}

/// Executes the future `fut` with `ctx` as the active span context.
/// Wrapper over `LocalKey::scope`. Typically called by the HTTP
/// wrapper at the request boot (12.3.b.2).
///
/// Inside the scope, `current_span_context()` returns `Some(ctx)`.
/// Nested spans are created with `ctx.new_child()` + a new call to
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

    /// Helper: builds a `Value::Map` with `Vec<(Value, Value)>` inside
    /// `Arc<parking_lot::Mutex<...>>` (post-F17 shape).
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
    fn format_json_shape_flat_with_basic_kwargs() {
        let kvs = vec![
            ("user_id".into(), Value::Int(42)),
            ("role".into(), Value::Str("admin".into())),
            ("active".into(), Value::Bool(true)),
        ];
        let line = format_json("info", "login ok", &kvs);
        // We parse so we don't depend on the exact whitespace.
        let parsed: JsonValue = serde_json::from_str(&line).expect("should be valid JSON");
        let obj = parsed.as_object().expect("Object expected");
        assert_eq!(obj.get("level"), Some(&JsonValue::String("INFO".into())));
        assert_eq!(obj.get("msg"), Some(&JsonValue::String("login ok".into())));
        assert_eq!(obj.get("user_id"), Some(&JsonValue::Number(42.into())));
        assert_eq!(obj.get("role"), Some(&JsonValue::String("admin".into())));
        assert_eq!(obj.get("active"), Some(&JsonValue::Bool(true)));
        // timestamp is ISO 8601 ending in `Z`.
        let ts = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .expect("timestamp string esperado");
        assert!(ts.ends_with('Z'), "timestamp should end in Z, was {}", ts);
    }

    #[test]
    fn format_json_secret_direct_is_redacted() {
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
        // The real secret must NEVER leak into the output.
        assert!(
            !line.contains("super-secret"),
            "the secret leaked: {}",
            line
        );
    }

    #[test]
    fn format_json_secret_inside_list_is_redacted_recursive() {
        let kvs = vec![(
            "tokens".into(),
            make_list(vec![
                Value::Str("first".into()),
                make_secret(Value::Str("hidden-token".into())),
                Value::Str("third".into()),
            ]),
        )];
        let line = format_json("info", "rotating", &kvs);
        // The secret must not leak even from inside the list.
        assert!(!line.contains("hidden-token"), "leak: {}", line);
        assert!(
            line.contains("<redacted>"),
            "esperaba redacted en: {}",
            line
        );
        assert!(line.contains("first"));
        assert!(line.contains("third"));
    }

    #[test]
    fn format_json_secret_inside_map_is_redacted_recursive() {
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
        assert!(!line.contains("sk-live-12345"), "leak: {}", line);
        assert!(line.contains("<redacted>"));
        assert!(line.contains("postgres://..."));
    }

    #[test]
    fn format_json_field_order_stable_timestamp_level_msg_kwargs() {
        let kvs = vec![("a".into(), Value::Int(1)), ("b".into(), Value::Int(2))];
        let line = format_json("error", "fail", &kvs);
        // serde_json with preserve_order respects insertion order.
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
    fn format_pretty_contains_level_uppercase_and_kwargs_inline() {
        let kvs = vec![
            ("user_id".into(), Value::Int(42)),
            ("active".into(), Value::Bool(true)),
        ];
        let line = format_pretty("info", "login ok", &kvs);
        // Strip ANSI to compare textual content.
        let stripped = strip_ansi(&line);
        assert!(stripped.contains("INFO"));
        assert!(stripped.contains("login ok"));
        assert!(stripped.contains("user_id=42"));
        assert!(stripped.contains("active=true"));
    }

    #[test]
    fn format_pretty_secret_is_redacted() {
        let kvs = vec![("token".into(), make_secret(Value::Str("secret-x".into())))];
        let line = format_pretty("warn", "auth", &kvs);
        let stripped = strip_ansi(&line);
        assert!(stripped.contains("token=<redacted>"));
        assert!(!stripped.contains("secret-x"));
    }

    #[test]
    fn format_pretty_strings_with_double_quotes() {
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
    fn value_to_json_redacted_float_nan_fallback_to_string() {
        let v = Value::Float(f64::NAN);
        let json = value_to_json_redacted(&v);
        assert!(json.is_string());
    }

    #[test]
    fn detect_format_respects_env_override_json() {
        // Save the previous value so we don't contaminate other tests.
        let prev = std::env::var("FITZ_LOG_FORMAT").ok();
        unsafe {
            std::env::set_var("FITZ_LOG_FORMAT", "json");
        }
        let fmt = detect_format();
        // Restore before asserting.
        match prev {
            Some(v) => unsafe { std::env::set_var("FITZ_LOG_FORMAT", v) },
            None => unsafe { std::env::remove_var("FITZ_LOG_FORMAT") },
        }
        assert_eq!(fmt, LogFormat::Json);
    }

    #[test]
    fn detect_format_respects_env_override_pretty() {
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
    fn detect_format_invalid_override_falls_back_to_auto_detect() {
        let prev = std::env::var("FITZ_LOG_FORMAT").ok();
        unsafe {
            std::env::set_var("FITZ_LOG_FORMAT", "yaml-no-existe");
        }
        let fmt = detect_format();
        match prev {
            Some(v) => unsafe { std::env::set_var("FITZ_LOG_FORMAT", v) },
            None => unsafe { std::env::remove_var("FITZ_LOG_FORMAT") },
        }
        // Auto-detect: depends on the runner's TTY — in CI it's not a
        // TTY, so we expect Json. If running locally with a TTY it
        // comes out Pretty, OK — the assertion is that it does NOT
        // crash nor return an invalid format, not what concrete value
        // it returns.
        assert!(matches!(fmt, LogFormat::Json | LogFormat::Pretty));
    }

    /// Small ANSI escape sequence stripper for pretty format tests.
    /// Handles `\x1b[...m` (the codes we use in `colorize_level` and
    /// the timestamp's `\x1b[2m...\x1b[0m`).
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next(); // skip `[`
                              // Consume up to and including `m`.
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
    fn strip_ansi_test_helper_removes_sequences() {
        // Self-check of the helper: if the anti-ANSI logic breaks,
        // all the pretty tests can pass falsely.
        let s = "\x1b[1;32mINFO\x1b[0m hola";
        assert_eq!(strip_ansi(s), "INFO hola");
    }

    // ---- Phase 12.3.b.1 — SpanContext + trace_id correlation in logs ----

    #[test]
    fn span_context_new_root_generates_otel_compatible_ids() {
        let ctx = SpanContext::new_root();
        // trace_id = 32 hex chars (16 bytes × 2).
        assert_eq!(ctx.trace_id.len(), 32);
        assert!(ctx.trace_id.chars().all(|c| c.is_ascii_hexdigit()));
        // span_id = 16 hex chars (8 bytes × 2).
        assert_eq!(ctx.span_id.len(), 16);
        assert!(ctx.span_id.chars().all(|c| c.is_ascii_hexdigit()));
        // Root has no parent.
        assert_eq!(ctx.parent_span_id, None);
    }

    #[test]
    fn span_context_new_root_generates_distinct_ids_between_calls() {
        let a = SpanContext::new_root();
        let b = SpanContext::new_root();
        assert_ne!(a.trace_id, b.trace_id, "trace_id should be unique");
        assert_ne!(a.span_id, b.span_id, "span_id should be unique");
    }

    #[test]
    fn span_context_new_child_inherits_trace_id_and_registers_parent() {
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
    fn span_context_grandchild_keeps_trace_id_and_updates_parent() {
        let root = SpanContext::new_root();
        let child = root.new_child();
        let grandchild = child.new_child();
        // The trace_id is stable across the entire chain.
        assert_eq!(grandchild.trace_id, root.trace_id);
        // The grandchild's parent is the child, NOT the root.
        assert_eq!(grandchild.parent_span_id, Some(child.span_id.clone()));
    }

    #[test]
    fn iter2b_value_to_any_value_primitives_map_direct() {
        // Phase 12.3.iter2.b — `value_to_any_value` converts a Fitz
        // Value to an OTel AnyValue for the LogRecord. Primitives go
        // through directly.
        use opentelemetry::logs::AnyValue;
        // Int → Int
        match value_to_any_value(&Value::Int(42)) {
            Some(AnyValue::Int(n)) => assert_eq!(n, 42),
            other => panic!("esperaba AnyValue::Int(42), fue {:?}", other),
        }
        // Float → Double
        match value_to_any_value(&Value::Float(2.5)) {
            Some(AnyValue::Double(f)) => assert!((f - 2.5).abs() < 1e-9),
            other => panic!("esperaba AnyValue::Double(2.5), fue {:?}", other),
        }
        // Str → String
        match value_to_any_value(&Value::Str("hola".into())) {
            Some(AnyValue::String(s)) => assert_eq!(s.as_str(), "hola"),
            other => panic!("esperaba AnyValue::String(\"hola\"), fue {:?}", other),
        }
        // Bool → Boolean
        match value_to_any_value(&Value::Bool(true)) {
            Some(AnyValue::Boolean(b)) => assert!(b),
            other => panic!("esperaba AnyValue::Boolean(true), fue {:?}", other),
        }
        // Null → String "null"
        match value_to_any_value(&Value::Null) {
            Some(AnyValue::String(s)) => assert_eq!(s.as_str(), "null"),
            other => panic!("esperaba AnyValue::String(\"null\"), fue {:?}", other),
        }
    }

    #[test]
    fn iter2b_value_to_any_value_secret_is_redacted() {
        // Phase 12.3.iter2.b — Secret is exported as `"***"` to the
        // OTel backend, parallel to the Phase 12.3.a redaction in
        // stderr.
        use opentelemetry::logs::AnyValue;
        let secret = make_secret(Value::Str("super-secret-token".into()));
        match value_to_any_value(&secret) {
            Some(AnyValue::String(s)) => {
                assert_eq!(s.as_str(), "***");
                assert!(
                    !s.as_str().contains("super-secret"),
                    "el inner del Secret NO debe aparecer en el AnyValue: {:?}",
                    s
                );
            }
            other => panic!("esperaba AnyValue::String(\"***\"), fue {:?}", other),
        }
    }

    #[test]
    fn iter2b_value_to_any_value_list_and_map_are_recursive() {
        // Phase 12.3.iter2.b — structured List/Map go as ListAny/Map
        // preserving the shape (not as string).
        use opentelemetry::logs::AnyValue;
        let list = make_list(vec![Value::Int(1), Value::Int(2)]);
        match value_to_any_value(&list) {
            Some(AnyValue::ListAny(items)) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("esperaba AnyValue::ListAny, fue {:?}", other),
        }
        let map = make_map(vec![
            (Value::Str("k".into()), Value::Int(42)),
            (Value::Str("ok".into()), Value::Bool(true)),
        ]);
        match value_to_any_value(&map) {
            Some(AnyValue::Map(m)) => {
                assert_eq!(m.len(), 2);
            }
            other => panic!("esperaba AnyValue::Map, fue {:?}", other),
        }
    }

    #[test]
    fn iter2a_span_context_with_ids_preserves_passed_ids_and_parent_is_none() {
        // Phase 12.3.iter2.a — constructor for Fitz↔OTel trace_id
        // correlation. `dispatch_request` uses it to derive its own
        // SpanContext from the OTel span when the provider is
        // installed, so the trace_id in stderr logs matches the one
        // in the OTel backend (Jaeger/Tempo/Datadog).
        let ctx = SpanContext::with_ids(
            "0123456789abcdef0123456789abcdef".to_string(),
            "fedcba9876543210".to_string(),
        );
        assert_eq!(ctx.trace_id, "0123456789abcdef0123456789abcdef");
        assert_eq!(ctx.span_id, "fedcba9876543210");
        assert_eq!(ctx.parent_span_id, None, "with_ids construye root span");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn current_span_context_returns_none_out_of_scope() {
        // Without `with_span_context(...)`, the TaskLocal is not set.
        assert!(current_span_context().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn current_span_context_returns_some_inside_with_span_context() {
        let ctx = SpanContext::new_root();
        let trace_id_expected = ctx.trace_id.clone();
        let span_id_expected = ctx.span_id.clone();
        with_span_context(ctx, || async move {
            let observed = current_span_context().expect("should be set");
            assert_eq!(observed.trace_id, trace_id_expected);
            assert_eq!(observed.span_id, span_id_expected);
        })
        .await;
        // After the scope, the TaskLocal goes out of scope.
        assert!(current_span_context().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_json_without_span_omits_trace_id_and_span_id() {
        // Without an active span, the JSON does NOT include trace_id
        // or span_id.
        let line = format_json("info", "test", &[]);
        let parsed: JsonValue = serde_json::from_str(&line).unwrap();
        let obj = parsed.as_object().unwrap();
        assert!(
            obj.get("trace_id").is_none(),
            "trace_id should not be present"
        );
        assert!(
            obj.get("span_id").is_none(),
            "span_id should not be present"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_json_with_active_span_includes_trace_id_and_span_id() {
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
            // The user's kwargs are still present.
            assert_eq!(obj.get("user_id"), Some(&JsonValue::Number(42.into())));
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_json_order_is_timestamp_level_msg_trace_span_kwargs() {
        // The order matters for queryability — trace_id/span_id go
        // between msg and kwargs so that downstream tools find them
        // with a fixed pattern.
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
    async fn format_pretty_without_span_does_not_emit_trace_part() {
        let line = format_pretty("info", "test", &[]);
        let stripped = strip_ansi(&line);
        assert!(
            !stripped.contains("trace="),
            "trace= should not be present without span"
        );
        assert!(
            !stripped.contains("span="),
            "span= should not be present without span"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn format_pretty_with_active_span_emits_trace_and_span_dim() {
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
    async fn format_pretty_with_span_and_kwargs_inclusion() {
        // Visual order: ts LEVEL msg trace= span= k=v
        // We validate that both are present and that the kwargs go
        // after the span info.
        let ctx = SpanContext::new_root();
        with_span_context(ctx, || async move {
            let line = format_pretty("info", "msg", &[("user_id".into(), Value::Int(42))]);
            let stripped = strip_ansi(&line);
            let trace_pos = stripped.find("trace=").expect("trace= should be present");
            let user_pos = stripped
                .find("user_id=")
                .expect("user_id= should be present");
            assert!(
                trace_pos < user_pos,
                "trace= should come before user kwargs: {}",
                stripped
            );
        })
        .await;
    }
}
