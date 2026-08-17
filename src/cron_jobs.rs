//! Phase 9.w.3 — Registry + scheduler for scheduled jobs with `@cron`.
//!
//! The module exposes:
//! - `CronJob`: a `@cron("expr")` fn with its parsed schedule.
//! - `CronRegistry`: jobs container, parallel to `HttpRegistry`.
//!   The evaluator stores it in `HttpRegistry.cron_registry` to
//!   reuse the shared lifecycle between HTTP server and CLI
//!   without server (cron-only mode).
//! - `spawn_cron_scheduler`: starts one `tokio::spawn` per job,
//!   each with its own loop `sleep_until(next_tick) -> invoke`.
//!
//! Scheduling design:
//! - Each job is independent: `tokio::spawn(loop { sleep_until +
//!   invoke })`. If a job is slow, it does not block the others.
//! - The handler invoke uses the `EnvRef` captured at registration,
//!   with a new child scope per fire (they do not accidentally
//!   share state between runs).
//! - Handler errors (`return Err(...)` or panic) are logged to
//!   stderr; the scheduler keeps running. Policy: failed jobs do
//!   NOT abort the whole process (parallel to `tokio::spawn` that
//!   silences panics of individual tasks).
//!
//! No persistence in MVP — jobs live in memory. Process restart
//! loses the state. Persistence arrives in 9.w iteration 2
//! post-Phase 10 (requires native DB).

use crate::env::EnvRef;
use crate::value::Value;
use chrono::Utc;
use cron::Schedule;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

// ============================================================
// 9.w.3.iter2 — Retry config + timezone + persistence opts.
// ============================================================

/// Backoff strategy between retries. Agreed D5: three kinds from
/// day 1; default `Exponential`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackoffKind {
    /// `delay = initial_secs * 2^(attempt-1)`. Recommended for
    /// jobs with external dependencies that can bounce (DB, HTTP).
    #[default]
    Exponential,
    /// `delay = initial_secs * attempt`. More predictable than
    /// exponential.
    Linear,
    /// `delay = initial_secs`. Useful when you already know the
    /// upstream's rate-limit and want a fixed pace.
    Constant,
}

impl BackoffKind {
    /// Parses the string accepted in `retry={backoff: "..."}`. The
    /// checker (9.w.3.iter2.a) already guarantees that only
    /// whitelist values arrive, but we replicate defensively at
    /// runtime.
    pub fn from_str_strict(s: &str) -> Result<Self, String> {
        match s {
            "exponential" => Ok(Self::Exponential),
            "linear" => Ok(Self::Linear),
            "constant" => Ok(Self::Constant),
            other => Err(format!(
                "invalid backoff `{}`. Accepted: `exponential`/`linear`/`constant`.",
                other
            )),
        }
    }
}

/// Per-job retry config. `max=0` disables retry (= MVP); with
/// `max>0` each failing job retries up to `max+1` times total
/// (the first + N retries) with the computed backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    /// Maximum number of **retries**. 0 = one single run (same
    /// as without retry). 3 = first + up to 3 retries = 4 max
    /// attempts.
    pub max: u32,
    /// Strategy between retries.
    pub backoff: BackoffKind,
    /// Base delay in seconds. Minimum cap 1.
    pub initial_secs: u64,
    /// Maximum cap of the computed delay. Avoids exponential
    /// blowing up.
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
    /// Computes the delay before retry `attempt` (1-indexed:
    /// `attempt=1` is the first retry after a failure, `attempt=2`
    /// the second, etc). Capped by `max_secs`.
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

/// Accumulated options from the kwargs of `@cron("...", tz=...,
/// retry=..., catch_up=..., store=...)`. The evaluator/codegen
/// parse the `Decorator` kwargs to this struct and pass it to the
/// registry.
#[derive(Debug, Clone)]
pub struct CronJobOptions {
    /// IANA timezone used to compute the next tick. Default `Utc`
    /// (behavior identical to MVP).
    pub tz: chrono_tz::Tz,
    /// Retry policy on failure. `None` = single attempt (= MVP).
    pub retry: Option<RetryConfig>,
    /// Missed runs policy after restart. `false` = skip (default),
    /// `true` = execute ONE immediate run for the last missed
    /// tick (not N, avoids spam).
    pub catch_up: bool,
    /// DB conn for job persistence. `None` = in-memory (= MVP,
    /// backwards-compat with old programs).
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

/// Normalizes a user cron expression to the `cron` crate format.
///
/// The `cron = "0.12"` crate requires 6 or 7 fields (with seconds
/// at the start as the first field). The typical user comes from
/// Linux/macOS cron which uses 5 fields (without seconds). To
/// preserve familiar UX, we detect 5 fields and prepend `"0 "`
/// (second 0 of the minute) — semantics identical to traditional
/// Unix cron.
///
/// Defensive trim of the input just in case; the rest of the
/// parsing is done by the crate (ranges, lists with `,`, steps
/// with `/`, wildcards with `*`, ordinals with `L`/`#`/etc.).
fn normalize_cron_expression(expr: &str) -> String {
    let trimmed = expr.trim();
    let field_count = trimmed.split_whitespace().count();
    if field_count == 5 {
        format!("0 {}", trimmed)
    } else {
        trimmed.to_string()
    }
}

/// A scheduled job: `@cron("expr")` fn with its parsed schedule
/// and the handler to invoke on fire.
#[derive(Clone)]
pub struct CronJob {
    /// Name of the fn — for logging and debugging.
    pub name: String,
    /// Parsed schedule of the `cron` crate. The MVP syntax
    /// accepts 5 fields (classic Unix, without seconds), 6 fields
    /// (with seconds at start), or 7 fields (with year at end).
    /// Valid examples: `"0 0 * * *"` every midnight;
    /// `"*/5 * * * *"` every 5 min; `"0 */30 * * * *"` every 30
    /// seconds.
    pub schedule: Schedule,
    /// The registered `Value::Function`. The scheduler invokes it
    /// with empty args (the checker guarantees the fn has no
    /// params).
    pub handler: Value,
    /// `true` if the declared fn is `async fn`. The scheduler
    /// adjusts dispatch: async fns return `Value::Future` that
    /// we await; sync fns return the value directly.
    pub is_async: bool,
    /// `EnvRef` captured at registration time — top-level var
    /// lookups (consts, builtins) see the same state as the rest
    /// of the program.
    pub env: EnvRef,
    /// 9.w.3.iter2 — IANA timezone used to compute the next tick.
    /// The scheduler does
    /// `chrono::DateTime<Tz>::with_timezone` before passing "now"
    /// to `Schedule::upcoming`. Default UTC (parallel to MVP).
    pub tz: chrono_tz::Tz,
    /// 9.w.3.iter2 — retry policy. `None` = single run per tick
    /// (= MVP). `Some(cfg)` = retry up to `cfg.max` times with
    /// the backoff of `cfg.backoff`.
    pub retry: Option<RetryConfig>,
    /// 9.w.3.iter2 — missed runs policy after restart. `false` =
    /// skip lost ticks (default). `true` = execute ONE immediate
    /// run if there were missed runs (not N — avoids spam).
    pub catch_up: bool,
    /// 9.w.3.iter2 — DB conn for job persistence (registry +
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

/// Registry of cron jobs registered in the program. Like
/// `HttpRegistry`, it lives inside an `Arc<...>` shared between
/// the main thread and tokio workers. Mutex for insertion during
/// evaluation (single-threaded in `current_thread`), then only
/// read-only during scheduling.
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

    /// Registers a new job. If the cron expression is invalid,
    /// returns `Err(msg)` so the caller emits a FitzError.
    ///
    /// **Accepted syntax**:
    /// - **5 fields classic Unix**: `"min hour day month day-of-week"`.
    ///   Example: `"0 0 * * *"` every midnight. Internally we
    ///   prepend `"0 "` (second 0) — semantics identical to
    ///   traditional Linux/macOS cron.
    /// - **6 fields with seconds**: `"sec min hour day month day-of-week"`.
    ///   Example: `"0 */30 * * * *"` every 30 seconds.
    /// - **7 fields with year**: `"sec min hour day month day-of-week year"`.
    ///   Example: `"0 0 0 1 1 * 2027"` on January 1st, 2027.
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
                "@cron on fn '{}': invalid cron expression `{}`: {}. \
                 Accepted syntax: classic 5-field Unix (\"min hour day month day-of-week\"), \
                 6 fields with seconds at the start, or 7 fields with year at the end. \
                 Examples: `\"0 0 * * *\"` (midnight), `\"*/5 * * * *\"` (every 5 min), \
                 `\"0 */30 * * * *\"` (every 30 seconds).",
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

    /// `true` if at least one job is registered. The evaluator
    /// uses it to decide whether to start the standalone
    /// scheduler (cron-only mode) when there are no HTTP handlers.
    pub fn has_jobs(&self) -> bool {
        !self.jobs.lock().is_empty()
    }

    /// Reads the list of registered jobs (clone). Useful for the
    /// scheduler that starts one task per job.
    pub fn jobs_snapshot(&self) -> Vec<CronJob> {
        self.jobs.lock().clone()
    }
}

// ============================================================
// 9.w.3.iter2 — SQL helpers for optional job persistence.
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
//       next_run_at TIMESTAMPTZ  -- reserved for future visibility
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
// Decisions:
// - `CREATE TABLE IF NOT EXISTS` at scheduler boot — no
//   ceremony, safe against multiple instances starting.
// - `name PRIMARY KEY` avoids duplicates between restarts.
// - `last_run_at` is updated when ONE complete job run finishes
//   (successfully or after exhausting retries). It is not
//   updated per intermediate attempt.
// - `fitz_cron_runs.attempt` starts at 1 (the first attempt).
//   If `retry.max=3` there are up to 4 rows per tick (1+3).
// - `error` is free TEXT, without typed FitzError — debug
//   buffer. The user can clean the table manually.

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

/// Creates the two tables `fitz_cron_jobs` and `fitz_cron_runs`
/// + the index if they do not exist.
///
/// **Important**: NOT concurrency-safe when called directly —
/// Postgres `CREATE TABLE IF NOT EXISTS` has a documented race
/// condition when two sessions execute it in parallel on the
/// same table, where one of the two sessions can break with
/// `duplicate key value violates unique constraint
/// "pg_type_typname_nsp_index"` (race on the `pg_type` catalog).
/// Bug documented upstream since ~2020.
///
/// For use from `run_cron_job` (which spawns N jobs in parallel
/// at boot), always use [`ensure_storage_initialized`] which
/// serializes the first init with a global `OnceCell`. This fn
/// stays public mainly for tests + cases where the caller
/// guarantees it is the only invocation in flight.
#[doc(hidden)]
pub async fn init_storage(conn: &crate::db::DbConnHandle) -> Result<(), String> {
    conn.exec(SQL_CREATE_TABLE_JOBS, &[])
        .await
        .map_err(|e| format!("initializing table fitz_cron_jobs: {}", e))?;
    conn.exec(SQL_CREATE_TABLE_RUNS, &[])
        .await
        .map_err(|e| format!("initializing table fitz_cron_runs: {}", e))?;
    conn.exec(SQL_CREATE_INDEX_RUNS, &[])
        .await
        .map_err(|e| format!("initializing index fitz_cron_runs: {}", e))?;
    Ok(())
}

/// v0.15.13 — Global `OnceCell` that serializes the first
/// `init_storage` of the process. Closes the URGENT-1 debt
/// (`docs/deudas-post-5b.md` → "init_storage of @cron with
/// persistence: race condition in CREATE TABLE").
///
/// When the binary has N `@cron(store=db_result)` and the spawns
/// start in parallel, the N ran `init_storage` simultaneously
/// against Postgres and hit the `pg_type` catalog race.
/// `OnceCell::get_or_init` guarantees that the async init closure
/// runs exactly ONCE per process; the other callers wait for the
/// first one and clone the stored `Result<(), String>`.
///
/// Decisions:
/// - **Global** (not per instance): assumes a single DB target
///   per process (99% case for apps). If in the future a binary
///   connects to several DBs and declares crons on each, move
///   this to an `Arc<OnceCell>` inside `CronRegistry`.
/// - **Cached result** (not `get_or_try_init`): if the first
///   init fails (e.g.: DB down at boot), the other jobs get the
///   same error and abort the same way. Retry is NOT done
///   automatically — process restart resets the OnceCell.
/// - **`tokio::sync::OnceCell`** (not `std::sync::OnceLock`):
///   allows async init without blocking the tokio runtime.
static INIT_STORAGE_ONCE: tokio::sync::OnceCell<Result<(), String>> =
    tokio::sync::OnceCell::const_new();

/// v0.15.13 — Concurrency-safe wrapper over [`init_storage`].
///
/// The first call of the process executes the real
/// `init_storage(conn)`. Subsequent calls (concurrent or
/// sequential) wait for the `OnceCell` and return the cached
/// result. No race between parallel jobs.
///
/// **Not safe across processes** — if two fitz binaries start
/// simultaneously against the same DB, both will attempt the
/// first init of their own OnceCell. For multi-process use
/// `pg_advisory_lock` inside `init_storage` (minor open debt —
/// typical cases deploy ONE container per service).
#[doc(hidden)]
pub async fn ensure_storage_initialized(conn: &crate::db::DbConnHandle) -> Result<(), String> {
    INIT_STORAGE_ONCE
        .get_or_init(|| async { init_storage(conn).await })
        .await
        .clone()
}

/// v0.15.13 — Test-only helper to reset the global `OnceCell`
/// between tests. Cargo test runs tests of the same crate in the
/// same process; without reset, the second test calling
/// `ensure_storage_initialized` would reuse the first one's
/// result (probably against a different DB).
///
/// **Do not use outside tests** — the reset's concurrent
/// behavior is not safe. Marked `#[doc(hidden)]` so it does not
/// appear in generated docs and `#[allow(dead_code)]` because in
/// builds without integration tests it has no call sites.
#[doc(hidden)]
#[allow(dead_code)]
pub fn reset_init_storage_once_for_tests() {
    // tokio::sync::OnceCell has `take()` to drop the cached value
    // but requires `&mut self`. Since our OnceCell is `static`, we
    // need `unsafe` to obtain mutable access — alternative would
    // be to use `lazy_static!` with a `Mutex<Option<...>>`, but
    // adding a dep just for tests is overkill. The "&mut static"
    // pattern inside tests is standard (compatible with miri if
    // single-threaded).
    //
    // SAFETY: only called from tests serialized with an external
    // Mutex or opt-in `#[ignore]` tests with `--test-threads=1`.
    unsafe {
        // The borrow checker does not allow taking &mut of a
        // static. We use a cast via raw pointer (UB-safe because
        // OnceCell::take only requires synchronized read of the
        // internal state).
        let cell_ptr: *const tokio::sync::OnceCell<Result<(), String>> =
            std::ptr::addr_of!(INIT_STORAGE_ONCE);
        let cell_mut = &mut *(cell_ptr as *mut tokio::sync::OnceCell<Result<(), String>>);
        let _ = cell_mut.take();
    }
}

/// `INSERT ... ON CONFLICT (name) DO UPDATE` that registers /
/// updates the job in `fitz_cron_jobs`. When restarting the
/// process with the same `@cron("...", store=db)` fn the
/// schedule/tz sync without losing `last_run_at` or
/// `last_status`.
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
/// Returns the BIGSERIAL id so that `record_run_finish` updates
/// it later.
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
        .ok_or_else(|| format!("fitz_cron_runs insert did not return id for '{}'", name))?;
    match row.get_at(0) {
        Some(crate::db::PgValue::Int(n)) => Ok(*n),
        // The driver may return the BIGSERIAL as Text if the wire
        // format is text (Simple Query). We accept both
        // representations.
        Some(crate::db::PgValue::Text(s)) => s
            .parse::<i64>()
            .map_err(|_| format!("fitz_cron_runs id `{}` does not parse to Int", s)),
        other => Err(format!(
            "fitz_cron_runs id with unexpected shape: {:?}",
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

/// `SELECT last_run_at FROM fitz_cron_jobs WHERE name=$1`. Returns
/// `Ok(None)` if the row does not exist (first boot of the job) or
/// if `last_run_at` is NULL.
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
            // Postgres text format for TIMESTAMPTZ: "2026-06-02 12:34:56.789+00".
            // chrono::DateTime parses with `parse_from_str` or the
            // RFC3339 format if we reformat the space to 'T'. We
            // use a defensive parse.
            parse_pg_timestamptz(s).map(Some)
        }
        other => Err(format!(
            "fitz_cron_jobs.last_run_at with unexpected shape: {:?}",
            other
        )),
    }
}

/// Parses a Postgres text-format timestamptz to `DateTime<Utc>`.
/// Postgres emits the tz offset **without minutes** (`+00`,
/// `-03`, `+05`) when they are whole, and with minutes when there
/// is a fraction (`+05:30`). RFC3339 always requires `±HH:MM`.
/// We normalize:
///   1. Space between date and time → `T`.
///   2. Offset without minutes at end → add `:00`.
fn parse_pg_timestamptz(s: &str) -> Result<chrono::DateTime<Utc>, String> {
    let normalized = s.replacen(' ', "T", 1);
    let with_offset = normalize_pg_tz_offset(&normalized);
    chrono::DateTime::parse_from_rfc3339(&with_offset)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("parsing timestamptz `{}`: {}", s, e))
}

/// If the string ends in `±DD` (Postgres offset without
/// minutes), we add `:00`. Covered cases: `+00`, `-03`, `+05`.
/// If it already brings `:MM` or ends in `Z`, do not touch.
fn normalize_pg_tz_offset(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    if n < 3 {
        return s.to_string();
    }
    // Last 3 chars: `±DD`. Detect `+/-` at pos n-3 + two digits.
    let sign = bytes[n - 3];
    let d1 = bytes[n - 2];
    let d2 = bytes[n - 1];
    if (sign == b'+' || sign == b'-') && d1.is_ascii_digit() && d2.is_ascii_digit() {
        return format!("{}:00", s);
    }
    s.to_string()
}

/// Spawns one `tokio::spawn` per registered cron job. Each task
/// loops with `tokio::time::sleep_until(next_tick) -> invoke`.
/// The caller must be inside a tokio runtime (typical: `serve()`
/// for HTTP or `run_scheduler_only()` for cron-only).
///
/// The scheduler starts all jobs and returns immediately — tasks
/// run detached. Shutdown policy depends on the caller: `serve()`
/// uses graceful shutdown via ctrl_c; `run_scheduler_only()`
/// does the same and kills the tasks on signal.
pub fn spawn_cron_scheduler(registry: Arc<CronRegistry>) {
    let jobs = registry.jobs_snapshot();
    if jobs.is_empty() {
        return;
    }
    eprintln!("🕐 Fitz scheduler started with {} cron job(s)", jobs.len());
    for job in &jobs {
        eprintln!("   @cron  {} ({})", job.name, job.schedule);
    }
    for job in jobs {
        tokio::spawn(run_cron_job(job));
    }
}

/// 9.w.3.iter2 — Main loop of a cron job, with tz-aware schedule,
/// retry with backoff, optional persistence, and catch_up of
/// missed runs.
///
/// Boot:
/// 1. If `job.store.is_some()`: init storage + upsert of the job
///    row. Init failure → log and abort the task (without storage
///    we cannot fulfill the contract; other jobs keep running).
/// 2. If `job.catch_up && job.store.is_some()`: read
///    `last_run_at`, detect missed runs (≥1 tick between
///    last_run_at and now), if any execute ONE immediate run (not
///    N — avoids spam).
///
/// Loop:
/// 1. Compute `next` with `Schedule::upcoming(job.tz)` and sleep
///    until then (converted to UTC for the sleep).
/// 2. Call `invoke_with_retry` which applies the retry policy.
async fn run_cron_job(job: CronJob) {
    // ---- Boot: persistence + catch_up ----
    if let Some(conn) = job.store.clone() {
        // v0.15.13 — use `ensure_storage_initialized` (wrapper
        // over a global OnceCell) instead of `init_storage`
        // directly. With N jobs spawned in parallel (TaskHub case
        // with 2 crons), the direct version hit a race in the
        // Postgres `pg_type` catalog and one of the jobs aborted
        // silently at boot. Closes URGENT-1 of
        // docs/deudas-post-5b.md.
        if let Err(e) = ensure_storage_initialized(&conn).await {
            eprintln!(
                "🕐 cron job '{}' could not initialize storage, aborting task: {}",
                job.name, e
            );
            return;
        }
        if let Err(e) =
            upsert_job_row(&conn, &job.name, &job.schedule.to_string(), job.tz.name()).await
        {
            eprintln!(
                "🕐 cron job '{}' could not register in fitz_cron_jobs, aborting task: {}",
                job.name, e
            );
            return;
        }
        if job.catch_up {
            match read_last_run_at(&conn, &job.name).await {
                Ok(Some(last)) => {
                    // There are missed runs if at least ONE
                    // scheduled tick falls between `last` and
                    // `now`. We use `Schedule::after(last)` in the
                    // job's tz and ask for the first — if it
                    // exists and is <= now, there were missed runs.
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
                            "🕐 cron job '{}' catch_up: missed runs detected (last={}), \
                             executing ONE immediate run.",
                            job.name, last
                        );
                        invoke_with_retry(&job).await;
                    }
                }
                Ok(None) => {
                    // First boot of the job — no last_run_at, no
                    // sense talking about missed runs.
                }
                Err(e) => {
                    eprintln!(
                        "🕐 cron job '{}' catch_up: error reading last_run_at: {}",
                        job.name, e
                    );
                    // We do not abort — continue to the normal loop.
                }
            }
        }
    }

    // ---- Normal loop ----
    loop {
        let Some(next_in_tz) = job.schedule.upcoming(job.tz).next() else {
            eprintln!(
                "🕐 cron job '{}' exhausted its schedule, terminating.",
                job.name
            );
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

/// 9.w.3.iter2 — Invokes the handler applying the retry policy.
/// Persists each attempt in `fitz_cron_runs` if
/// `job.store.is_some()`. Updates `fitz_cron_jobs.last_*` when
/// the last attempt finishes.
///
/// Total number of attempts = `1 + retry.max` (the first run +
/// N retries). If there is no `retry`, it is 1 (parallel to MVP).
async fn invoke_with_retry(job: &CronJob) {
    let max_attempts = 1 + job.retry.map(|r| r.max).unwrap_or(0);
    let mut attempt: u32 = 1;
    loop {
        // 1) Persist run start (if there is storage).
        let run_id = if let Some(conn) = job.store.as_ref() {
            match record_run_start(conn, &job.name, attempt).await {
                Ok(id) => Some(id),
                Err(e) => {
                    eprintln!(
                        "🕐 cron job '{}' (attempt {}) record_run_start failed: {}",
                        job.name, attempt, e
                    );
                    None
                }
            }
        } else {
            None
        };

        // 2) Invoke the handler.
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
                // Persist the run with `failed` (last) or `retrying`
                // (next remaining attempts).
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
                        "🕐 cron job '{}' failed definitively after {} attempt(s): {}",
                        job.name, attempt, msg
                    );
                    if let Some(conn) = job.store.as_ref() {
                        let _ = update_job_last_run(conn, &job.name, "failed", Some(&msg)).await;
                    }
                    return;
                }
                // Retry pending: sleep the backoff and continue.
                let retry_cfg = job.retry.expect("max_attempts > 1 implies retry.is_some()");
                let delay = retry_cfg.delay_for_attempt(attempt);
                eprintln!(
                    "🕐 cron job '{}' failed (attempt {}/{}): {} — retry in {:?}",
                    job.name, attempt, max_attempts, msg, delay
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// Invokes a cron job's handler. Async fns return a `Future` that
/// we await; sync fns return the value directly. The return value
/// is discarded (jobs are fire-and-forget from the scheduler's
/// point of view).
async fn invoke_cron_handler(job: &CronJob) -> Result<(), String> {
    use crate::ast::Span;
    use crate::evaluator::invoke_value;

    let result = invoke_value(job.handler.clone(), Vec::new(), &job.name, Span::ZERO).await;
    match result {
        Ok(value) => {
            // If the fn is async, the value is a Future — we
            // await it. If sync, the value is already the final.
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
            // EvalSignal::Error / Return / Break / Continue — only
            // Error is relevant for logging; the others should not
            // escape a @cron fn body (the checker validated).
            Err(format!("{:?}", signal))
        }
    }
}

/// Cron-only mode: when the program does NOT have `@server` but
/// DOES have `@cron`, this helper sets up a multi-thread tokio
/// runtime, starts the scheduler, and blocks until SIGINT/Ctrl+C.
/// Parallel to `serve()` but without axum.
///
/// Decision confirmed with the author when starting 9.w.3: the
/// process stays alive blocking (systemd-friendly mode). "Run-once"
/// mode is NOT in the MVP — workaround for tests: kill the
/// process after the first tick.
pub fn run_scheduler_only(registry: Arc<CronRegistry>) -> std::io::Result<()> {
    if !registry.has_jobs() {
        return Ok(());
    }
    // Build our own runtime and delegate (backward-compat for any
    // caller that does not already own a runtime).
    let runtime = crate::http::build_server_runtime()?;
    run_scheduler_on_runtime(&runtime, registry)
}

/// Same as [`run_scheduler_only`], but drives the scheduler on a
/// caller-provided runtime instead of building its own. `run_file` uses
/// this to share ONE runtime with the eval — so the DB connections the
/// eval opened (for `@cron(store=db)`) stay bound to a live reactor when
/// the scheduler runs its first query (`CREATE TABLE fitz_cron_jobs`).
/// See `http::build_server_runtime`.
pub fn run_scheduler_on_runtime(
    runtime: &tokio::runtime::Runtime,
    registry: Arc<CronRegistry>,
) -> std::io::Result<()> {
    if !registry.has_jobs() {
        return Ok(());
    }
    runtime.block_on(async move {
        spawn_cron_scheduler(registry);
        // We block until ctrl_c. When it arrives, we drop the
        // runtime and the spawned tasks are cancelled.
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                eprintln!("\n🕐 Fitz scheduler received Ctrl+C, terminating.");
            }
            Err(e) => {
                eprintln!("\n🕐 Fitz scheduler error in signal handler: {}", e);
            }
        }
    });
    Ok(())
}

// ============================================================
// Phase 3c — `@every(N)` interval jobs.
// ============================================================
//
// The interval analogue of `@cron`: a fixed period (in seconds) instead of a
// cron `Schedule`, so `@every(90)` (90 s) and `@every(0.5)` (sub-second) work
// where a cron expression can't. A separate, minimal path — no tz/retry/store —
// that reuses the same boot lifecycle (a registry on `HttpRegistry`, one tokio
// task per job, invoke via `invoke_value`) alongside the cron + HTTP schedulers.

/// A registered `@every(N)` job: run `handler` every `interval_secs` seconds.
#[derive(Clone, Debug)]
pub struct EveryJob {
    /// Name of the fn — for logging.
    pub name: String,
    /// Interval in seconds (`> 0`, checker-validated).
    pub interval_secs: f64,
    /// The registered `Value::Function` (invoked with no args).
    pub handler: Value,
    /// `true` if the fn is `async fn` (dispatch awaits the returned Future).
    pub is_async: bool,
    /// The `EnvRef` captured at registration, so lookups see the program state.
    pub env: EnvRef,
}

#[derive(Default, Debug)]
pub struct EveryRegistry {
    jobs: parking_lot::Mutex<Vec<EveryJob>>,
}

impl EveryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        name: String,
        interval_secs: f64,
        handler: Value,
        is_async: bool,
        env: EnvRef,
    ) {
        self.jobs.lock().push(EveryJob {
            name,
            interval_secs,
            handler,
            is_async,
            env,
        });
    }

    pub fn has_jobs(&self) -> bool {
        !self.jobs.lock().is_empty()
    }

    pub fn jobs_snapshot(&self) -> Vec<EveryJob> {
        self.jobs.lock().clone()
    }
}

/// Spawn one tokio task per interval job. Additive alongside the cron + HTTP
/// schedulers (never instead of them).
pub fn spawn_every_scheduler(registry: Arc<EveryRegistry>) {
    let jobs = registry.jobs_snapshot();
    if jobs.is_empty() {
        return;
    }
    eprintln!(
        "⏱ Fitz every-scheduler started with {} interval job(s)",
        jobs.len()
    );
    for job in &jobs {
        eprintln!("   @every {} ({}s)", job.name, job.interval_secs);
    }
    for job in jobs {
        tokio::spawn(run_every_job(job));
    }
}

/// One interval job: fire `handler` every `interval_secs`. The first run
/// happens AFTER the first interval (no immediate-at-boot tick — matches "every
/// N seconds"). Missed ticks are skipped (a slow handler doesn't queue a
/// backlog). Handler failures are logged; the loop keeps running.
async fn run_every_job(job: EveryJob) {
    let period = std::time::Duration::from_secs_f64(job.interval_secs);
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // `interval` yields immediately on the first `tick()`; consume it so the
    // first real run is after `interval_secs`, not at t=0.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        if let Err(e) = invoke_every_handler(&job).await {
            eprintln!("⏱ @every job '{}' failed: {}", job.name, e);
        }
    }
}

async fn invoke_every_handler(job: &EveryJob) -> Result<(), String> {
    use crate::ast::Span;
    use crate::evaluator::invoke_value;
    let _ = job.is_async; // implicit in the handler's returned value shape.
    let result = invoke_value(job.handler.clone(), Vec::new(), &job.name, Span::ZERO).await;
    match result {
        Ok(value) => match value {
            Value::Future(cell) => {
                let inner = cell.0.lock().take();
                if let Some(future) = inner {
                    let _ = future.await;
                }
                Ok(())
            }
            _ => Ok(()),
        },
        Err(signal) => Err(format!("{:?}", signal)),
    }
}

/// Scheduler-only mode (no HTTP): spawn the cron AND every schedulers on the
/// given runtime and block on Ctrl+C. Covers cron-only, every-only, and
/// cron+every programs with no `@server`/`@ws`/HTTP routes.
pub fn run_schedulers_on_runtime(
    runtime: &tokio::runtime::Runtime,
    cron_registry: Arc<CronRegistry>,
    every_registry: Arc<EveryRegistry>,
) -> std::io::Result<()> {
    if !cron_registry.has_jobs() && !every_registry.has_jobs() {
        return Ok(());
    }
    runtime.block_on(async move {
        spawn_cron_scheduler(cron_registry);
        spawn_every_scheduler(every_registry);
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                eprintln!("\n🕐 Fitz scheduler received Ctrl+C, terminating.");
            }
            Err(e) => {
                eprintln!("\n🕐 Fitz scheduler error in signal handler: {}", e);
            }
        }
    });
    Ok(())
}

// ============================================================
// 9.w.3.iter2 — Registry extension tests.
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Environment;

    #[test]
    fn retry_config_default_is_no_retry() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max, 0);
        assert_eq!(cfg.backoff, BackoffKind::Exponential);
        assert_eq!(cfg.initial_secs, 1);
        assert_eq!(cfg.max_secs, 60);
    }

    #[test]
    fn delay_exponential_doubles_each_attempt() {
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
    fn delay_exponential_capped_by_max_secs() {
        let cfg = RetryConfig {
            max: 10,
            backoff: BackoffKind::Exponential,
            initial_secs: 1,
            max_secs: 10,
        };
        // attempt=5 → 16s without cap, capped to 10s.
        assert_eq!(cfg.delay_for_attempt(5), Duration::from_secs(10));
        // Large attempts never exceed the cap.
        assert_eq!(cfg.delay_for_attempt(20), Duration::from_secs(10));
    }

    #[test]
    fn delay_linear_multiplies_attempt_by_initial() {
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
    fn delay_constant_always_initial() {
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
    fn backoff_kind_from_str_accepts_all_three() {
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
    fn backoff_kind_from_str_rejects_others() {
        let err = BackoffKind::from_str_strict("quadratic").unwrap_err();
        assert!(err.contains("exponential"), "msg: {}", err);
    }

    #[test]
    fn cron_job_options_default_is_utc_in_memory() {
        let opts = CronJobOptions::default();
        assert_eq!(opts.tz, chrono_tz::UTC);
        assert!(opts.retry.is_none());
        assert!(!opts.catch_up);
        assert!(opts.store.is_none());
    }

    #[test]
    fn registry_register_with_defaults_is_backwards_compat() {
        // Without tz/retry/catch_up/store: behavior equals MVP.
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
        assert!(res.is_ok(), "register should pass: {:?}", res);
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
    fn registry_register_with_custom_options_preserves_fields() {
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
    fn registry_register_invalid_cron_returns_err() {
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

    // ---- Phase 3c — @every(N) interval registry ----

    #[test]
    fn every_registry_register_and_snapshot() {
        let registry = EveryRegistry::new();
        assert!(!registry.has_jobs());
        let env = Environment::new();
        registry.register("tick".to_string(), 1.5, Value::Null, false, env);
        assert!(registry.has_jobs());
        let jobs = registry.jobs_snapshot();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "tick");
        assert_eq!(jobs[0].interval_secs, 1.5);
        assert!(!jobs[0].is_async);
    }

    #[test]
    fn every_registry_holds_multiple_jobs() {
        let registry = EveryRegistry::new();
        registry.register("a".to_string(), 1.0, Value::Null, false, Environment::new());
        registry.register("b".to_string(), 0.5, Value::Null, true, Environment::new());
        let jobs = registry.jobs_snapshot();
        assert_eq!(jobs.len(), 2);
        assert!(jobs
            .iter()
            .any(|j| j.name == "b" && j.is_async && j.interval_secs == 0.5));
    }
}
