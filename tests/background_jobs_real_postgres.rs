//! v0.37.7 — E2E tests for the `@background(store=db)` persistence SQL
//! helpers against a real Postgres on localhost. Marked `#[ignore]` by
//! default — run only with:
//!
//!   $env:FITZ_TEST_PG_URL = "postgres://postgres:secret@localhost:5432/postgres?sslmode=disable"
//!   cargo test --test background_jobs_real_postgres -- --ignored
//!
//! Validate the SQL helpers `init_bg_storage` / `insert_bg_job_running`
//! / `update_bg_job_finish` / `update_bg_job_retrying` /
//! `mark_orphaned_failed` against the real wire protocol (no mocks).
//! The full spawn loop (timing-dependent) is covered by the runnable
//! example `30c-background-persistente.fitz` + the manual smoke.
//!
//! Cleanup: each test deletes the whole `fitz_bg_jobs` table before and
//! after so tests are idempotent (the table is process-global, single
//! DB target).

use fitz::background_jobs::{
    ensure_bg_storage_initialized, init_bg_storage, insert_bg_job_running, mark_orphaned_failed,
    update_bg_job_finish, update_bg_job_retrying,
};
use fitz::db::{connect_url, DbConnHandle, PgValue};
use std::sync::Arc;

fn pg_url() -> String {
    std::env::var("FITZ_TEST_PG_URL").unwrap_or_else(|_| {
        panic!(
            "E2E tests for @background persistence require the env var\n  \
             FITZ_TEST_PG_URL=\"postgres://user:pass@localhost:5432/db?sslmode=disable\"\n\
             Set it before running with `--ignored`."
        )
    })
}

/// Wipes the whole `fitz_bg_jobs` table. Idempotent — if the table does
/// not exist yet, the DELETE fails silently (suppressed). Each test
/// clears at start and end.
async fn cleanup(conn: &Arc<DbConnHandle>) {
    let _ = conn.exec("DELETE FROM fitz_bg_jobs", &[]).await;
}

async fn count_status(conn: &Arc<DbConnHandle>, status: &str) -> i64 {
    let res = conn
        .query(
            "SELECT count(*) FROM fitz_bg_jobs WHERE status = $1",
            &[PgValue::Text(status.to_string())],
        )
        .await
        .expect("count query");
    match res.rows.first().and_then(|r| r.get_at(0)) {
        Some(PgValue::Int(n)) => *n,
        Some(PgValue::Text(s)) => s.parse().unwrap_or(-1),
        _ => -1,
    }
}

#[tokio::test]
#[ignore]
async fn init_bg_storage_creates_table_and_is_idempotent() {
    let conn = connect_url(&pg_url()).await.expect("connect");
    // Two runs must not error (CREATE TABLE IF NOT EXISTS).
    init_bg_storage(&conn).await.expect("first init");
    init_bg_storage(&conn)
        .await
        .expect("second init idempotent");
    let res = conn
        .query(
            "SELECT count(*) FROM information_schema.tables WHERE table_name = 'fitz_bg_jobs'",
            &[],
        )
        .await
        .expect("table check");
    let present = matches!(res.rows.first().and_then(|r| r.get_at(0)), Some(PgValue::Int(n)) if *n >= 1)
        || matches!(res.rows.first().and_then(|r| r.get_at(0)), Some(PgValue::Text(s)) if s.parse::<i64>().unwrap_or(0) >= 1);
    assert!(present, "fitz_bg_jobs table should exist after init");
    conn.close().await.ok();
}

#[tokio::test]
#[ignore]
async fn insert_running_then_finish_ok_persists_row() {
    let conn = connect_url(&pg_url()).await.expect("connect");
    init_bg_storage(&conn).await.expect("init");
    cleanup(&conn).await;

    let id = insert_bg_job_running(&conn, "send_email", "[42,\"Welcome\"]")
        .await
        .expect("insert running");
    assert!(id > 0, "insert should return a positive BIGSERIAL id");

    update_bg_job_finish(&conn, id, "ok", None)
        .await
        .expect("finish ok");

    let res = conn
        .query(
            "SELECT fn_name, args_json, status, attempt, finished_at IS NOT NULL FROM fitz_bg_jobs WHERE id = $1",
            &[PgValue::Int(id)],
        )
        .await
        .expect("select row");
    let row = res.rows.first().expect("row present");
    assert!(matches!(row.get_at(0), Some(PgValue::Text(s)) if s == "send_email"));
    assert!(matches!(row.get_at(1), Some(PgValue::Text(s)) if s == "[42,\"Welcome\"]"));
    assert!(matches!(row.get_at(2), Some(PgValue::Text(s)) if s == "ok"));
    // attempt = 1
    let attempt_ok = matches!(row.get_at(3), Some(PgValue::Int(n)) if *n == 1)
        || matches!(row.get_at(3), Some(PgValue::Text(s)) if s == "1");
    assert!(attempt_ok, "attempt should be 1");
    // finished_at not null
    assert!(
        matches!(row.get_at(4), Some(PgValue::Bool(true)))
            || matches!(row.get_at(4), Some(PgValue::Text(s)) if s == "t" || s == "true")
    );

    cleanup(&conn).await;
    conn.close().await.ok();
}

#[tokio::test]
#[ignore]
async fn update_retrying_bumps_attempt_and_status() {
    let conn = connect_url(&pg_url()).await.expect("connect");
    init_bg_storage(&conn).await.expect("init");
    cleanup(&conn).await;

    let id = insert_bg_job_running(&conn, "flaky", "[5]")
        .await
        .expect("insert");
    update_bg_job_retrying(&conn, id, 2, Some("boom"))
        .await
        .expect("retrying");

    let res = conn
        .query(
            "SELECT status, attempt, error FROM fitz_bg_jobs WHERE id = $1",
            &[PgValue::Int(id)],
        )
        .await
        .expect("select");
    let row = res.rows.first().expect("row");
    assert!(matches!(row.get_at(0), Some(PgValue::Text(s)) if s == "retrying"));
    let attempt_2 = matches!(row.get_at(1), Some(PgValue::Int(n)) if *n == 2)
        || matches!(row.get_at(1), Some(PgValue::Text(s)) if s == "2");
    assert!(attempt_2, "attempt should be 2 after retrying");
    assert!(matches!(row.get_at(2), Some(PgValue::Text(s)) if s == "boom"));

    // Then a terminal 'failed'.
    update_bg_job_finish(&conn, id, "failed", Some("boom final"))
        .await
        .expect("failed");
    let res2 = conn
        .query(
            "SELECT status, error FROM fitz_bg_jobs WHERE id = $1",
            &[PgValue::Int(id)],
        )
        .await
        .expect("select2");
    let row2 = res2.rows.first().expect("row2");
    assert!(matches!(row2.get_at(0), Some(PgValue::Text(s)) if s == "failed"));
    assert!(matches!(row2.get_at(1), Some(PgValue::Text(s)) if s == "boom final"));

    cleanup(&conn).await;
    conn.close().await.ok();
}

#[tokio::test]
#[ignore]
async fn mark_orphaned_flips_running_and_retrying_to_failed() {
    let conn = connect_url(&pg_url()).await.expect("connect");
    init_bg_storage(&conn).await.expect("init");
    cleanup(&conn).await;

    // Insert one 'running' (via helper) + one 'retrying' + one 'ok'
    // (should NOT be touched).
    let _running = insert_bg_job_running(&conn, "orphan_a", "[1]")
        .await
        .expect("insert running");
    let retrying_id = insert_bg_job_running(&conn, "orphan_b", "[2]")
        .await
        .expect("insert b");
    update_bg_job_retrying(&conn, retrying_id, 2, Some("mid"))
        .await
        .expect("to retrying");
    let ok_id = insert_bg_job_running(&conn, "done", "[3]")
        .await
        .expect("insert ok");
    update_bg_job_finish(&conn, ok_id, "ok", None)
        .await
        .expect("finish ok");

    let n = mark_orphaned_failed(&conn).await.expect("mark orphaned");
    assert_eq!(n, 2, "should flip exactly the running + retrying rows");

    assert_eq!(count_status(&conn, "failed").await, 2);
    assert_eq!(count_status(&conn, "ok").await, 1);
    assert_eq!(count_status(&conn, "running").await, 0);
    assert_eq!(count_status(&conn, "retrying").await, 0);

    // The flipped rows carry the orphaned error message.
    let res = conn
        .query(
            "SELECT count(*) FROM fitz_bg_jobs WHERE status = 'failed' AND error = 'orphaned by restart'",
            &[],
        )
        .await
        .expect("orphan error check");
    let orphaned = match res.rows.first().and_then(|r| r.get_at(0)) {
        Some(PgValue::Int(n)) => *n,
        Some(PgValue::Text(s)) => s.parse().unwrap_or(-1),
        _ => -1,
    };
    assert_eq!(orphaned, 2);

    cleanup(&conn).await;
    conn.close().await.ok();
}

#[tokio::test]
#[ignore]
async fn ensure_bg_storage_initialized_is_safe_under_parallel() {
    let conn = connect_url(&pg_url()).await.expect("connect");
    fitz::background_jobs::reset_init_bg_storage_once_for_tests();
    // Drop the table so the init actually has to CREATE it.
    let _ = conn.exec("DROP TABLE IF EXISTS fitz_bg_jobs", &[]).await;

    let mut handles = Vec::new();
    for _ in 0..10 {
        let c = conn.clone();
        handles.push(tokio::spawn(async move {
            ensure_bg_storage_initialized(&c).await
        }));
    }
    let mut failures = 0;
    for h in handles {
        match h.await.expect("join") {
            Ok(()) => {}
            Err(_) => failures += 1,
        }
    }
    assert_eq!(failures, 0, "no init should fail under 10 parallel callers");

    let res = conn
        .query(
            "SELECT count(*) FROM information_schema.tables WHERE table_name = 'fitz_bg_jobs'",
            &[],
        )
        .await
        .expect("table check");
    let present = matches!(res.rows.first().and_then(|r| r.get_at(0)), Some(PgValue::Int(n)) if *n >= 1)
        || matches!(res.rows.first().and_then(|r| r.get_at(0)), Some(PgValue::Text(s)) if s.parse::<i64>().unwrap_or(0) >= 1);
    assert!(present, "table should exist after parallel init");

    cleanup(&conn).await;
    conn.close().await.ok();
}
