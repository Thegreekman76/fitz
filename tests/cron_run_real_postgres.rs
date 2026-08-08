//! v0.37.3 — Tests E2E del path `fitz run` con `@cron(store=db)`
//! persistente contra un Postgres real. Marcados `#[ignore]` por
//! default — solo corren con:
//!
//!   $env:FITZ_TEST_PG_URL = "postgres://postgres:pass@localhost:5432/postgres?sslmode=disable"
//!   cargo test --test cron_run_real_postgres -- --ignored --test-threads=1
//!
//! A diferencia de `cron_jobs_real_postgres.rs` (que ejercita los
//! helpers SQL con UNA sola runtime tokio via `#[tokio::test]`), estos
//! tests spawnean el binario `fitz` real (`fitz run <programa>`) para
//! reproducir el bug de dos runtimes: pre-v0.37.3 el eval abría la
//! conexión DB en un runtime `current_thread` que se dropeaba, y el
//! scheduler —en un runtime `multi_thread` posterior— fallaba la
//! primera query (`CREATE TABLE fitz_cron_jobs`) con "A Tokio 1.x
//! context was found, but it is being shutdown". El fix unifica ambos
//! en UN runtime compartido; estos tests confirman que las tablas se
//! crean y se persisten runs `ok` en AMBOS modos (cron-only y
//! HTTP+cron).
//!
//! Ejecución: `--test-threads=1` recomendado (cada test spawnea un
//! proceso que abre conexiones + bindea un puerto en el caso HTTP).

use fitz::db::{connect_url, PgValue};
use std::io::Write;
use std::process::Command;
use std::time::Duration;

fn pg_url() -> String {
    std::env::var("FITZ_TEST_PG_URL").unwrap_or_else(|_| {
        panic!(
            "Tests E2E de `fitz run` cron+store requieren la env var\n  \
             FITZ_TEST_PG_URL=\"postgres://user:pass@localhost:5432/db?sslmode=disable\"\n\
             Seteala antes de correr con `--ignored`."
        )
    })
}

/// Borra las filas del job-name en ambas tablas (idempotente — si las
/// tablas no existen aún, los DELETE fallan silenciosamente).
async fn cleanup_job(url: &str, name: &str) {
    let Ok(conn) = connect_url(url).await else {
        return;
    };
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
    let _ = conn.close().await;
}

/// Cuenta las filas `status = 'ok'` de un job en `fitz_cron_runs`.
async fn count_ok_runs(url: &str, name: &str) -> i64 {
    let conn = connect_url(url).await.expect("connect para verificar");
    let qr = conn
        .query(
            "SELECT COUNT(*) AS n FROM fitz_cron_runs WHERE job_name = $1 AND status = 'ok'",
            &[PgValue::Text(name.to_string())],
        )
        .await
        .expect("query count runs");
    let n = match qr.rows[0].get("n") {
        Some(PgValue::Int(n)) => *n,
        other => panic!("COUNT(*) inesperado: {:?}", other),
    };
    let _ = conn.close().await;
    n
}

/// Escribe el programa a un `.fitz` temporal, spawnea `fitz run`,
/// espera `run_secs`, mata el proceso, y devuelve control. El binario
/// hereda `DATABASE_URL` = `pg_url()`.
fn run_fitz_program_for(program: &str, file_stem: &str, run_secs: u64) {
    let path = std::env::temp_dir().join(format!("{}.fitz", file_stem));
    {
        let mut f = std::fs::File::create(&path).expect("crear .fitz temporal");
        f.write_all(program.as_bytes()).expect("escribir programa");
    }

    let bin = env!("CARGO_BIN_EXE_fitz");
    let mut child = Command::new(bin)
        .arg("run")
        .arg(&path)
        .env("DATABASE_URL", pg_url())
        // Silenciamos el auto access-log / prints del cron para no
        // ensuciar la salida del test; los runs igual se persisten.
        .env("RUST_LOG", "error")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawnear `fitz run`");

    // Dejamos correr el scheduler: `*/2 * * * * *` = cada 2s. Con
    // ~8s cosechamos varios ticks incluso contando el arranque del
    // intérprete + la resolución del primer schedule.
    std::thread::sleep(Duration::from_secs(run_secs));

    // Matamos el proceso (el cron `fitz run` bloquea hasta SIGINT).
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
#[ignore]
async fn cron_only_run_persists_via_shared_runtime_v0_37_3() {
    let url = pg_url();
    let job = "cron_run_e2e_only";
    cleanup_job(&url, job).await;

    // Modo cron-only: NO hay `@server` ni handlers HTTP. Pre-fix, el
    // scheduler standalone (`run_scheduler_only`) construía un segundo
    // runtime y la primera query del cron rompía.
    let program = format!(
        r#"let db = db.connect(env_or("DATABASE_URL", "{url}")).await

@cron("*/2 * * * * *", store=db)
async fn {job}() -> Result<Null> {{
    return Ok(null)
}}
"#,
        url = url,
        job = job,
    );

    // El sleep + spawn son bloqueantes; los corremos en un thread
    // blocking para no clavar el runtime del test.
    let program_owned = program.clone();
    tokio::task::spawn_blocking(move || {
        run_fitz_program_for(&program_owned, "fitz_cron_run_only", 8)
    })
    .await
    .expect("blocking task no paniquea");

    let ok_runs = count_ok_runs(&url, job).await;
    let result = ok_runs;
    cleanup_job(&url, job).await;
    assert!(
        result >= 1,
        "esperaba >= 1 run 'ok' persistido en modo cron-only, fue {} \
         (el bug de dos runtimes habría dejado 0 — las tablas ni se crean)",
        result
    );
}

#[tokio::test]
#[ignore]
async fn http_plus_cron_run_persists_via_shared_runtime_v0_37_3() {
    let url = pg_url();
    let job = "cron_run_e2e_http";
    cleanup_job(&url, job).await;

    // Modo HTTP+cron: hay `@server` + un handler. Pre-fix, agregar un
    // handler HTTP NO evitaba el bug (`serve` también construía el
    // segundo runtime). Puerto alto para minimizar colisiones.
    let program = format!(
        r#"let db = db.connect(env_or("DATABASE_URL", "{url}")).await

@cron("*/2 * * * * *", store=db)
async fn {job}() -> Result<Null> {{
    return Ok(null)
}}

@get("/ping")
fn ping() -> Str {{
    return "pong"
}}

@server(43977, docs=false)
fn main() => 0
"#,
        url = url,
        job = job,
    );

    let program_owned = program.clone();
    tokio::task::spawn_blocking(move || {
        run_fitz_program_for(&program_owned, "fitz_cron_run_http", 8)
    })
    .await
    .expect("blocking task no paniquea");

    let ok_runs = count_ok_runs(&url, job).await;
    let result = ok_runs;
    cleanup_job(&url, job).await;
    assert!(
        result >= 1,
        "esperaba >= 1 run 'ok' persistido en modo HTTP+cron, fue {} \
         (el bug de dos runtimes habría dejado 0)",
        result
    );
}
