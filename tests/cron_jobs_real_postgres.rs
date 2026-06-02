//! 9.w.3.iter2.c — Tests E2E reales del scheduler de cron jobs
//! contra una instancia Postgres vivida en localhost. Marcados
//! `#[ignore]` por default — solo corren con:
//!
//!   $env:FITZ_TEST_PG_URL = "postgres://postgres:secret@localhost:5432/postgres?sslmode=disable"
//!   cargo test --test cron_jobs_real_postgres -- --ignored
//!
//! Validan los helpers SQL `init_storage` / `upsert_job_row` /
//! `record_run_start` / `record_run_finish` / `update_job_last_run`
//! / `read_last_run_at` contra el wire protocol real (sin mocks). No
//! testean el loop completo del scheduler (timing-dependent) — eso
//! lo cubre el ejemplo runnable `30b-cron-persistente.fitz` del
//! sub-paso `e`.
//!
//! Limpieza: cada test borra sus filas de `fitz_cron_jobs`/
//! `fitz_cron_runs` por `job_name` antes y después para idempotencia.

use fitz::cron_jobs::{
    init_storage, read_last_run_at, record_run_finish, record_run_start, update_job_last_run,
    upsert_job_row,
};
use fitz::db::{connect_url, DbConnHandle, PgValue};
use std::sync::Arc;

fn pg_url() -> String {
    std::env::var("FITZ_TEST_PG_URL").unwrap_or_else(|_| {
        panic!(
            "Tests E2E del cron scheduler requieren la env var\n  \
             FITZ_TEST_PG_URL=\"postgres://user:pass@localhost:5432/db?sslmode=disable\"\n\
             Seteala antes de correr con `--ignored`."
        )
    })
}

/// Limpia las filas del job-name dado en ambas tablas. Idempotente —
/// si las tablas no existen aún, los DELETE fallan silenciosamente
/// (suprimimos el error). Llamado por cada test al inicio.
async fn cleanup_job(conn: &Arc<DbConnHandle>, name: &str) {
    let _ = conn
        .exec(
            "DELETE FROM fitz_cron_runs WHERE job_name = $1",
            &[PgValue::Text(name.to_string())],
        )
        .await;
    let _ = conn
        .exec(
            "DELETE FROM fitz_cron_jobs WHERE name = $1",
            &[PgValue::Text(name.to_string())],
        )
        .await;
}

#[tokio::test]
#[ignore]
async fn iter2_init_storage_crea_las_tablas_y_es_idempotente() {
    let conn = connect_url(&pg_url()).await.expect("connect");
    init_storage(&conn).await.expect("primer init_storage");
    // Segunda vez: `CREATE TABLE IF NOT EXISTS` debe ser no-op sin
    // error (Postgres serializa internamente).
    init_storage(&conn).await.expect("segundo init_storage");
    // Verificamos que las dos tablas existen vía information_schema.
    let qr = conn
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name IN ('fitz_cron_jobs', 'fitz_cron_runs') \
             ORDER BY table_name",
            &[],
        )
        .await
        .expect("query information_schema");
    assert_eq!(qr.rows.len(), 2, "esperaba ambas tablas, fue {:?}", qr.rows);
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2_upsert_job_row_inserta_y_actualiza() {
    let conn = connect_url(&pg_url()).await.unwrap();
    init_storage(&conn).await.unwrap();
    cleanup_job(&conn, "iter2_upsert_test").await;

    // Primera vez: insert.
    upsert_job_row(&conn, "iter2_upsert_test", "0 0 * * *", "UTC")
        .await
        .expect("primer upsert");
    let qr = conn
        .query(
            "SELECT schedule, tz, last_run_at FROM fitz_cron_jobs WHERE name = $1",
            &[PgValue::Text("iter2_upsert_test".into())],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(
        qr.rows[0].get("schedule"),
        Some(&PgValue::Text("0 0 * * *".into()))
    );
    assert_eq!(qr.rows[0].get("tz"), Some(&PgValue::Text("UTC".into())));
    assert_eq!(qr.rows[0].get("last_run_at"), Some(&PgValue::Null));

    // Segunda vez con tz/schedule distinto: update via ON CONFLICT.
    upsert_job_row(
        &conn,
        "iter2_upsert_test",
        "0 3 * * *",
        "America/Argentina/Buenos_Aires",
    )
    .await
    .expect("segundo upsert");
    let qr = conn
        .query(
            "SELECT schedule, tz FROM fitz_cron_jobs WHERE name = $1",
            &[PgValue::Text("iter2_upsert_test".into())],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(
        qr.rows[0].get("schedule"),
        Some(&PgValue::Text("0 3 * * *".into()))
    );
    assert_eq!(
        qr.rows[0].get("tz"),
        Some(&PgValue::Text("America/Argentina/Buenos_Aires".into()))
    );

    cleanup_job(&conn, "iter2_upsert_test").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2_record_run_start_y_finish_persisten_status() {
    let conn = connect_url(&pg_url()).await.unwrap();
    init_storage(&conn).await.unwrap();
    cleanup_job(&conn, "iter2_run_test").await;
    upsert_job_row(&conn, "iter2_run_test", "0 * * * *", "UTC")
        .await
        .unwrap();

    // Attempt 1: start + finish ok.
    let run_id = record_run_start(&conn, "iter2_run_test", 1)
        .await
        .expect("record_run_start");
    assert!(run_id > 0);
    record_run_finish(&conn, run_id, "ok", None)
        .await
        .expect("record_run_finish");

    // Verificamos shape de la fila.
    let qr = conn
        .query(
            "SELECT job_name, attempt, status, error, finished_at \
             FROM fitz_cron_runs WHERE id = $1",
            &[PgValue::Int(run_id)],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    let row = &qr.rows[0];
    assert_eq!(
        row.get("job_name"),
        Some(&PgValue::Text("iter2_run_test".into()))
    );
    assert_eq!(row.get("attempt"), Some(&PgValue::Int(1)));
    assert_eq!(row.get("status"), Some(&PgValue::Text("ok".into())));
    assert_eq!(row.get("error"), Some(&PgValue::Null));
    // finished_at debe estar populated (no Null).
    assert_ne!(row.get("finished_at"), Some(&PgValue::Null));

    cleanup_job(&conn, "iter2_run_test").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2_record_run_finish_failed_lleva_error_message() {
    let conn = connect_url(&pg_url()).await.unwrap();
    init_storage(&conn).await.unwrap();
    cleanup_job(&conn, "iter2_fail_test").await;
    upsert_job_row(&conn, "iter2_fail_test", "0 * * * *", "UTC")
        .await
        .unwrap();

    let run_id = record_run_start(&conn, "iter2_fail_test", 1).await.unwrap();
    record_run_finish(&conn, run_id, "failed", Some("ConnectionRefused"))
        .await
        .unwrap();

    let qr = conn
        .query(
            "SELECT status, error FROM fitz_cron_runs WHERE id = $1",
            &[PgValue::Int(run_id)],
        )
        .await
        .unwrap();
    let row = &qr.rows[0];
    assert_eq!(row.get("status"), Some(&PgValue::Text("failed".into())));
    assert_eq!(
        row.get("error"),
        Some(&PgValue::Text("ConnectionRefused".into()))
    );

    cleanup_job(&conn, "iter2_fail_test").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2_update_job_last_run_se_refleja_en_read_last_run_at() {
    let conn = connect_url(&pg_url()).await.unwrap();
    init_storage(&conn).await.unwrap();
    cleanup_job(&conn, "iter2_last_run_test").await;
    upsert_job_row(&conn, "iter2_last_run_test", "0 * * * *", "UTC")
        .await
        .unwrap();

    // Antes del update: last_run_at = NULL.
    let before = read_last_run_at(&conn, "iter2_last_run_test")
        .await
        .unwrap();
    assert!(before.is_none(), "esperaba None inicial, fue {:?}", before);

    // Update con status ok.
    update_job_last_run(&conn, "iter2_last_run_test", "ok", None)
        .await
        .unwrap();
    let after = read_last_run_at(&conn, "iter2_last_run_test")
        .await
        .unwrap();
    assert!(
        after.is_some(),
        "esperaba Some(dt) tras update, fue {:?}",
        after
    );
    // El timestamp debería estar cerca de "now" — toleramos ±60s.
    let dt = after.unwrap();
    let now = chrono::Utc::now();
    let diff = (now - dt).num_seconds().abs();
    assert!(
        diff < 60,
        "esperaba last_run_at cerca de now (diff < 60s), fue diff = {}s",
        diff
    );

    // Update con status failed + error: debe actualizar todos los campos.
    update_job_last_run(
        &conn,
        "iter2_last_run_test",
        "failed",
        Some("timeout en upstream"),
    )
    .await
    .unwrap();
    let qr = conn
        .query(
            "SELECT last_status, last_error FROM fitz_cron_jobs WHERE name = $1",
            &[PgValue::Text("iter2_last_run_test".into())],
        )
        .await
        .unwrap();
    let row = &qr.rows[0];
    assert_eq!(
        row.get("last_status"),
        Some(&PgValue::Text("failed".into()))
    );
    assert_eq!(
        row.get("last_error"),
        Some(&PgValue::Text("timeout en upstream".into()))
    );

    cleanup_job(&conn, "iter2_last_run_test").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2_read_last_run_at_para_job_inexistente_es_none() {
    let conn = connect_url(&pg_url()).await.unwrap();
    init_storage(&conn).await.unwrap();
    cleanup_job(&conn, "iter2_inexistente").await;

    let res = read_last_run_at(&conn, "iter2_inexistente")
        .await
        .expect("query OK aunque no haya fila");
    assert!(res.is_none(), "esperaba None, fue {:?}", res);

    conn.close().await.unwrap();
}
