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
use std::time::Duration;

// ============================================================
// 9.w.3.iter2 — Retry config + timezone + persistencia opts.
// ============================================================

/// Estrategia de backoff entre retries. Acordado D5: tres kinds desde
/// el día 1; default `Exponential`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackoffKind {
    /// `delay = initial_secs * 2^(attempt-1)`. Recomendado para
    /// jobs con dependencias externas que pueden bouncear (DB, HTTP).
    #[default]
    Exponential,
    /// `delay = initial_secs * attempt`. Más predecible que exponential.
    Linear,
    /// `delay = initial_secs`. Útil cuando ya sabés el rate-limit del
    /// upstream y querés un ritmo fijo.
    Constant,
}

impl BackoffKind {
    /// Parsea el string aceptado en `retry={backoff: "..."}`. El
    /// checker (9.w.3.iter2.a) ya garantiza que solo lleguen valores
    /// del whitelist, pero replicamos defensivamente en runtime.
    pub fn from_str_strict(s: &str) -> Result<Self, String> {
        match s {
            "exponential" => Ok(Self::Exponential),
            "linear" => Ok(Self::Linear),
            "constant" => Ok(Self::Constant),
            other => Err(format!(
                "backoff `{}` inválido. Aceptados: `exponential`/`linear`/`constant`.",
                other
            )),
        }
    }
}

/// Config de retries por job. `max=0` desactiva retry (= MVP); con
/// `max>0` cada job que falle vuelve a intentar hasta `max+1` veces
/// total (la primera + N retries) con el backoff calculado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    /// Cantidad máxima de **reintentos**. 0 = una sola corrida (igual
    /// que sin retry). 3 = primera + hasta 3 retries = 4 intentos max.
    pub max: u32,
    /// Estrategia entre retries.
    pub backoff: BackoffKind,
    /// Delay base en segundos. Cap mínimo 1.
    pub initial_secs: u64,
    /// Cap máximo del delay calculado. Evita que exponential explote.
    pub max_secs: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max: 0,
            backoff: BackoffKind::Exponential,
            initial_secs: 1,
            max_secs: 60,
        }
    }
}

impl RetryConfig {
    /// Calcula el delay antes del retry `attempt` (1-indexed: `attempt=1`
    /// es el primer retry tras un fallo, `attempt=2` el segundo, etc).
    /// Capeado por `max_secs`.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base = self.initial_secs.max(1);
        let cap = self.max_secs.max(1);
        let raw = match self.backoff {
            BackoffKind::Constant => base,
            BackoffKind::Linear => base.saturating_mul(attempt as u64),
            BackoffKind::Exponential => {
                let shift = (attempt.saturating_sub(1)).min(63) as u64;
                base.saturating_mul(1u64 << shift)
            }
        };
        Duration::from_secs(raw.min(cap))
    }
}

/// Opciones acumuladas de los kwargs de `@cron("...", tz=..., retry=...,
/// catch_up=..., store=...)`. El evaluator/codegen parsean los kwargs
/// del `Decorator` a esta struct y se la pasan al registry.
#[derive(Debug, Clone)]
pub struct CronJobOptions {
    /// Timezone IANA usada para calcular el próximo tick. Default `Utc`
    /// (comportamiento idéntico al MVP).
    pub tz: chrono_tz::Tz,
    /// Política de retry tras fallo. `None` = un solo intento (= MVP).
    pub retry: Option<RetryConfig>,
    /// Política de missed runs tras restart. `false` = skip (default),
    /// `true` = ejecutar UN run inmediato para el último tick perdido
    /// (no N, evita spam).
    pub catch_up: bool,
    /// Conn DB para persistencia del job. `None` = in-memory (= MVP,
    /// backwards-compat con programas viejos).
    pub store: Option<Arc<crate::db::DbConnHandle>>,
}

impl Default for CronJobOptions {
    fn default() -> Self {
        Self {
            tz: chrono_tz::UTC,
            retry: None,
            catch_up: false,
            store: None,
        }
    }
}

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
    /// 9.w.3.iter2 — timezone IANA usada para calcular el próximo
    /// tick. El scheduler hace `chrono::DateTime<Tz>::with_timezone`
    /// antes de pasar el "now" al `Schedule::upcoming`. Default UTC
    /// (paridad con el MVP).
    pub tz: chrono_tz::Tz,
    /// 9.w.3.iter2 — política de retry. `None` = una sola corrida
    /// por tick (= MVP). `Some(cfg)` = retry hasta `cfg.max` veces
    /// con el backoff de `cfg.backoff`.
    pub retry: Option<RetryConfig>,
    /// 9.w.3.iter2 — política de missed runs tras restart. `false` =
    /// skip ticks perdidos (default). `true` = ejecutar UN run
    /// inmediato si hubo missed runs (no N — evita spam).
    pub catch_up: bool,
    /// 9.w.3.iter2 — conn DB para persistencia del job (registry +
    /// runs). `None` = in-memory (= MVP).
    pub store: Option<Arc<crate::db::DbConnHandle>>,
}

impl std::fmt::Debug for CronJob {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("CronJob")
            .field("name", &self.name)
            .field("schedule", &self.schedule.to_string())
            .field("is_async", &self.is_async)
            .field("tz", &self.tz.name())
            .field("retry", &self.retry)
            .field("catch_up", &self.catch_up)
            .field(
                "store",
                &self.store.as_ref().map(|h| h.url_redacted.clone()),
            )
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
        options: CronJobOptions,
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
        let CronJobOptions {
            tz,
            retry,
            catch_up,
            store,
        } = options;
        self.jobs.lock().push(CronJob {
            name,
            schedule,
            handler,
            is_async,
            env,
            tz,
            retry,
            catch_up,
            store,
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

// ============================================================
// 9.w.3.iter2 — Helpers SQL para persistencia opcional de jobs.
// ============================================================
//
// Schema:
//   CREATE TABLE fitz_cron_jobs (
//       name TEXT PRIMARY KEY,
//       schedule TEXT NOT NULL,
//       tz TEXT NOT NULL DEFAULT 'UTC',
//       last_run_at TIMESTAMPTZ,
//       last_status TEXT,        -- 'ok' | 'failed'
//       last_error TEXT,
//       next_run_at TIMESTAMPTZ  -- reservado para visibility futura
//   );
//   CREATE TABLE fitz_cron_runs (
//       id BIGSERIAL PRIMARY KEY,
//       job_name TEXT NOT NULL,
//       started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
//       finished_at TIMESTAMPTZ,
//       status TEXT NOT NULL,    -- 'running' | 'ok' | 'failed' | 'retrying'
//       attempt INT NOT NULL DEFAULT 1,
//       error TEXT
//   );
//   CREATE INDEX idx_fitz_cron_runs_job_started
//       ON fitz_cron_runs (job_name, started_at DESC);
//
// Decisiones:
// - `CREATE TABLE IF NOT EXISTS` al boot del scheduler — sin
//   ceremonia, seguro contra múltiples instancias arrancando.
// - `name PRIMARY KEY` evita duplicados entre restarts.
// - `last_run_at` se actualiza al finalizar UN job run completo
//   (con éxito o tras agotar retries). No se actualiza por cada
//   attempt intermedio.
// - `fitz_cron_runs.attempt` empieza en 1 (el primer intento). Si
//   `retry.max=3` entonces hay hasta 4 rows por tick (1+3).
// - `error` es TEXT libre, sin tipado del FitzError — buffer de
//   debug. El user puede limpiar la tabla manualmente.

const SQL_CREATE_TABLE_JOBS: &str = "\
CREATE TABLE IF NOT EXISTS fitz_cron_jobs (\
    name TEXT PRIMARY KEY,\
    schedule TEXT NOT NULL,\
    tz TEXT NOT NULL DEFAULT 'UTC',\
    last_run_at TIMESTAMPTZ,\
    last_status TEXT,\
    last_error TEXT,\
    next_run_at TIMESTAMPTZ\
)";

const SQL_CREATE_TABLE_RUNS: &str = "\
CREATE TABLE IF NOT EXISTS fitz_cron_runs (\
    id BIGSERIAL PRIMARY KEY,\
    job_name TEXT NOT NULL,\
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),\
    finished_at TIMESTAMPTZ,\
    status TEXT NOT NULL,\
    attempt INT NOT NULL DEFAULT 1,\
    error TEXT\
)";

const SQL_CREATE_INDEX_RUNS: &str = "\
CREATE INDEX IF NOT EXISTS idx_fitz_cron_runs_job_started \
ON fitz_cron_runs (job_name, started_at DESC)";

/// Crea las dos tablas `fitz_cron_jobs` y `fitz_cron_runs` + el índice
/// si no existen. Idempotente; seguro contra concurrencia (Postgres
/// serializa `CREATE TABLE IF NOT EXISTS`).
#[doc(hidden)]
pub async fn init_storage(conn: &crate::db::DbConnHandle) -> Result<(), String> {
    conn.exec(SQL_CREATE_TABLE_JOBS, &[])
        .await
        .map_err(|e| format!("inicializando tabla fitz_cron_jobs: {}", e))?;
    conn.exec(SQL_CREATE_TABLE_RUNS, &[])
        .await
        .map_err(|e| format!("inicializando tabla fitz_cron_runs: {}", e))?;
    conn.exec(SQL_CREATE_INDEX_RUNS, &[])
        .await
        .map_err(|e| format!("inicializando índice fitz_cron_runs: {}", e))?;
    Ok(())
}

/// `INSERT ... ON CONFLICT (name) DO UPDATE` que registra/actualiza el
/// job en `fitz_cron_jobs`. Al reiniciar el proceso con la misma fn
/// `@cron("...", store=db)` el schedule/tz se sincronizan sin perder
/// `last_run_at` ni `last_status`.
#[doc(hidden)]
pub async fn upsert_job_row(
    conn: &crate::db::DbConnHandle,
    name: &str,
    schedule: &str,
    tz_name: &str,
) -> Result<(), String> {
    conn.exec(
        "INSERT INTO fitz_cron_jobs (name, schedule, tz) VALUES ($1, $2, $3) \
         ON CONFLICT (name) DO UPDATE SET schedule = EXCLUDED.schedule, tz = EXCLUDED.tz",
        &[
            crate::db::PgValue::Text(name.to_string()),
            crate::db::PgValue::Text(schedule.to_string()),
            crate::db::PgValue::Text(tz_name.to_string()),
        ],
    )
    .await
    .map_err(|e| format!("upsert fitz_cron_jobs '{}': {}", name, e))?;
    Ok(())
}

/// `INSERT INTO fitz_cron_runs (... status='running') RETURNING id`.
/// Devuelve el id BIGSERIAL para que `record_run_finish` lo actualice
/// más tarde.
#[doc(hidden)]
pub async fn record_run_start(
    conn: &crate::db::DbConnHandle,
    name: &str,
    attempt: u32,
) -> Result<i64, String> {
    let res = conn
        .query(
            "INSERT INTO fitz_cron_runs (job_name, attempt, status) \
             VALUES ($1, $2, 'running') RETURNING id",
            &[
                crate::db::PgValue::Text(name.to_string()),
                crate::db::PgValue::Int(attempt as i64),
            ],
        )
        .await
        .map_err(|e| format!("insert fitz_cron_runs '{}': {}", name, e))?;
    let row = res
        .rows
        .first()
        .ok_or_else(|| format!("fitz_cron_runs insert no devolvió id para '{}'", name))?;
    match row.get_at(0) {
        Some(crate::db::PgValue::Int(n)) => Ok(*n),
        // El driver puede devolver el BIGSERIAL como Text si el wire
        // format es text (Simple Query). Aceptamos ambas representaciones.
        Some(crate::db::PgValue::Text(s)) => s
            .parse::<i64>()
            .map_err(|_| format!("fitz_cron_runs id `{}` no parsea a Int", s)),
        other => Err(format!(
            "fitz_cron_runs id con shape inesperado: {:?}",
            other
        )),
    }
}

/// `UPDATE fitz_cron_runs SET status=..., finished_at=now(), error=... WHERE id=...`.
#[doc(hidden)]
pub async fn record_run_finish(
    conn: &crate::db::DbConnHandle,
    run_id: i64,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let err_val = match error {
        Some(s) => crate::db::PgValue::Text(s.to_string()),
        None => crate::db::PgValue::Null,
    };
    conn.exec(
        "UPDATE fitz_cron_runs SET status = $1, finished_at = now(), error = $2 WHERE id = $3",
        &[
            crate::db::PgValue::Text(status.to_string()),
            err_val,
            crate::db::PgValue::Int(run_id),
        ],
    )
    .await
    .map_err(|e| format!("update fitz_cron_runs id={}: {}", run_id, e))?;
    Ok(())
}

/// `UPDATE fitz_cron_jobs SET last_run_at=now(), last_status=..., last_error=... WHERE name=...`.
#[doc(hidden)]
pub async fn update_job_last_run(
    conn: &crate::db::DbConnHandle,
    name: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let err_val = match error {
        Some(s) => crate::db::PgValue::Text(s.to_string()),
        None => crate::db::PgValue::Null,
    };
    conn.exec(
        "UPDATE fitz_cron_jobs SET last_run_at = now(), last_status = $1, last_error = $2 \
         WHERE name = $3",
        &[
            crate::db::PgValue::Text(status.to_string()),
            err_val,
            crate::db::PgValue::Text(name.to_string()),
        ],
    )
    .await
    .map_err(|e| format!("update fitz_cron_jobs '{}': {}", name, e))?;
    Ok(())
}

/// `SELECT last_run_at FROM fitz_cron_jobs WHERE name=$1`. Devuelve
/// `Ok(None)` si la fila no existe (primer arranque del job) o si
/// `last_run_at` es NULL.
#[doc(hidden)]
pub async fn read_last_run_at(
    conn: &crate::db::DbConnHandle,
    name: &str,
) -> Result<Option<chrono::DateTime<Utc>>, String> {
    let res = conn
        .query(
            "SELECT last_run_at FROM fitz_cron_jobs WHERE name = $1",
            &[crate::db::PgValue::Text(name.to_string())],
        )
        .await
        .map_err(|e| format!("select fitz_cron_jobs '{}': {}", name, e))?;
    let Some(row) = res.rows.first() else {
        return Ok(None);
    };
    match row.get_at(0) {
        Some(crate::db::PgValue::Null) | None => Ok(None),
        Some(crate::db::PgValue::Text(s)) => {
            // Postgres text format para TIMESTAMPTZ: "2026-06-02 12:34:56.789+00".
            // chrono::DateTime parsea con `parse_from_str` o el formato RFC3339
            // si reformateamos el espacio a 'T'. Usamos un parse defensivo.
            parse_pg_timestamptz(s).map(Some)
        }
        other => Err(format!(
            "fitz_cron_jobs.last_run_at shape inesperado: {:?}",
            other
        )),
    }
}

/// Parsea un timestamptz Postgres text-format al `DateTime<Utc>`.
/// Postgres emite el offset de tz **sin minutos** (`+00`, `-03`,
/// `+05`) cuando son enteros, y con minutos cuando hay fracción
/// (`+05:30`). RFC3339 exige siempre `±HH:MM`. Normalizamos:
///   1. Espacio entre fecha y hora → `T`.
///   2. Offset sin minutos al final → agregamos `:00`.
fn parse_pg_timestamptz(s: &str) -> Result<chrono::DateTime<Utc>, String> {
    let normalized = s.replacen(' ', "T", 1);
    let with_offset = normalize_pg_tz_offset(&normalized);
    chrono::DateTime::parse_from_rfc3339(&with_offset)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("parsing timestamptz `{}`: {}", s, e))
}

/// Si el string termina en `±DD` (offset Postgres sin minutos),
/// agregamos `:00`. Casos cubiertos: `+00`, `-03`, `+05`. Si ya
/// trae `:MM` o termina en `Z`, no toca.
fn normalize_pg_tz_offset(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    if n < 3 {
        return s.to_string();
    }
    // Últimos 3 chars: `±DD`. Detectamos `+/-` en pos n-3 + dos dígitos.
    let sign = bytes[n - 3];
    let d1 = bytes[n - 2];
    let d2 = bytes[n - 1];
    if (sign == b'+' || sign == b'-') && d1.is_ascii_digit() && d2.is_ascii_digit() {
        return format!("{}:00", s);
    }
    s.to_string()
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
    eprintln!("🕐 Fitz scheduler arrancado con {} job(s) cron", jobs.len());
    for job in &jobs {
        eprintln!("   @cron  {} ({})", job.name, job.schedule);
    }
    for job in jobs {
        tokio::spawn(run_cron_job(job));
    }
}

/// 9.w.3.iter2 — Loop principal de un cron job, con tz-aware schedule,
/// retry con backoff, persistencia opcional y catch_up de missed runs.
///
/// Boot:
/// 1. Si `job.store.is_some()`: init storage + upsert del job row.
///    Falla del init → log y abort del task (sin storage no podemos
///    cumplir el contrato; otros jobs siguen corriendo).
/// 2. Si `job.catch_up && job.store.is_some()`: leer `last_run_at`,
///    detectar missed runs (≥1 tick entre last_run_at y now), si los
///    hay ejecutar UN run inmediato (no N — evita spam).
///
/// Loop:
/// 1. Calcular `next` con `Schedule::upcoming(job.tz)` y dormir hasta
///    ahí (convertido a UTC para el sleep).
/// 2. Llamar a `invoke_with_retry` que aplica la política de retry.
async fn run_cron_job(job: CronJob) {
    // ---- Boot: persistencia + catch_up ----
    if let Some(conn) = job.store.clone() {
        if let Err(e) = init_storage(&conn).await {
            eprintln!(
                "🕐 cron job '{}' no pudo inicializar storage, abortando task: {}",
                job.name, e
            );
            return;
        }
        if let Err(e) =
            upsert_job_row(&conn, &job.name, &job.schedule.to_string(), job.tz.name()).await
        {
            eprintln!(
                "🕐 cron job '{}' no pudo registrar en fitz_cron_jobs, abortando task: {}",
                job.name, e
            );
            return;
        }
        if job.catch_up {
            match read_last_run_at(&conn, &job.name).await {
                Ok(Some(last)) => {
                    // Hay missed runs si al menos UN tick programado cae
                    // entre `last` y `now`. Usamos `Schedule::after(last)`
                    // en la tz del job y pedimos el primero — si existe y
                    // es <= ahora, hubo missed runs.
                    let last_in_tz = last.with_timezone(&job.tz);
                    let now_in_tz = Utc::now().with_timezone(&job.tz);
                    let missed = job
                        .schedule
                        .after(&last_in_tz)
                        .next()
                        .map(|next| next <= now_in_tz)
                        .unwrap_or(false);
                    if missed {
                        eprintln!(
                            "🕐 cron job '{}' catch_up: missed runs detectados (last={}), \
                             ejecutando UN run inmediato.",
                            job.name, last
                        );
                        invoke_with_retry(&job).await;
                    }
                }
                Ok(None) => {
                    // Primer arranque del job — no hay last_run_at, no
                    // tiene sentido hablar de missed runs.
                }
                Err(e) => {
                    eprintln!(
                        "🕐 cron job '{}' catch_up: error leyendo last_run_at: {}",
                        job.name, e
                    );
                    // No abortamos — seguimos al loop normal.
                }
            }
        }
    }

    // ---- Loop normal ----
    loop {
        let Some(next_in_tz) = job.schedule.upcoming(job.tz).next() else {
            eprintln!("🕐 cron job '{}' agotó su schedule, terminando.", job.name);
            return;
        };
        let next_utc = next_in_tz.with_timezone(&Utc);
        let delay = (next_utc - Utc::now())
            .to_std()
            .unwrap_or_else(|_| std::time::Duration::from_millis(0));
        tokio::time::sleep(delay).await;
        invoke_with_retry(&job).await;
    }
}

/// 9.w.3.iter2 — Invoca el handler aplicando la política de retry.
/// Persiste cada attempt en `fitz_cron_runs` si `job.store.is_some()`.
/// Actualiza `fitz_cron_jobs.last_*` al finalizar el último intento.
///
/// Cantidad total de intentos = `1 + retry.max` (la primera corrida
/// más N retries). Si no hay `retry`, es 1 (paridad con MVP).
async fn invoke_with_retry(job: &CronJob) {
    let max_attempts = 1 + job.retry.map(|r| r.max).unwrap_or(0);
    let mut attempt: u32 = 1;
    loop {
        // 1) Persist run start (si hay storage).
        let run_id = if let Some(conn) = job.store.as_ref() {
            match record_run_start(conn, &job.name, attempt).await {
                Ok(id) => Some(id),
                Err(e) => {
                    eprintln!(
                        "🕐 cron job '{}' (attempt {}) record_run_start falló: {}",
                        job.name, attempt, e
                    );
                    None
                }
            }
        } else {
            None
        };

        // 2) Invocar el handler.
        let result = invoke_cron_handler(job).await;
        let is_last_attempt = attempt >= max_attempts;

        match result {
            Ok(()) => {
                if let (Some(conn), Some(rid)) = (job.store.as_ref(), run_id) {
                    let _ = record_run_finish(conn, rid, "ok", None).await;
                    let _ = update_job_last_run(conn, &job.name, "ok", None).await;
                }
                return;
            }
            Err(msg) => {
                // Persist el run con `failed` (último) o `retrying`
                // (siguientes intentos restantes).
                let status = if is_last_attempt {
                    "failed"
                } else {
                    "retrying"
                };
                if let (Some(conn), Some(rid)) = (job.store.as_ref(), run_id) {
                    let _ = record_run_finish(conn, rid, status, Some(&msg)).await;
                }
                if is_last_attempt {
                    eprintln!(
                        "🕐 cron job '{}' falló definitivamente tras {} intento(s): {}",
                        job.name, attempt, msg
                    );
                    if let Some(conn) = job.store.as_ref() {
                        let _ = update_job_last_run(conn, &job.name, "failed", Some(&msg)).await;
                    }
                    return;
                }
                // Hay retry pendiente: dormir el backoff y seguir.
                let retry_cfg = job.retry.expect("max_attempts > 1 implica retry.is_some()");
                let delay = retry_cfg.delay_for_attempt(attempt);
                eprintln!(
                    "🕐 cron job '{}' falló (attempt {}/{}): {} — retry en {:?}",
                    job.name, attempt, max_attempts, msg, delay
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// Invoca el handler de un cron job. Async fns devuelven `Future` que
/// awaiteamos; sync fns devuelven el value directo. El return value
/// se descarta (los jobs son fire-and-forget desde el punto de vista
/// del scheduler).
async fn invoke_cron_handler(job: &CronJob) -> Result<(), String> {
    use crate::ast::Span;
    use crate::evaluator::invoke_value;

    let result = invoke_value(job.handler.clone(), Vec::new(), &job.name, Span::ZERO).await;
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

// ============================================================
// 9.w.3.iter2 — Tests del registry extension.
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Environment;

    #[test]
    fn retry_config_default_es_no_retry() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max, 0);
        assert_eq!(cfg.backoff, BackoffKind::Exponential);
        assert_eq!(cfg.initial_secs, 1);
        assert_eq!(cfg.max_secs, 60);
    }

    #[test]
    fn delay_exponential_duplica_cada_attempt() {
        let cfg = RetryConfig {
            max: 5,
            backoff: BackoffKind::Exponential,
            initial_secs: 1,
            max_secs: 600,
        };
        assert_eq!(cfg.delay_for_attempt(1), Duration::from_secs(1));
        assert_eq!(cfg.delay_for_attempt(2), Duration::from_secs(2));
        assert_eq!(cfg.delay_for_attempt(3), Duration::from_secs(4));
        assert_eq!(cfg.delay_for_attempt(4), Duration::from_secs(8));
        assert_eq!(cfg.delay_for_attempt(5), Duration::from_secs(16));
    }

    #[test]
    fn delay_exponential_capeado_por_max_secs() {
        let cfg = RetryConfig {
            max: 10,
            backoff: BackoffKind::Exponential,
            initial_secs: 1,
            max_secs: 10,
        };
        // attempt=5 → 16s sin cap, capeado a 10s.
        assert_eq!(cfg.delay_for_attempt(5), Duration::from_secs(10));
        // attempts grandes nunca pasan el cap.
        assert_eq!(cfg.delay_for_attempt(20), Duration::from_secs(10));
    }

    #[test]
    fn delay_linear_multiplica_attempt_por_initial() {
        let cfg = RetryConfig {
            max: 5,
            backoff: BackoffKind::Linear,
            initial_secs: 3,
            max_secs: 100,
        };
        assert_eq!(cfg.delay_for_attempt(1), Duration::from_secs(3));
        assert_eq!(cfg.delay_for_attempt(2), Duration::from_secs(6));
        assert_eq!(cfg.delay_for_attempt(3), Duration::from_secs(9));
    }

    #[test]
    fn delay_constant_siempre_initial() {
        let cfg = RetryConfig {
            max: 5,
            backoff: BackoffKind::Constant,
            initial_secs: 5,
            max_secs: 100,
        };
        assert_eq!(cfg.delay_for_attempt(1), Duration::from_secs(5));
        assert_eq!(cfg.delay_for_attempt(7), Duration::from_secs(5));
    }

    #[test]
    fn backoff_kind_from_str_acepta_los_tres() {
        assert_eq!(
            BackoffKind::from_str_strict("exponential").unwrap(),
            BackoffKind::Exponential
        );
        assert_eq!(
            BackoffKind::from_str_strict("linear").unwrap(),
            BackoffKind::Linear
        );
        assert_eq!(
            BackoffKind::from_str_strict("constant").unwrap(),
            BackoffKind::Constant
        );
    }

    #[test]
    fn backoff_kind_from_str_rechaza_otros() {
        let err = BackoffKind::from_str_strict("quadratic").unwrap_err();
        assert!(err.contains("exponential"), "msg: {}", err);
    }

    #[test]
    fn cron_job_options_default_es_utc_in_memory() {
        let opts = CronJobOptions::default();
        assert_eq!(opts.tz, chrono_tz::UTC);
        assert!(opts.retry.is_none());
        assert!(!opts.catch_up);
        assert!(opts.store.is_none());
    }

    #[test]
    fn registry_register_con_defaults_es_backwards_compat() {
        // Sin tz/retry/catch_up/store: comportamiento equivale al MVP.
        let registry = CronRegistry::new();
        let env = Environment::new();
        let res = registry.register(
            "tick".to_string(),
            "0 0 * * *",
            Value::Null,
            false,
            env,
            CronJobOptions::default(),
        );
        assert!(res.is_ok(), "register debería pasar: {:?}", res);
        assert!(registry.has_jobs());
        let jobs = registry.jobs_snapshot();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.name, "tick");
        assert_eq!(job.tz, chrono_tz::UTC);
        assert!(job.retry.is_none());
        assert!(!job.catch_up);
        assert!(job.store.is_none());
    }

    #[test]
    fn registry_register_con_opciones_custom_preserva_campos() {
        let registry = CronRegistry::new();
        let env = Environment::new();
        let opts = CronJobOptions {
            tz: "America/Argentina/Buenos_Aires"
                .parse::<chrono_tz::Tz>()
                .unwrap(),
            retry: Some(RetryConfig {
                max: 3,
                backoff: BackoffKind::Linear,
                initial_secs: 2,
                max_secs: 30,
            }),
            catch_up: true,
            store: None,
        };
        let res = registry.register(
            "cleanup".to_string(),
            "0 3 * * *",
            Value::Null,
            true,
            env,
            opts,
        );
        assert!(res.is_ok());
        let jobs = registry.jobs_snapshot();
        let job = &jobs[0];
        assert_eq!(job.tz.name(), "America/Argentina/Buenos_Aires");
        let retry = job.retry.expect("retry");
        assert_eq!(retry.max, 3);
        assert_eq!(retry.backoff, BackoffKind::Linear);
        assert_eq!(retry.initial_secs, 2);
        assert_eq!(retry.max_secs, 30);
        assert!(job.catch_up);
    }

    #[test]
    fn registry_register_cron_invalido_devuelve_err() {
        let registry = CronRegistry::new();
        let env = Environment::new();
        let res = registry.register(
            "bad".to_string(),
            "no es un cron",
            Value::Null,
            false,
            env,
            CronJobOptions::default(),
        );
        assert!(res.is_err());
        let msg = res.unwrap_err();
        assert!(msg.contains("cron expression"), "msg: {}", msg);
    }
}
