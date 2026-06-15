//! 9.w.1.iter2.b — Tests E2E reales del schema + queries de
//! `fitz_token_blacklist` contra una instancia Postgres viva en
//! localhost. Marcados `#[ignore]` por default — solo corren con:
//!
//!   $env:FITZ_TEST_PG_URL = "postgres://postgres:secret@localhost:5432/postgres?sslmode=disable"
//!   cargo test --test auth_blacklist_real_postgres -- --ignored
//!
//! Validan que la tabla `fitz_token_blacklist` se auto-crea al primer
//! call y que los SQL emitidos por los builtins `auth.*` funcionan
//! end-to-end contra el wire protocol real (sin mocks ni evaluator).
//! La validación de shape sintáctico (args, tipos) está cubierta por
//! unit tests en `src/evaluator.rs`.
//!
//! Limpieza: cada test borra sus filas de `fitz_token_blacklist` por
//! `jti` antes y después para idempotencia.

use fitz::db::{connect_url, DbConnHandle, PgValue};
use fitz::evaluator::{
    ensure_token_blacklist_table, SQL_CLEANUP_EXPIRED_TOKENS, SQL_INSERT_TOKEN_BLACKLIST,
    SQL_IS_TOKEN_BLACKLISTED,
};
use std::sync::Arc;

fn pg_url() -> String {
    std::env::var("FITZ_TEST_PG_URL").unwrap_or_else(|_| {
        panic!(
            "Tests E2E de auth.blacklist requieren la env var\n  \
             FITZ_TEST_PG_URL=\"postgres://user:pass@localhost:5432/db?sslmode=disable\"\n\
             Seteala antes de correr con `--ignored`."
        )
    })
}

/// Limpia las filas para los jti dados. Idempotente.
async fn cleanup_jti(conn: &Arc<DbConnHandle>, jti: &str) {
    let _ = conn
        .exec(
            "DELETE FROM fitz_token_blacklist WHERE jti = $1",
            &[PgValue::Text(jti.to_string())],
        )
        .await;
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
#[ignore]
async fn iter2b_blacklist_jti_then_is_blacklisted_returns_true() {
    let conn = connect_url(&pg_url()).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();
    cleanup_jti(&conn, "iter2b-test-jti-1").await;

    let expires_at = now_secs() + 3600;

    conn.exec(
        SQL_INSERT_TOKEN_BLACKLIST,
        &[
            PgValue::Text("iter2b-test-jti-1".to_string()),
            PgValue::Int(expires_at),
        ],
    )
    .await
    .unwrap();

    let qr = conn
        .query(
            SQL_IS_TOKEN_BLACKLISTED,
            &[PgValue::Text("iter2b-test-jti-1".to_string())],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    let blacklisted = matches!(qr.rows[0].values()[0], PgValue::Bool(true));
    assert!(blacklisted);

    cleanup_jti(&conn, "iter2b-test-jti-1").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2b_is_blacklisted_for_nonexistent_jti_returns_false() {
    let conn = connect_url(&pg_url()).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();
    cleanup_jti(&conn, "iter2b-jti-no-existe").await;

    let qr = conn
        .query(
            SQL_IS_TOKEN_BLACKLISTED,
            &[PgValue::Text("iter2b-jti-no-existe".to_string())],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    let blacklisted = matches!(qr.rows[0].values()[0], PgValue::Bool(true));
    assert!(!blacklisted);

    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2b_is_blacklisted_with_past_expires_at_returns_false() {
    let conn = connect_url(&pg_url()).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();
    cleanup_jti(&conn, "iter2b-jti-expirado").await;

    let past = now_secs() - 7200;

    conn.exec(
        SQL_INSERT_TOKEN_BLACKLIST,
        &[
            PgValue::Text("iter2b-jti-expirado".to_string()),
            PgValue::Int(past),
        ],
    )
    .await
    .unwrap();

    let qr = conn
        .query(
            SQL_IS_TOKEN_BLACKLISTED,
            &[PgValue::Text("iter2b-jti-expirado".to_string())],
        )
        .await
        .unwrap();
    let blacklisted = matches!(qr.rows[0].values()[0], PgValue::Bool(true));
    assert!(
        !blacklisted,
        "expires_at pasado debe contar como NO blacklisted"
    );

    cleanup_jti(&conn, "iter2b-jti-expirado").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2b_cleanup_expired_deletes_only_expired_rows() {
    let conn = connect_url(&pg_url()).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();
    cleanup_jti(&conn, "iter2b-cleanup-vivo").await;
    cleanup_jti(&conn, "iter2b-cleanup-muerto").await;

    let now = now_secs();
    let future = now + 3600;
    let past = now - 7200;

    conn.exec(
        SQL_INSERT_TOKEN_BLACKLIST,
        &[
            PgValue::Text("iter2b-cleanup-vivo".to_string()),
            PgValue::Int(future),
        ],
    )
    .await
    .unwrap();
    conn.exec(
        SQL_INSERT_TOKEN_BLACKLIST,
        &[
            PgValue::Text("iter2b-cleanup-muerto".to_string()),
            PgValue::Int(past),
        ],
    )
    .await
    .unwrap();

    let deleted = conn.exec(SQL_CLEANUP_EXPIRED_TOKENS, &[]).await.unwrap();
    assert!(
        deleted >= 1,
        "esperaba al menos 1 row borrada, fue {}",
        deleted
    );

    let qr_vivo = conn
        .query(
            "SELECT jti FROM fitz_token_blacklist WHERE jti = $1",
            &[PgValue::Text("iter2b-cleanup-vivo".to_string())],
        )
        .await
        .unwrap();
    assert_eq!(qr_vivo.rows.len(), 1, "the live jti should remain");

    let qr_muerto = conn
        .query(
            "SELECT jti FROM fitz_token_blacklist WHERE jti = $1",
            &[PgValue::Text("iter2b-cleanup-muerto".to_string())],
        )
        .await
        .unwrap();
    assert_eq!(qr_muerto.rows.len(), 0, "el jti vencido debe estar borrado");

    cleanup_jti(&conn, "iter2b-cleanup-vivo").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2b_blacklist_re_blacklist_same_jti_updates_expires_at() {
    // ON CONFLICT (jti) DO UPDATE SET expires_at = EXCLUDED.expires_at:
    // si re-blacklisteás el mismo jti, el expires_at se actualiza sin
    // fallar con duplicate key.
    let conn = connect_url(&pg_url()).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();
    cleanup_jti(&conn, "iter2b-jti-doble").await;

    let now = now_secs();

    conn.exec(
        SQL_INSERT_TOKEN_BLACKLIST,
        &[
            PgValue::Text("iter2b-jti-doble".to_string()),
            PgValue::Int(now + 100),
        ],
    )
    .await
    .unwrap();

    // Re-INSERT con expires_at distinto — ON CONFLICT updatea.
    conn.exec(
        SQL_INSERT_TOKEN_BLACKLIST,
        &[
            PgValue::Text("iter2b-jti-doble".to_string()),
            PgValue::Int(now + 3600),
        ],
    )
    .await
    .unwrap();

    // Verificá que solo hay 1 row con el valor final.
    let qr = conn
        .query(
            "SELECT expires_at FROM fitz_token_blacklist WHERE jti = $1",
            &[PgValue::Text("iter2b-jti-doble".to_string())],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    match qr.rows[0].values().first() {
        Some(PgValue::Int(n)) => {
            assert_eq!(*n, now + 3600, "expires_at must match the last update");
        }
        other => panic!("expected Int, received {:?}", other),
    }

    cleanup_jti(&conn, "iter2b-jti-doble").await;
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn iter2b_ensure_token_blacklist_table_is_idempotent() {
    // Llamar `ensure_token_blacklist_table` dos veces no debe fallar
    // (CREATE TABLE IF NOT EXISTS).
    let conn = connect_url(&pg_url()).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();
    ensure_token_blacklist_table(&conn).await.unwrap();

    conn.close().await.unwrap();
}
