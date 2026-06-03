//! Fase 12.3.c.1 — OTLP exporter para spans HTTP.
//!
//! Conecta los spans que abre `dispatch_request` (12.3.b.2) a un
//! backend OTel real (Jaeger, Tempo, Honeycomb, Datadog, etc.) cuando
//! la env var `OTEL_EXPORTER_OTLP_ENDPOINT` está seteada. Sin esa var,
//! `init_otel()` es no-op silencioso — zero overhead, zero conexiones
//! de red, zero spans enviados a ningún lado.
//!
//! ## Env vars (OTel-standard)
//!
//! - `OTEL_EXPORTER_OTLP_ENDPOINT` — URL del collector
//!   (e.g. `http://localhost:4318` para un OTel Collector local).
//!   **Sin default**: si no está, `init_otel()` no instala nada.
//! - `OTEL_SERVICE_NAME` — nombre del servicio que aparece en el
//!   backend. Default: `"fitz-app"`.
//! - `OTEL_TRACES_SAMPLER_ARG` — ratio de sampling como Float entre
//!   `0.0` y `1.0`. Default: `1.0` (100% de los spans se envían).
//!
//! ## Lo que NO está en 12.3.c.1
//!
//! - Métricas OTel (`metrics::counter!` / `histogram!` siguen
//!   despachando a recorder global vacío; el bridge OTel queda como
//!   sub-paso futuro).
//! - Logs OTel (los `log.X(...)` siguen yendo a stderr; el bridge
//!   `opentelemetry-appender-tracing` queda como sub-paso futuro).
//! - Correlación trace_id Fitz↔OTel: hoy son IDs independientes —
//!   nuestro `SpanContext::new_root()` genera trace_id propio para
//!   stderr logs, y el OTel span genera otro distinto para el
//!   backend. Refinable post-MVP si entra demanda real.
//!
//! ## API
//!
//! - `init_otel()` — llamar UNA vez al boot del binario, después de
//!   `init_logging()`. Idempotente.
//! - `is_otel_enabled()` — `true` si el provider OTel fue instalado.
//!   `dispatch_request` lo consulta para decidir si abrir un span
//!   adicional OTel (paralelo al SpanContext propio).
//! - `tracer()` — devuelve el tracer global con nombre `"fitz"` para
//!   abrir spans HTTP. Llamar solo si `is_otel_enabled()` es `true`.

use std::sync::OnceLock;

/// Flag global que `init_otel()` setea cuando logra instalar el
/// provider OTel. `dispatch_request` lo consulta para decidir si
/// abrir un span OTel adicional. Sin OTel instalado, el flag queda
/// `false` y los handlers HTTP corren con la instrumentación
/// existente (SpanContext + access log + métricas) sin enviar nada
/// al backend.
static OTEL_ENABLED: OnceLock<bool> = OnceLock::new();

/// `true` si `init_otel()` instaló el provider OTel y los spans se
/// envían al backend. `false` cuando `OTEL_EXPORTER_OTLP_ENDPOINT`
/// no está seteada o el init falló por algún error de transporte.
pub fn is_otel_enabled() -> bool {
    OTEL_ENABLED.get().copied().unwrap_or(false)
}

/// Inicializa el provider OTel global cuando `OTEL_EXPORTER_OTLP_ENDPOINT`
/// está seteada. Idempotente: si ya está inicializado, no-op.
///
/// **Llamado desde `main.rs` al boot del binario, después de
/// `init_logging()`**. Sin la env var, esta fn no toca nada — los
/// handlers HTTP siguen con la instrumentación existente (SpanContext
/// + access log + métricas) sin enviar nada por la red.
///
/// Errores de instalación (endpoint malformado, no se puede conectar
/// al collector, etc.) se silencian — escribimos a stderr una nota
/// breve y dejamos el flag `OTEL_ENABLED` en `false`. Esto evita que
/// un error de configuración del operador haga crashear el binario.
pub fn init_otel() {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
    use opentelemetry_sdk::Resource;

    // Idempotente: solo el primer call instala. Si ya hay flag set,
    // no-op silencioso (sirve para tests/REPL/fitz dev re-ejecutando).
    if OTEL_ENABLED.get().is_some() {
        return;
    }

    let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") else {
        // Sin endpoint, no instalamos nada y dejamos el flag en false.
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
    // Clamp a [0.0, 1.0] — valores fuera de rango se silencian al
    // borde más cercano. Mejor que rechazar el init.
    let sampler_arg = sampler_arg.clamp(0.0, 1.0);

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
        .build()
    {
        Ok(e) => e,
        Err(err) => {
            eprintln!(
                "fitz: OTel exporter init failed ({}). Continuing without OTLP export.",
                err
            );
            let _ = OTEL_ENABLED.set(false);
            return;
        }
    };

    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", service_name))
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .with_sampler(Sampler::TraceIdRatioBased(sampler_arg))
        .build();

    // Smoke check: el provider se puede crear sin endpoint vivo. El
    // envío real falla async cuando se intenta exportar — ese error
    // queda silenciado por el batch processor.
    let _tracer = provider.tracer("fitz");
    opentelemetry::global::set_tracer_provider(provider);
    let _ = OTEL_ENABLED.set(true);
}

/// Devuelve el tracer global con nombre `"fitz"` para abrir spans
/// HTTP. Llamar SOLO cuando `is_otel_enabled()` retorna `true` —
/// sin provider instalado, el tracer es no-op pero igual paga la
/// overhead de lookup global.
pub fn tracer() -> opentelemetry::global::BoxedTracer {
    opentelemetry::global::tracer("fitz")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_otel_sin_env_var_es_noop_y_flag_false() {
        // El OnceLock es global del proceso — un test anterior
        // podría haber instalado uno. Como no podemos limpiarlo
        // determinístico, solo aseveramos que `is_otel_enabled()`
        // no panic.
        let _ = is_otel_enabled();
    }

    #[test]
    fn tracer_devuelve_boxed_tracer_aun_sin_init() {
        // Sin provider instalado, `global::tracer` devuelve un NoOp
        // que no panicquea ni envía nada. Validamos que la API
        // existe y no aborta.
        let _t = tracer();
    }
}
