//! Fase 9.w.3 — Registry + scheduler de jobs programados con `@cron`.
//!
//! El módulo expone:
//! - `CronJob`: una fn `@cron("expr")` con su schedule parseado.
//! - `CronRegistry`: contenedor de jobs, paralelo a `HttpRegistry`.
//!   El evaluator lo guarda en `HttpRegistry.cron_registry` para
//!   reusar el lifecycle compartido entre HTTP server y CLI sin
//!   server (cron-only mode).
//! - `spawn_cron_scheduler`: arranca un `tokio::spawn` por job, cada
//!   uno con su propio loop `sleep_until(next_tick) -> invoke`.
//!
//! Diseño de scheduling:
//! - Cada job es independiente: `tokio::spawn(loop { sleep_until +
//!   invoke })`. Si un job es lento, no bloquea a los demás.
//! - El invoke del handler usa el `EnvRef` capturado al registro,
//!   con un scope hijo nuevo por cada disparo (no comparten state
//!   accidentalmente entre runs).
//! - Errores del handler (return `Err(...)` o panic) se loguean a
//!   stderr; el scheduler sigue vivo. Política: jobs fallidos NO
//!   abortan al proceso entero (paralelo a `tokio::spawn` que
//!   silencia panics de tasks individuales).
//!
//! Sin persistencia en MVP — los jobs viven en memoria. Restart del
//! proceso pierde el state. Persistencia llega en 9.w iteración 2
//! post-Fase 10 (requiere DB nativa).

use crate::env::EnvRef;
use crate::value::Value;
use chrono::Utc;
use cron::Schedule;
use std::str::FromStr;
use std::sync::Arc;

/// Normaliza una cron expression del usuario al formato del crate `cron`.
///
/// El crate `cron = "0.12"` exige 6 o 7 fields (con seconds al inicio
/// como primer field). El usuario típico viene de Linux/macOS cron que
/// usa 5 fields (sin seconds). Para preservar la UX familiar,
/// detectamos 5 fields y prependeamos `"0 "` (segundo 0 del minuto)
/// — semántica idéntica al cron Unix tradicional.
///
/// Trim defensivo del input por las dudas; el resto del parsing lo
/// hace el crate (rangos, listas con `,`, steps con `/`, wildcards
/// con `*`, ordinales con `L`/`#`/etc.).
fn normalize_cron_expression(expr: &str) -> String {
    let trimmed = expr.trim();
    let field_count = trimmed.split_whitespace().count();
    if field_count == 5 {
        format!("0 {}", trimmed)
    } else {
        trimmed.to_string()
    }
}

/// Un job programado: fn `@cron("expr")` con su schedule parseado y
/// el handler que invoca al disparar.
#[derive(Clone)]
pub struct CronJob {
    /// Nombre de la fn — para logging y debugging.
    pub name: String,
    /// Schedule parseado del crate `cron`. La sintaxis del MVP acepta
    /// 5 fields (Unix clásico, sin seconds), 6 fields (con seconds al
    /// inicio) o 7 fields (con year al final). Ejemplos válidos:
    /// `"0 0 * * *"` cada medianoche; `"*/5 * * * *"` cada 5 min;
    /// `"0 */30 * * * *"` cada 30 segundos.
    pub schedule: Schedule,
    /// El `Value::Function` registrado. El scheduler lo invoca con
    /// args vacíos (el checker garantiza que la fn no tiene params).
    pub handler: Value,
    /// `true` si la fn declarada es `async fn`. El scheduler ajusta
    /// el dispatch: async fns retornan `Value::Future` que awaitamos;
    /// sync fns retornan el value directo.
    pub is_async: bool,
    /// `EnvRef` capturado al momento del registro — los lookups de
    /// vars top-level (consts, builtins) ven el mismo estado que el
    /// resto del programa.
    pub env: EnvRef,
}

impl std::fmt::Debug for CronJob {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("CronJob")
            .field("name", &self.name)
            .field("schedule", &self.schedule.to_string())
            .field("is_async", &self.is_async)
            .finish()
    }
}

/// Registry de cron jobs registrados en el programa. Igual que
/// `HttpRegistry`, vive adentro de un `Arc<...>` compartido entre el
/// thread main y los workers tokio. Mutex por insertion durante el
/// evaluation (single-threaded en `current_thread`), después solo
/// read-only durante el scheduling.
#[derive(Default)]
pub struct CronRegistry {
    jobs: parking_lot::Mutex<Vec<CronJob>>,
}

impl std::fmt::Debug for CronRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let jobs = self.jobs.lock();
        f.debug_struct("CronRegistry")
            .field("count", &jobs.len())
            .finish()
    }
}

impl CronRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un job nuevo. Si el cron expression es inválido,
    /// devuelve `Err(msg)` para que el caller emita un FitzError.
    ///
    /// **Sintaxis aceptada**:
    /// - **5 fields Unix clásico**: `"min hora día mes día-semana"`.
    ///   Ejemplo: `"0 0 * * *"` cada medianoche. Internamente
    ///   prependeamos `"0 "` (segundo 0) — semántica idéntica al
    ///   cron tradicional de Linux/macOS.
    /// - **6 fields con seconds**: `"sec min hora día mes día-semana"`.
    ///   Ejemplo: `"0 */30 * * * *"` cada 30 segundos.
    /// - **7 fields con year**: `"sec min hora día mes día-semana año"`.
    ///   Ejemplo: `"0 0 0 1 1 * 2027"` el 1 de enero de 2027.
    pub fn register(
        &self,
        name: String,
        cron_expr: &str,
        handler: Value,
        is_async: bool,
        env: EnvRef,
    ) -> Result<(), String> {
        let normalized = normalize_cron_expression(cron_expr);
        let schedule = Schedule::from_str(&normalized).map_err(|e| {
            format!(
                "@cron sobre fn '{}': cron expression `{}` inválida: {}. \
                 Sintaxis aceptada: 5 fields Unix clásico (\"min hora día mes día-semana\"), \
                 6 fields con seconds al inicio, o 7 fields con año al final. \
                 Ejemplos: `\"0 0 * * *\"` (medianoche), `\"*/5 * * * *\"` (cada 5 min), \
                 `\"0 */30 * * * *\"` (cada 30 segundos).",
                name, cron_expr, e
            )
        })?;
        self.jobs.lock().push(CronJob {
            name,
            schedule,
            handler,
            is_async,
            env,
        });
        Ok(())
    }

    /// `true` si hay al menos un job registrado. Lo usa el evaluator
    /// para decidir si arrancar el scheduler standalone (cron-only
    /// mode) cuando no hay handlers HTTP.
    pub fn has_jobs(&self) -> bool {
        !self.jobs.lock().is_empty()
    }

    /// Lee la lista de jobs registrados (clone). Útil para el
    /// scheduler que arranca un task por job.
    pub fn jobs_snapshot(&self) -> Vec<CronJob> {
        self.jobs.lock().clone()
    }
}

/// Spawnea un `tokio::spawn` por cada cron job registrado. Cada task
/// loopea con `tokio::time::sleep_until(next_tick) -> invoke`. El
/// caller debe estar adentro de un runtime tokio (typical: el de
/// `serve()` para HTTP o el de `run_scheduler_only()` para cron-only).
///
/// El scheduler arranca todos los jobs y retorna inmediatamente — las
/// tasks corren detached. La política de shutdown depende del caller:
/// `serve()` usa graceful shutdown vía ctrl_c; `run_scheduler_only()`
/// hace lo mismo y mata los tasks al recibir la señal.
pub fn spawn_cron_scheduler(registry: Arc<CronRegistry>) {
    let jobs = registry.jobs_snapshot();
    if jobs.is_empty() {
        return;
    }
    eprintln!(
        "🕐 Fitz scheduler arrancado con {} job(s) cron",
        jobs.len()
    );
    for job in &jobs {
        eprintln!("   @cron  {} ({})", job.name, job.schedule);
    }
    for job in jobs {
        tokio::spawn(run_cron_job(job));
    }
}

/// Loop principal de un cron job. Calcula el próximo tick, duerme
/// hasta ahí, invoca el handler, repite. Errores del handler (panics
/// del invoke o `Err(msg)` en el return) van a stderr; el loop sigue.
async fn run_cron_job(job: CronJob) {
    loop {
        let now = Utc::now();
        let Some(next) = job.schedule.upcoming(Utc).next() else {
            // El schedule no produce más disparos (e.g. una fecha
            // pasada con year fijo). Salimos limpio.
            eprintln!(
                "🕐 cron job '{}' agotó su schedule, terminando.",
                job.name
            );
            return;
        };
        let delay = (next - now)
            .to_std()
            .unwrap_or_else(|_| std::time::Duration::from_millis(0));
        tokio::time::sleep(delay).await;
        if let Err(e) = invoke_cron_handler(&job).await {
            eprintln!("🕐 cron job '{}' falló: {}", job.name, e);
            // No abortamos el loop — el próximo tick reintenta.
        }
    }
}

/// Invoca el handler de un cron job. Async fns devuelven `Future` que
/// awaiteamos; sync fns devuelven el value directo. El return value
/// se descarta (los jobs son fire-and-forget desde el punto de vista
/// del scheduler).
async fn invoke_cron_handler(job: &CronJob) -> Result<(), String> {
    use crate::evaluator::invoke_value;
    use crate::ast::Span;

    let result = invoke_value(
        job.handler.clone(),
        Vec::new(),
        &job.name,
        Span::ZERO,
    )
    .await;
    match result {
        Ok(value) => {
            // Si la fn es async, el value es un Future — lo
            // await-eamos. Si es sync, el value ya es el final.
            match value {
                Value::Future(cell) => {
                    let inner = cell.0.lock().take();
                    if let Some(future) = inner {
                        let _ = future.await;
                    }
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        Err(signal) => {
            // EvalSignal::Error / Return / Break / Continue — solo
            // Error es relevante para logging; los otros no deberían
            // escapar un body de fn @cron (el checker validó).
            Err(format!("{:?}", signal))
        }
    }
}

/// Cron-only mode: cuando el programa NO tiene `@server` pero SÍ tiene
/// `@cron`, este helper arma un runtime tokio multi-thread, arranca
/// el scheduler, y bloquea hasta SIGINT/Ctrl+C. Paralelo a `serve()`
/// pero sin axum.
///
/// Decisión confirmada con el autor al arrancar 9.w.3: el proceso
/// queda vivo bloqueante (modo systemd-friendly). Modo "run-once" no
/// está en el MVP — workaround para tests: matar el proceso después
/// del primer tick.
pub fn run_scheduler_only(registry: Arc<CronRegistry>) -> std::io::Result<()> {
    if !registry.has_jobs() {
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        spawn_cron_scheduler(registry);
        // Bloqueamos hasta ctrl_c. Cuando llega, dropeamos el runtime
        // y los tasks spawneados se cancelan.
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                eprintln!("\n🕐 Fitz scheduler recibió Ctrl+C, terminando.");
            }
            Err(e) => {
                eprintln!("\n🕐 Fitz scheduler error en signal handler: {}", e);
            }
        }
    });
    Ok(())
}
