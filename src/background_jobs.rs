//! v0.37.7 — Registry + persistence for `@background` jobs with
//! `spawn(...)`.
//!
//! Parallel to [`crate::cron_jobs`] but for the fire-and-forget
//! `spawn(fn(args))` model instead of scheduled jobs. The module
//! exposes:
//! - [`BackgroundRegistry`]: maps each `@background(store=db)` fn
//!   name to its persistence config (store + retry + catch_up).
//!   The evaluator stores it in `HttpRegistry.background_registry`.
//! - SQL helpers over the single table `fitz_bg_jobs`.
//! - [`run_persisted_spawn`]: the retry loop that persists one row
//!   per `spawn(...)` (best-effort).
//!
//! Design (agreed with the author, 2026-08-10):
//! - **One table** (`fitz_bg_jobs`) — a background job is a single
//!   `spawn(...)`, one lifecycle, no recurrence. Retries update the
//!   SAME row (attempt counter + status), unlike cron's two tables
//!   (`fitz_cron_jobs` + `fitz_cron_runs`) which model recurrence.
//! - **Best-effort** — the INSERT is the first step INSIDE the
//!   spawned task, so `spawn(...)` stays fire-and-forget and
//!   non-blocking. If the DB is down at spawn time, the job still
//!   runs (persistence is additive, never blocks the work).
//! - **Args serialized to JSON** (via `value_to_json`) only for
//!   visibility. They are NEVER deserialized back — `catch_up`
//!   marks orphans as failed, it does not re-execute.
//! - **catch_up** — at boot, orphaned rows (`running`/`retrying`
//!   left mid-flight by a crash) are marked `failed`. No
//!   re-execution, no dispatch, no arg re-hydration.
//!
//! The retry loop reuses [`crate::cron_jobs::RetryConfig`] for the
//! backoff computation. The codegen (`fitz build`) emits a
//! bit-for-bit parallel copy of this machinery as `&str` preludes
//! (`__fitz_bg_*`) — keep both in sync.

use std::collections::HashMap;
use std::sync::Arc;

use crate::cron_jobs::RetryConfig;
use crate::db::DbConnHandle;

// ============================================================
// SQL schema + init (single table, parallel to cron's DDL).
// ============================================================
//
// Schema:
//   CREATE TABLE fitz_bg_jobs (
//       id BIGSERIAL PRIMARY KEY,
//       fn_name TEXT NOT NULL,
//       args_json TEXT NOT NULL,      -- JSON array of args (visibility only)
//       status TEXT NOT NULL,         -- 'running' | 'ok' | 'failed' | 'retrying'
//       attempt INT NOT NULL DEFAULT 1,
//       error TEXT,
//       created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
//       finished_at TIMESTAMPTZ
//   );
//   CREATE INDEX idx_fitz_bg_jobs_status
//       ON fitz_bg_jobs (status, created_at DESC);

const SQL_CREATE_TABLE_BG_JOBS: &str = "\
CREATE TABLE IF NOT EXISTS fitz_bg_jobs (\
    id BIGSERIAL PRIMARY KEY,\
    fn_name TEXT NOT NULL,\
    args_json TEXT NOT NULL,\
    status TEXT NOT NULL,\
    attempt INT NOT NULL DEFAULT 1,\
    error TEXT,\
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),\
    finished_at TIMESTAMPTZ\
)";

const SQL_CREATE_INDEX_BG: &str = "\
CREATE INDEX IF NOT EXISTS idx_fitz_bg_jobs_status \
ON fitz_bg_jobs (status, created_at DESC)";

/// Creates the `fitz_bg_jobs` table + index if they do not exist.
///
/// **Not concurrency-safe when called directly** — Postgres
/// `CREATE TABLE IF NOT EXISTS` has a documented `pg_type` catalog
/// race under parallel execution. Use [`ensure_bg_storage_initialized`]
/// which serializes the first init with a global `OnceCell`. This fn
/// stays public for tests + single-invocation callers.
#[doc(hidden)]
pub async fn init_bg_storage(conn: &DbConnHandle) -> Result<(), String> {
    conn.exec(SQL_CREATE_TABLE_BG_JOBS, &[])
        .await
        .map_err(|e| format!("initializing table fitz_bg_jobs: {}", e))?;
    conn.exec(SQL_CREATE_INDEX_BG, &[])
        .await
        .map_err(|e| format!("initializing index fitz_bg_jobs: {}", e))?;
    Ok(())
}

/// Global `OnceCell` that serializes the first `init_bg_storage` of
/// the process (parallel to cron's `INIT_STORAGE_ONCE`, v0.15.13 —
/// avoids the `pg_type` catalog race when several spawns init in
/// parallel). Global = assumes a single DB target per process (99%
/// case); the cached result is shared across callers.
static INIT_BG_STORAGE_ONCE: tokio::sync::OnceCell<Result<(), String>> =
    tokio::sync::OnceCell::const_new();

/// Concurrency-safe wrapper over [`init_bg_storage`]. The first call
/// of the process runs the real init; subsequent calls (concurrent
/// or sequential) wait on the `OnceCell` and return the cached
/// result. Not safe across processes.
#[doc(hidden)]
pub async fn ensure_bg_storage_initialized(conn: &DbConnHandle) -> Result<(), String> {
    INIT_BG_STORAGE_ONCE
        .get_or_init(|| async { init_bg_storage(conn).await })
        .await
        .clone()
}

/// Test-only reset of the global `OnceCell` between tests. See
/// `cron_jobs::reset_init_storage_once_for_tests` for the rationale
/// of the `&mut static` pattern.
///
/// SAFETY: only called from tests serialized with an external Mutex
/// or opt-in `#[ignore]` tests with `--test-threads=1`.
#[doc(hidden)]
#[allow(dead_code)]
pub fn reset_init_bg_storage_once_for_tests() {
    unsafe {
        let cell_ptr: *const tokio::sync::OnceCell<Result<(), String>> =
            std::ptr::addr_of!(INIT_BG_STORAGE_ONCE);
        let cell_mut = &mut *(cell_ptr as *mut tokio::sync::OnceCell<Result<(), String>>);
        let _ = cell_mut.take();
    }
}

/// `INSERT INTO fitz_bg_jobs (... status='running') RETURNING id`.
/// Returns the BIGSERIAL id so the finish/retrying updates target
/// the same row. Parses the id as `Int` or `Text` (the driver may
/// return BIGSERIAL as text under the Simple Query wire format).
#[doc(hidden)]
pub async fn insert_bg_job_running(
    conn: &DbConnHandle,
    fn_name: &str,
    args_json: &str,
) -> Result<i64, String> {
    let res = conn
        .query(
            "INSERT INTO fitz_bg_jobs (fn_name, args_json, status, attempt) \
             VALUES ($1, $2, 'running', 1) RETURNING id",
            &[
                crate::db::PgValue::Text(fn_name.to_string()),
                crate::db::PgValue::Text(args_json.to_string()),
            ],
        )
        .await
        .map_err(|e| format!("insert fitz_bg_jobs '{}': {}", fn_name, e))?;
    let row = res
        .rows
        .first()
        .ok_or_else(|| format!("fitz_bg_jobs insert did not return id for '{}'", fn_name))?;
    match row.get_at(0) {
        Some(crate::db::PgValue::Int(n)) => Ok(*n),
        Some(crate::db::PgValue::Text(s)) => s
            .parse::<i64>()
            .map_err(|_| format!("fitz_bg_jobs id `{}` does not parse to Int", s)),
        other => Err(format!(
            "fitz_bg_jobs id with unexpected shape: {:?}",
            other
        )),
    }
}

/// `UPDATE fitz_bg_jobs SET status=..., finished_at=now(), error=... WHERE id=...`.
/// Terminal update: marks the row `ok` or `failed`.
#[doc(hidden)]
pub async fn update_bg_job_finish(
    conn: &DbConnHandle,
    job_id: i64,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let err_val = match error {
        Some(s) => crate::db::PgValue::Text(s.to_string()),
        None => crate::db::PgValue::Null,
    };
    conn.exec(
        "UPDATE fitz_bg_jobs SET status = $1, finished_at = now(), error = $2 WHERE id = $3",
        &[
            crate::db::PgValue::Text(status.to_string()),
            err_val,
            crate::db::PgValue::Int(job_id),
        ],
    )
    .await
    .map_err(|e| format!("update fitz_bg_jobs id={}: {}", job_id, e))?;
    Ok(())
}

/// `UPDATE fitz_bg_jobs SET status='retrying', attempt=..., error=... WHERE id=...`.
/// Intermediate update between failed attempts (no `finished_at`).
#[doc(hidden)]
pub async fn update_bg_job_retrying(
    conn: &DbConnHandle,
    job_id: i64,
    attempt: u32,
    error: Option<&str>,
) -> Result<(), String> {
    let err_val = match error {
        Some(s) => crate::db::PgValue::Text(s.to_string()),
        None => crate::db::PgValue::Null,
    };
    conn.exec(
        "UPDATE fitz_bg_jobs SET status = 'retrying', attempt = $1, error = $2 WHERE id = $3",
        &[
            crate::db::PgValue::Int(attempt as i64),
            err_val,
            crate::db::PgValue::Int(job_id),
        ],
    )
    .await
    .map_err(|e| format!("update fitz_bg_jobs id={} retrying: {}", job_id, e))?;
    Ok(())
}

/// `UPDATE fitz_bg_jobs SET status='failed', ... WHERE status IN
/// ('running','retrying')`. Run at boot when `catch_up=true`: any
/// row a crash left mid-flight is marked failed (visibility only —
/// jobs are NOT re-executed).
#[doc(hidden)]
pub async fn mark_orphaned_failed(conn: &DbConnHandle) -> Result<u64, String> {
    conn.exec(
        "UPDATE fitz_bg_jobs SET status = 'failed', finished_at = now(), \
         error = 'orphaned by restart' WHERE status IN ('running', 'retrying')",
        &[],
    )
    .await
    .map_err(|e| format!("marking orphaned fitz_bg_jobs: {}", e))
}

// ============================================================
// Registry — maps @background fn name -> persistence config.
// ============================================================

/// Persistence config of one `@background(store=db)` fn.
#[derive(Clone)]
pub struct BgEntry {
    /// DB conn where each `spawn(...)` of this fn is recorded.
    pub store: Arc<DbConnHandle>,
    /// Retry policy on failure. `None` = single attempt.
    pub retry: Option<RetryConfig>,
    /// `true` = mark orphaned rows as failed at boot.
    pub catch_up: bool,
}

impl std::fmt::Debug for BgEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("BgEntry")
            .field("store", &self.store.url_redacted)
            .field("retry", &self.retry)
            .field("catch_up", &self.catch_up)
            .finish()
    }
}

/// Registry of `@background(store=db)` fns. Populated at boot by the
/// evaluator (`process_decorator`); consulted per `spawn(...)` by
/// `eval_spawn_call`. Fns without `store` are NOT registered here
/// (they stay in-memory fire-and-forget, backward-compat).
#[derive(Default)]
pub struct BackgroundRegistry {
    entries: parking_lot::Mutex<HashMap<String, BgEntry>>,
}

impl std::fmt::Debug for BackgroundRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let entries = self.entries.lock();
        f.debug_struct("BackgroundRegistry")
            .field("count", &entries.len())
            .finish()
    }
}

impl BackgroundRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers (or overwrites) the persistence config for a
    /// `@background` fn name.
    pub fn register(
        &self,
        fn_name: String,
        store: Arc<DbConnHandle>,
        retry: Option<RetryConfig>,
        catch_up: bool,
    ) {
        self.entries.lock().insert(
            fn_name,
            BgEntry {
                store,
                retry,
                catch_up,
            },
        );
    }

    /// Returns the persistence config of a fn (clone; `store` is an
    /// `Arc`, so the clone is cheap). `None` = the fn is not
    /// persisted → the spawn stays in-memory.
    pub fn entry(&self, fn_name: &str) -> Option<BgEntry> {
        self.entries.lock().get(fn_name).cloned()
    }

    /// `true` if no `@background(store=...)` fn is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Distinct stores (by redacted URL) of the fns that declared
    /// `catch_up=true`. Used at boot to mark orphaned rows. Dedup
    /// avoids marking the same table twice when several fns share a
    /// DB (the common case).
    pub fn catch_up_stores(&self) -> Vec<Arc<DbConnHandle>> {
        let entries = self.entries.lock();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in entries.values() {
            if e.catch_up && seen.insert(e.store.url_redacted.clone()) {
                out.push(e.store.clone());
            }
        }
        out
    }
}

// ============================================================
// Persisted spawn — retry loop that records one row per spawn.
// ============================================================

/// Runs a persisted `spawn(...)`: records one `fitz_bg_jobs` row and
/// applies the retry policy, updating the SAME row per attempt.
///
/// Best-effort: if init or the INSERT fails, the job still runs (the
/// persistence is additive — a DB hiccup must not swallow the work).
/// The `invoke` closure produces the `Result<(), String>` of one
/// attempt; the caller supplies it (it already resolved the handler
/// + arg values). Retries clone-and-re-invoke via the closure.
///
/// Bit-for-bit parallel to `__fitz_run_persisted_spawn` in the
/// codegen (`fitz build`).
pub async fn run_persisted_spawn<F, Fut>(
    store: Arc<DbConnHandle>,
    fn_name: String,
    args_json: String,
    retry: Option<RetryConfig>,
    mut invoke: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    // 1) Ensure the table exists (best-effort). If it fails we still
    //    run the job, just without persisting (id = None).
    let storage_ok = match ensure_bg_storage_initialized(&store).await {
        Ok(()) => true,
        Err(e) => {
            eprintln!("⚙️  @background '{}' storage init failed: {}", fn_name, e);
            false
        }
    };

    // 2) INSERT the row as 'running' (best-effort).
    let job_id = if storage_ok {
        match insert_bg_job_running(&store, &fn_name, &args_json).await {
            Ok(id) => Some(id),
            Err(e) => {
                eprintln!("⚙️  @background '{}' insert failed: {}", fn_name, e);
                None
            }
        }
    } else {
        None
    };

    // 3) Retry loop over the same row.
    let max_attempts = 1 + retry.map(|r| r.max).unwrap_or(0);
    let mut attempt: u32 = 1;
    loop {
        let result = invoke().await;
        let is_last_attempt = attempt >= max_attempts;
        match result {
            Ok(()) => {
                if let Some(id) = job_id {
                    let _ = update_bg_job_finish(&store, id, "ok", None).await;
                }
                return;
            }
            Err(msg) => {
                if is_last_attempt {
                    eprintln!(
                        "⚙️  @background '{}' failed definitively after {} attempt(s): {}",
                        fn_name, attempt, msg
                    );
                    if let Some(id) = job_id {
                        let _ = update_bg_job_finish(&store, id, "failed", Some(&msg)).await;
                    }
                    return;
                }
                // Retry pending: bump attempt on the row, sleep the
                // backoff, and re-invoke.
                let retry_cfg = retry.expect("max_attempts > 1 implies retry.is_some()");
                let next_attempt = attempt + 1;
                if let Some(id) = job_id {
                    let _ = update_bg_job_retrying(&store, id, next_attempt, Some(&msg)).await;
                }
                let delay = retry_cfg.delay_for_attempt(attempt);
                eprintln!(
                    "⚙️  @background '{}' failed (attempt {}/{}): {} — retry in {:?}",
                    fn_name, attempt, max_attempts, msg, delay
                );
                tokio::time::sleep(delay).await;
                attempt = next_attempt;
            }
        }
    }
}

// ============================================================
// Registry unit tests.
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron_jobs::{BackoffKind, RetryConfig};

    fn test_store(url: &str) -> Arc<DbConnHandle> {
        Arc::new(DbConnHandle::new_for_test_closed(url.to_string()))
    }

    #[test]
    fn register_and_entry_roundtrip() {
        let reg = BackgroundRegistry::new();
        assert!(reg.is_empty());
        reg.register(
            "send_email".into(),
            test_store("postgres://x/db"),
            None,
            false,
        );
        assert!(!reg.is_empty());
        let e = reg.entry("send_email").expect("registered");
        assert_eq!(e.store.url_redacted, "postgres://x/db");
        assert!(e.retry.is_none());
        assert!(!e.catch_up);
        assert!(reg.entry("nope").is_none());
    }

    #[test]
    fn register_overwrites_same_name() {
        let reg = BackgroundRegistry::new();
        reg.register("w".into(), test_store("postgres://a/db"), None, false);
        reg.register("w".into(), test_store("postgres://b/db"), None, true);
        let e = reg.entry("w").unwrap();
        assert_eq!(e.store.url_redacted, "postgres://b/db");
        assert!(e.catch_up);
    }

    #[test]
    fn catch_up_stores_dedups_by_url_and_filters_flag() {
        let reg = BackgroundRegistry::new();
        // Two fns, same DB, both catch_up -> one store.
        reg.register("a".into(), test_store("postgres://same/db"), None, true);
        reg.register("b".into(), test_store("postgres://same/db"), None, true);
        // A third fn with catch_up=false -> excluded.
        reg.register("c".into(), test_store("postgres://other/db"), None, false);
        let stores = reg.catch_up_stores();
        assert_eq!(stores.len(), 1, "expected dedup to one store: {:?}", stores);
        assert_eq!(stores[0].url_redacted, "postgres://same/db");
    }

    #[test]
    fn catch_up_stores_keeps_distinct_dbs() {
        let reg = BackgroundRegistry::new();
        reg.register("a".into(), test_store("postgres://one/db"), None, true);
        reg.register("b".into(), test_store("postgres://two/db"), None, true);
        let mut urls: Vec<String> = reg
            .catch_up_stores()
            .iter()
            .map(|s| s.url_redacted.clone())
            .collect();
        urls.sort();
        assert_eq!(urls, vec!["postgres://one/db", "postgres://two/db"]);
    }

    #[test]
    fn entry_carries_retry_config() {
        let reg = BackgroundRegistry::new();
        let retry = RetryConfig {
            max: 3,
            backoff: BackoffKind::Exponential,
            initial_secs: 1,
            max_secs: 30,
        };
        reg.register("w".into(), test_store("postgres://x/db"), Some(retry), true);
        let e = reg.entry("w").unwrap();
        assert_eq!(e.retry, Some(retry));
        assert!(e.catch_up);
    }
}
