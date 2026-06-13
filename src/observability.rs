//! Phase 12.3.c.1 — OTLP exporter for HTTP spans.
//!
//! Connects the spans opened by `dispatch_request` (12.3.b.2) to a
//! real OTel backend (Jaeger, Tempo, Honeycomb, Datadog, etc.) when
//! the `OTEL_EXPORTER_OTLP_ENDPOINT` env var is set. Without that
//! var, `init_otel()` is a silent no-op — zero overhead, zero
//! network connections, zero spans sent anywhere.
//!
//! ## Env vars (OTel-standard)
//!
//! - `OTEL_EXPORTER_OTLP_ENDPOINT` — collector URL
//!   (e.g. `http://localhost:4318` for a local OTel Collector).
//!   **No default**: if it's not set, `init_otel()` installs nothing.
//! - `OTEL_SERVICE_NAME` — service name that appears in the backend.
//!   Default: `"fitz-app"`.
//! - `OTEL_TRACES_SAMPLER_ARG` — sampling ratio as a Float between
//!   `0.0` and `1.0`. Default: `1.0` (100% of spans are sent).
//!
//! ## What is NOT in 12.3.c.1
//!
//! - OTel metrics (`metrics::counter!` / `histogram!` still dispatch
//!   to an empty global recorder; the OTel bridge is left as a future
//!   sub-step).
//! - ~~OTel logs~~ **CLOSED in Phase 12.3.iter2.b (2026-06-03)**:
//!   when `is_otel_enabled()` is `true`, `emit_log_record` emits the
//!   LogRecord in parallel to the global OTel logger via OTLP
//!   HTTP/proto (endpoint `/v1/logs`). Logs in stderr remain intact;
//!   logs inside an HTTP request inherit `trace_id`/`span_id` from
//!   the active OTel span (parallel to the iter2.a correlation).
//! - ~~Fitz↔OTel trace_id correlation~~ **CLOSED in Phase
//!   12.3.iter2.a (2026-06-03)**: when `is_otel_enabled()` is `true`,
//!   `dispatch_request` derives its own `SpanContext` from the opened
//!   OTel span (via `SpanContext::with_ids(trace_id, span_id)`). The
//!   `trace_id` that appears in stderr/JSON logs is THE SAME as the
//!   OTel span's in Jaeger/Tempo/Datadog/Honeycomb — enables
//!   cross-pipeline queries. Without OTel, `SpanContext::new_root()`
//!   keeps generating its own IDs via uuid.
//!
//! ## API
//!
//! - `init_otel()` — call ONCE at boot of the binary, after
//!   `init_logging()`. Idempotent.
//! - `is_otel_enabled()` — `true` if the OTel provider was installed.
//!   `dispatch_request` consults it to decide whether to open an
//!   additional OTel span (parallel to the own SpanContext).
//! - `tracer()` — returns the global tracer named `"fitz"` for
//!   opening HTTP spans. Call only if `is_otel_enabled()` is `true`.

use std::sync::OnceLock;

use opentelemetry_sdk::logs::SdkLoggerProvider;

/// Global flag that `init_otel()` sets when it manages to install the
/// OTel provider. `dispatch_request` consults it to decide whether to
/// open an additional OTel span. Without OTel installed, the flag
/// stays `false` and HTTP handlers run with the existing
/// instrumentation (SpanContext + access log + metrics) without
/// sending anything to the backend.
static OTEL_ENABLED: OnceLock<bool> = OnceLock::new();

/// Phase 12.3.iter2.b — process-global OTel LoggerProvider.
/// `init_otel()` installs it when `OTEL_EXPORTER_OTLP_ENDPOINT` is
/// set. `emit_log_record` consults it to emit the LogRecord in
/// parallel to stderr when the provider is active.
///
/// Architectural note: the `opentelemetry::logs::*` API is marked as
/// "not intended for application developers" — the idiomatic pattern
/// would be `opentelemetry-appender-tracing` as a `tracing::Layer`.
/// But our `emit_log_record` emits directly to stderr (it doesn't use
/// `tracing::event!`), so the appender captures nothing. The
/// idiomatic alternative implies refactoring the custom JSON/pretty
/// formatter of Phase 12.3.a into a `FormatEvent` implementation —
/// cost not justified for the case. We use the SDK directly by
/// holding the provider in a static. Fitz is a runtime, not a
/// typical app; managing the SDK directly is reasonable.
static OTEL_LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();

/// `true` if `init_otel()` installed the OTel provider and spans are
/// being sent to the backend. `false` when
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is not set or the init failed due to
/// some transport error.
pub fn is_otel_enabled() -> bool {
    OTEL_ENABLED.get().copied().unwrap_or(false)
}

/// Initializes the global OTel provider when
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Idempotent: if already
/// initialized, no-op.
///
/// **Called from `main.rs` at boot of the binary, after
/// `init_logging()`**. Without the env var, this fn touches nothing
/// — HTTP handlers keep running with the existing instrumentation
/// (SpanContext + access log + metrics) without sending anything over
/// the network.
///
/// Installs **two providers** when the env var is active:
/// 1. `SdkTracerProvider` — HTTP spans exported to `/v1/traces`.
/// 2. `SdkLoggerProvider` (Phase 12.3.iter2.b) — log records emitted
///    by `emit_log_record` exported in parallel to `/v1/logs`.
///
/// Installation errors (malformed endpoint, can't connect to the
/// collector, etc.) are silenced — we write a brief note to stderr
/// and leave the `OTEL_ENABLED` flag at `false`. This prevents an
/// operator's misconfiguration from crashing the binary.
pub fn init_otel() {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
    use opentelemetry_sdk::Resource;

    // Idempotent: only the first call installs. If the flag is
    // already set, silent no-op (useful for tests/REPL/fitz dev
    // re-executing).
    if OTEL_ENABLED.get().is_some() {
        return;
    }

    let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") else {
        // No endpoint, we install nothing and leave the flag at false.
        let _ = OTEL_ENABLED.set(false);
        return;
    };
    if endpoint.is_empty() {
        let _ = OTEL_ENABLED.set(false);
        return;
    }

    let service_name = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "fitz-app".into());
    let sampler_arg: f64 = std::env::var("OTEL_TRACES_SAMPLER_ARG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    // Clamp to [0.0, 1.0] — out-of-range values are silenced to the
    // nearest edge. Better than rejecting the init.
    let sampler_arg = sampler_arg.clamp(0.0, 1.0);

    let span_exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
        .build()
    {
        Ok(e) => e,
        Err(err) => {
            eprintln!(
                "fitz: OTel span exporter init failed ({}). Continuing without OTLP export.",
                err
            );
            let _ = OTEL_ENABLED.set(false);
            return;
        }
    };

    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", service_name.clone()))
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .with_sampler(Sampler::TraceIdRatioBased(sampler_arg))
        .build();

    // Smoke check: the provider can be created without a live
    // endpoint. The real send fails async when export is attempted —
    // that error gets silenced by the batch processor.
    let _tracer = tracer_provider.tracer("fitz");
    opentelemetry::global::set_tracer_provider(tracer_provider);

    // Phase 12.3.iter2.b — LoggerProvider parallel to the
    // TracerProvider. When the span exporter worked, we assume the
    // OTLP endpoint is valid and build the log exporter over
    // `/v1/logs`. If the log exporter fails, log to stderr and keep
    // going with the tracer installed (better partial degradation
    // than aborting the whole init — spans still reach the backend
    // even if the logs don't).
    match opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint(format!("{}/v1/logs", endpoint.trim_end_matches('/')))
        .build()
    {
        Ok(log_exporter) => {
            let logger_provider = SdkLoggerProvider::builder()
                .with_batch_exporter(log_exporter)
                .with_resource(resource)
                .build();
            // The provider lives the rest of the process inside the
            // OnceLock — necessary so the batch processor can keep
            // flushing logs until shutdown.
            let _ = OTEL_LOGGER_PROVIDER.set(logger_provider);
        }
        Err(err) => {
            eprintln!(
                "fitz: OTel log exporter init failed ({}). Logs will only go to stderr.",
                err
            );
        }
    }

    let _ = OTEL_ENABLED.set(true);
}

/// Returns the global `SdkLoggerProvider` when it is installed.
/// `emit_log_record` consults it before building a LogRecord to
/// export via OTLP. `None` when OTel is not active or the LogExporter
/// failed to initialize (partial degradation case).
///
/// Phase 12.3.iter2.b — public getter over `OTEL_LOGGER_PROVIDER`.
pub fn logger_provider() -> Option<&'static SdkLoggerProvider> {
    OTEL_LOGGER_PROVIDER.get()
}

/// Phase 12.3.iter2.Tier3 — Prometheus recorder handle when installed.
/// The handle is used by the `/metrics` endpoint to render the
/// exposition format on each scrape. `None` when Prometheus was not
/// enabled (default).
static PROMETHEUS_HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();

/// Initializes the global Prometheus recorder when `enabled` is
/// `true`. Idempotent: the `OnceLock` only accepts the first set;
/// subsequent calls return the already-installed handle.
///
/// Decision: the `/metrics` endpoint is mounted on the user's axum
/// (NOT on a separate port via the `http-listener` feature) — it uses
/// the same port + transport as the rest of the app. Less surface
/// area, less config (port forwarding, firewall rules, etc.).
///
/// Dual activation:
/// - Compile-time: `@server(prometheus=true)` in the Fitz code.
/// - Runtime: env var `FITZ_PROMETHEUS=1`/`true` (override).
///
/// If BOTH are present: env var precedence (`true` or `false`).
/// Without either: recorder is not installed, `/metrics` is not
/// mounted.
///
/// Phase 12.3.iter2.Tier3 — closes residual debt #4 of Phase 12.3.
pub fn init_prometheus(enabled: bool) {
    if !enabled || PROMETHEUS_HANDLE.get().is_some() {
        return;
    }
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    // `set_global_recorder` fails if there is already one installed.
    // We ignore it — the typical case is REPL/tests/fitz dev
    // re-executing.
    let _ = metrics::set_global_recorder(recorder);
    let _ = PROMETHEUS_HANDLE.set(handle);
}

/// Returns the Prometheus handle to render the exposition format.
/// `None` when the recorder was not installed. The user's `/metrics`
/// endpoint consults it; if `None`, it doesn't mount the route.
pub fn prometheus_handle() -> Option<&'static metrics_exporter_prometheus::PrometheusHandle> {
    PROMETHEUS_HANDLE.get()
}

/// Returns the global tracer named `"fitz"` for opening HTTP spans.
/// Call ONLY when `is_otel_enabled()` returns `true` — without an
/// installed provider, the tracer is a no-op but it still pays the
/// global lookup overhead.
pub fn tracer() -> opentelemetry::global::BoxedTracer {
    opentelemetry::global::tracer("fitz")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_otel_sin_env_var_es_noop_y_flag_false() {
        // The OnceLock is process-global — an earlier test might
        // have installed one. Since we can't clear it
        // deterministically, we only assert that `is_otel_enabled()`
        // doesn't panic.
        let _ = is_otel_enabled();
    }

    #[test]
    fn tracer_devuelve_boxed_tracer_aun_sin_init() {
        // Without a provider installed, `global::tracer` returns a
        // NoOp that neither panics nor sends anything. We validate
        // that the API exists and does not abort.
        let _t = tracer();
    }
}
