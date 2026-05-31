//! Tests E2E reales del driver Postgres contra una instancia
//! vivida en localhost. Marcados `#[ignore]` por default — solo
//! corren con `cargo test --test db_real_postgres -- --ignored`
//! (o `--include-ignored` para mezclar con la suite normal).
//!
//! Requieren la env var `FITZ_TEST_PG_URL` apuntando a un Postgres
//! 14+ con la creds embebidas. Ejemplo PowerShell:
//!
//!   $env:FITZ_TEST_PG_URL = "postgres://postgres:secret@localhost/postgres?sslmode=disable"
//!   cargo test --test db_real_postgres -- --ignored
//!
//! En CI estos tests quedan `ignored` por default — no rompen el
//! pipeline cuando no hay Postgres. Para activarlos en CI hay que
//! orchestrar Postgres en docker (ver `docker-compose.test.yml` o
//! el job de release que arme el container — sub-paso futuro).
//!
//! Cada test usa una tabla temporal con nombre derivado del
//! nombre del test + timestamp para que múltiples corridas no
//! se pisen (y la limpieza se haga sola si el test paniquea —
//! `CREATE TEMP TABLE` el server la borra al cerrar la conexión).

use fitz::db::{connect_url, PgValue};
use fitz::evaluator::{eval_program_with_env, new_repl_env};
use fitz::lexer::tokenize;
use fitz::parser::parse;
use fitz::value::Value;

/// Lee `FITZ_TEST_PG_URL` o panics con mensaje claro de cómo
/// setearla. Llamado por cada test al inicio.
fn pg_url() -> String {
    std::env::var("FITZ_TEST_PG_URL").unwrap_or_else(|_| {
        panic!(
            "Tests E2E del driver Postgres requieren la env var\n  FITZ_TEST_PG_URL=\"postgres://user:pass@localhost/db?sslmode=disable\"\nSeteala antes de correr con `--ignored`."
        )
    })
}

#[tokio::test]
#[ignore]
async fn connect_y_select_uno() {
    let url = pg_url();
    let conn = connect_url(&url).await.expect("connect debería funcionar");
    let qr = conn
        .query("SELECT 1 AS n", &[])
        .await
        .expect("SELECT 1 debería funcionar");
    assert_eq!(qr.rows.len(), 1);
    let row = &qr.rows[0];
    let n = row.get("n").expect("columna `n` debería existir");
    assert_eq!(n, &PgValue::Int(1));
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn select_con_args_parametrizados() {
    // Extended Query Protocol con $1/$2.
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();
    let qr = conn
        .query(
            "SELECT $1::int AS a, $2::text AS b",
            &[PgValue::Int(42), PgValue::Text("hola".into())],
        )
        .await
        .expect("query con args debería funcionar");
    assert_eq!(qr.rows.len(), 1);
    let row = &qr.rows[0];
    assert_eq!(row.get("a"), Some(&PgValue::Int(42)));
    assert_eq!(row.get("b"), Some(&PgValue::Text("hola".into())));
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn create_temp_insert_select_full_cycle() {
    // CREATE TEMP TABLE + INSERT + SELECT. La tabla temp se borra
    // al cerrar la conexión (cleanup automático).
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();

    conn.query(
        "CREATE TEMP TABLE fitz_test_users (id bigserial PRIMARY KEY, name text, age int)",
        &[],
    )
    .await
    .expect("CREATE TEMP TABLE debería funcionar");

    let n_inserted = conn
        .exec(
            "INSERT INTO fitz_test_users (name, age) VALUES ($1, $2), ($3, $4)",
            &[
                PgValue::Text("ada".into()),
                PgValue::Int(30),
                PgValue::Text("alan".into()),
                PgValue::Int(42),
            ],
        )
        .await
        .expect("INSERT debería funcionar");
    assert_eq!(n_inserted, 2);

    let qr = conn
        .query(
            "SELECT name, age FROM fitz_test_users WHERE age > $1 ORDER BY age",
            &[PgValue::Int(25)],
        )
        .await
        .expect("SELECT debería funcionar");
    assert_eq!(qr.rows.len(), 2);
    assert_eq!(qr.rows[0].get("name"), Some(&PgValue::Text("ada".into())));
    assert_eq!(qr.rows[0].get("age"), Some(&PgValue::Int(30)));
    assert_eq!(qr.rows[1].get("name"), Some(&PgValue::Text("alan".into())));
    assert_eq!(qr.rows[1].get("age"), Some(&PgValue::Int(42)));

    let n_deleted = conn
        .exec(
            "DELETE FROM fitz_test_users WHERE age < $1",
            &[PgValue::Int(40)],
        )
        .await
        .expect("DELETE debería funcionar");
    assert_eq!(n_deleted, 1);

    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn tipos_oid_core_round_trip() {
    // Valida marshaling de los 11 tipos OID del MVP. Cada uno
    // pasa como arg de un cast `$1::tipo` y vuelve como Postgres
    // lo serializa en text format.
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();

    // Int4 / Int8
    let qr = conn
        .query(
            "SELECT $1::int4 AS i4, $2::int8 AS i8",
            &[PgValue::Int(42), PgValue::Int(i64::MIN)],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows[0].get("i4"), Some(&PgValue::Int(42)));
    assert_eq!(qr.rows[0].get("i8"), Some(&PgValue::Int(i64::MIN)));

    // Float4 / Float8
    let qr = conn
        .query(
            "SELECT $1::float8 AS f, $2::float4 AS f4",
            &[PgValue::Float(2.5), PgValue::Float(1.5)],
        )
        .await
        .unwrap();
    match qr.rows[0].get("f") {
        Some(PgValue::Float(x)) => assert!((x - 2.5).abs() < 1e-9),
        other => panic!("esperaba Float(2.5), fue {:?}", other),
    }

    // Text / Varchar
    let qr = conn
        .query(
            "SELECT $1::text AS t, $2::varchar AS v",
            &[PgValue::Text("hola".into()), PgValue::Text("mundo".into())],
        )
        .await
        .unwrap();
    assert_eq!(qr.rows[0].get("t"), Some(&PgValue::Text("hola".into())));
    assert_eq!(qr.rows[0].get("v"), Some(&PgValue::Text("mundo".into())));

    // Bool
    let qr = conn
        .query("SELECT $1::bool AS b", &[PgValue::Bool(true)])
        .await
        .unwrap();
    assert_eq!(qr.rows[0].get("b"), Some(&PgValue::Bool(true)));

    // Null
    let qr = conn
        .query("SELECT $1::int AS n", &[PgValue::Null])
        .await
        .unwrap();
    assert_eq!(qr.rows[0].get("n"), Some(&PgValue::Null));

    // Bytea
    let qr = conn
        .query(
            "SELECT $1::bytea AS bx",
            &[PgValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef])],
        )
        .await
        .unwrap();
    assert_eq!(
        qr.rows[0].get("bx"),
        Some(&PgValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]))
    );

    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn error_response_parsea_mensaje_del_servidor() {
    // Query sintácticamente invalida → ErrorResponse Postgres.
    // El driver lo traduce a `DbError::Server { severity, code,
    // message }`, y `Display` lo formatea como `<severity>:
    // <message>`. Validamos que el flow no rompe la conn (el
    // próximo query funciona).
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();

    let r = conn
        .query("SELECT * FROM tabla_inexistente_xyz_abc", &[])
        .await;
    assert!(r.is_err(), "esperaba Err");
    let msg = format!("{}", r.unwrap_err());
    assert!(
        msg.contains("ERROR") || msg.to_lowercase().contains("relation"),
        "esperaba mensaje del error de relation, fue: {}",
        msg
    );

    // La conn sigue usable después del error.
    let qr = conn
        .query("SELECT 1", &[])
        .await
        .expect("conn debería seguir usable tras ErrorResponse");
    assert_eq!(qr.rows.len(), 1);
    conn.close().await.unwrap();
}

// =================================================================
// Fase 10.7 — Transactions ORM (v0.10.14)
// =================================================================
// Tests E2E de `DbConnHandle::transaction` contra Postgres real.
// Cubren los 3 casos canónicos:
//   1. Happy path — callback retorna Ok, COMMIT, todo persiste.
//   2. Rollback explícito — callback retorna Err, ROLLBACK, nada
//      persiste.
//   3. Conn vuelve al pool — después de tx OK o tx Err, la conn
//      vuelve al pool y se puede reusar para queries siguientes.

#[tokio::test]
#[ignore]
async fn tx_happy_path_commit_persiste() {
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();
    // Setup: tabla limpia.
    conn.exec(
        "DROP TABLE IF EXISTS fitz_tx_test; \
         CREATE TABLE fitz_tx_test (id bigserial PRIMARY KEY, name text)",
        &[],
    )
    .await
    .expect("setup tabla");

    let n = conn
        .transaction(|tx| async move {
            tx.exec(
                "INSERT INTO fitz_tx_test (name) VALUES ($1)",
                &[PgValue::Text("ada".into())],
            )
            .await?;
            tx.exec(
                "INSERT INTO fitz_tx_test (name) VALUES ($1)",
                &[PgValue::Text("alan".into())],
            )
            .await?;
            Ok(2)
        })
        .await
        .expect("tx debería commitear");
    assert_eq!(n, 2);

    // Validar que los 2 rows persisten post-COMMIT.
    let qr = conn
        .query("SELECT count(*) AS n FROM fitz_tx_test", &[])
        .await
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    let count = qr.rows[0].get("n").unwrap();
    assert_eq!(count, &PgValue::Int(2), "esperaba 2 rows post-COMMIT");

    conn.exec("DROP TABLE fitz_tx_test", &[]).await.unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn tx_rollback_explicito_nada_persiste() {
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();
    conn.exec(
        "DROP TABLE IF EXISTS fitz_tx_rb; \
         CREATE TABLE fitz_tx_rb (id bigserial PRIMARY KEY, name text)",
        &[],
    )
    .await
    .expect("setup tabla");

    let r: fitz::db::DbResult<()> = conn
        .transaction(|tx| async move {
            tx.exec(
                "INSERT INTO fitz_tx_rb (name) VALUES ($1)",
                &[PgValue::Text("never-commit".into())],
            )
            .await?;
            // Forzar rollback retornando Err.
            Err(fitz::db::DbError::Protocol("intencional rollback".into()))
        })
        .await;
    assert!(r.is_err(), "tx debería propagar el Err del callback");

    // Validar que el INSERT NO persistió (rollback automático).
    let qr = conn
        .query("SELECT count(*) AS n FROM fitz_tx_rb", &[])
        .await
        .unwrap();
    let count = qr.rows[0].get("n").unwrap();
    assert_eq!(
        count,
        &PgValue::Int(0),
        "esperaba 0 rows post-ROLLBACK, fue: {:?}",
        count
    );

    conn.exec("DROP TABLE fitz_tx_rb", &[]).await.unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn tx_conn_vuelve_al_pool_despues_de_tx() {
    // Después de una tx (sea OK o Err), la conn vuelve al pool
    // y queries siguientes la pueden reusar. Test contra leak.
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();
    conn.exec(
        "DROP TABLE IF EXISTS fitz_tx_pool; \
         CREATE TABLE fitz_tx_pool (id bigserial PRIMARY KEY)",
        &[],
    )
    .await
    .unwrap();

    // Run 5 transactions consecutivas — si la conn no volviera al
    // pool, agotaríamos el pool (max=10 default) eventualmente.
    // Con 5 iter sobre max=10, no llegamos al límite, pero el bug
    // se manifestaría como "acquire colgado" o "max conns reached".
    for i in 0..5 {
        conn.transaction(|tx| {
            let name = format!("iter_{i}");
            async move {
                tx.exec("INSERT INTO fitz_tx_pool DEFAULT VALUES", &[])
                    .await?;
                let _ = name; // capture, sin uso real
                Ok::<_, fitz::db::DbError>(())
            }
        })
        .await
        .expect("tx debería completar sin colgarse");
    }

    let qr = conn
        .query("SELECT count(*) AS n FROM fitz_tx_pool", &[])
        .await
        .unwrap();
    assert_eq!(
        qr.rows[0].get("n").unwrap(),
        &PgValue::Int(5),
        "esperaba 5 rows (1 por iter)"
    );

    conn.exec("DROP TABLE fitz_tx_pool", &[]).await.unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn pool_queries_concurrentes_no_se_serializan() {
    // Test del pool: lanzamos N queries en paralelo y validamos
    // que el tiempo total << N * tiempo_individual (serializado).
    // Cada query hace pg_sleep(0.1) para forzar I/O wait.
    use std::sync::Arc;
    let url = pg_url();
    // 10.9.2 — connect_url ya devuelve Arc<DbConnHandle>.
    let conn = connect_url(&url).await.unwrap();

    let n_queries = 5;
    let started = std::time::Instant::now();
    let mut handles = Vec::with_capacity(n_queries);
    for i in 0..n_queries {
        let conn_clone = Arc::clone(&conn);
        handles.push(tokio::spawn(async move {
            let qr = conn_clone
                .query(
                    "SELECT pg_sleep(0.2), $1::int AS n",
                    &[PgValue::Int(i as i64)],
                )
                .await
                .expect("query con sleep debería funcionar");
            qr.rows[0].get("n").cloned()
        }));
    }
    let mut results: Vec<i64> = Vec::with_capacity(n_queries);
    for h in handles {
        let v = h.await.unwrap();
        if let Some(PgValue::Int(n)) = v {
            results.push(n);
        }
    }
    let elapsed = started.elapsed();
    results.sort();
    assert_eq!(results, (0..n_queries as i64).collect::<Vec<_>>());

    // Si el pool funciona, 5 queries de 200ms paralelas tardan
    // ~200ms total (no 1000ms). Damos margen amplio (500ms) para
    // ruido de I/O en máquinas lentas.
    assert!(
        elapsed.as_millis() < 800,
        "queries paralelas se serializaron: {} ms para {} queries de 200ms",
        elapsed.as_millis(),
        n_queries
    );

    // El pool tiene >=2 conns abiertas tras los 5 queries
    // paralelos (idle queue las contiene).
    assert!(
        conn.idle_count() >= 2,
        "esperaba >=2 conns idle tras pool stress"
    );

    conn.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn close_idempotente_y_queries_post_close_fallan() {
    let url = pg_url();
    let conn = connect_url(&url).await.unwrap();
    conn.close().await.unwrap();
    // Doble close — no panic.
    conn.close().await.unwrap();
    // is_closed después de close.
    assert!(conn.is_closed().await);
    // Query después de close → error claro.
    let r = conn.query("SELECT 1", &[]).await;
    assert!(r.is_err(), "esperaba Err tras close, fue {:?}", r);
}

// =============================================================
// Fase 10.3.b1 — ORM `User.all(db)` end-to-end con Postgres real
// =============================================================

/// Pre-setup: crea una tabla `fitz_orm_test_users` con 2 rows.
/// Devuelve la conn (para que el test la cierre o reutilice).
async fn seed_orm_test_table(url: &str) -> std::sync::Arc<fitz::db::DbConnHandle> {
    let conn = connect_url(url).await.unwrap();
    // Drop si existe (de runs previos), luego recrea + insert.
    let _ = conn
        .exec("DROP TABLE IF EXISTS fitz_orm_test_users", &[])
        .await;
    conn.exec(
        "CREATE TABLE fitz_orm_test_users (id bigint, name text)",
        &[],
    )
    .await
    .expect("CREATE TABLE OK");
    conn.exec(
        "INSERT INTO fitz_orm_test_users VALUES (1, 'ada'), (2, 'alan')",
        &[],
    )
    .await
    .expect("INSERT seed OK");
    conn
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_user_all_db_devuelve_instancias_reales() {
    // Setup: tabla con 2 rows.
    let url = pg_url();
    let seed_conn = seed_orm_test_table(&url).await;

    // Programa Fitz que define el type ORM, conecta, y llama
    // User.all(db).await?. Resultado: List<User> con 2 instancias.
    let src = format!(
        "@table(\"fitz_orm_test_users\") type User {{\n  \
             id: Int\n  \
             name: Str\n\
         }}\n\
         async fn run() -> Result<List<User>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let users = User.all(db).await?\n  \
             return Ok(users)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    // El binding `result` es `Result<List<User>>`. Lo unwrappeamos.
    let result_val = env.lock().get("result").expect("result bindeado");
    let users_list = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let users_shared = match users_list {
        Value::List(s) => s,
        other => panic!("esperaba List, fue {:?}", other),
    };
    // Snapshot bajo lock corto + drop antes del await siguiente
    // (clippy::await_holding_lock). Recolectamos las assertions
    // primero, después limpiamos.
    let names: Vec<String> = {
        let users = users_shared.lock();
        assert_eq!(users.len(), 2, "esperaba 2 users");
        for (i, u) in users.iter().enumerate() {
            match u {
                Value::Instance { type_name, fields } => {
                    assert_eq!(type_name, "User");
                    let fields_guard = fields.lock();
                    assert_eq!(fields_guard.len(), 2);
                    let id = fields_guard.iter().find(|(n, _)| n == "id").unwrap();
                    let name = fields_guard.iter().find(|(n, _)| n == "name").unwrap();
                    match (&id.1, &name.1) {
                        (Value::Int(_), Value::Str(_)) => {} // shape OK
                        other => panic!("fields shape inesperado en row {}: {:?}", i, other),
                    }
                }
                other => panic!("row {} no es Instance: {:?}", i, other),
            }
        }
        users
            .iter()
            .filter_map(|u| match u {
                Value::Instance { fields, .. } => fields
                    .lock()
                    .iter()
                    .find(|(n, _)| n == "name")
                    .and_then(|(_, v)| match v {
                        Value::Str(s) => Some(s.clone()),
                        _ => None,
                    }),
                _ => None,
            })
            .collect()
        // `users` lock se dropea acá al cerrar el scope.
    };
    assert!(names.contains(&"ada".to_string()));
    assert!(names.contains(&"alan".to_string()));

    // Cleanup: drop tabla. Los locks ya se soltaron arriba.
    let _ = seed_conn.exec("DROP TABLE fitz_orm_test_users", &[]).await;
    seed_conn.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_where_filtra_por_age() {
    // Setup: tabla con 3 rows de edades distintas.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_where_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_where_test (id bigint, name text, age int)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_where_test VALUES (1, 'kid', 10), (2, 'ada', 30), (3, 'alan', 42)",
        &[],
    )
    .await
    .unwrap();

    // Programa Fitz que filtra adultos (age > 18).
    let src = format!(
        "@table(\"fitz_orm_where_test\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<List<User>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let adults = User.where(fn(u) => u.age > 18).all(db).await?\n  \
             return Ok(adults)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let users_list = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let users_shared = match users_list {
        Value::List(s) => s,
        other => panic!("esperaba List, fue {:?}", other),
    };
    let names: Vec<String> = {
        let users = users_shared.lock();
        // Solo 2 rows con age > 18 (ada=30, alan=42); kid=10 queda fuera.
        assert_eq!(users.len(), 2, "esperaba 2 users adultos");
        users
            .iter()
            .filter_map(|u| match u {
                Value::Instance { fields, .. } => fields
                    .lock()
                    .iter()
                    .find(|(n, _)| n == "name")
                    .and_then(|(_, v)| match v {
                        Value::Str(s) => Some(s.clone()),
                        _ => None,
                    }),
                _ => None,
            })
            .collect()
    };
    assert!(names.contains(&"ada".to_string()));
    assert!(names.contains(&"alan".to_string()));
    assert!(
        !names.contains(&"kid".to_string()),
        "kid no debería estar (age=10)"
    );

    let _ = seed.exec("DROP TABLE fitz_orm_where_test", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_where_chain_combina_con_and() {
    // `.where(f).where(g)` combina con AND. Filtramos por
    // age > 18 AND name == 'ada'.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_chain_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_chain_test (id bigint, name text, age int)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_chain_test VALUES (1, 'kid', 10), (2, 'ada', 30), (3, 'alan', 42)",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_chain_test\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<List<User>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let q = User.where(fn(u) => u.age > 18).where(fn(u) => u.name == \"ada\")\n  \
             let result = q.all(db).await?\n  \
             return Ok(result)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let users_list = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        other => panic!("esperaba Ok, fue {:?}", other),
    };
    let users_shared = match users_list {
        Value::List(s) => s,
        other => panic!("esperaba List, fue {:?}", other),
    };
    let name = {
        let users = users_shared.lock();
        assert_eq!(
            users.len(),
            1,
            "esperaba 1 user matching age>18 AND name=ada"
        );
        let n = if let Value::Instance { fields, .. } = &users[0] {
            fields
                .lock()
                .iter()
                .find(|(n, _)| n == "name")
                .and_then(|(_, v)| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap()
        } else {
            panic!("esperaba Instance");
        };
        n
    };
    assert_eq!(name, "ada");

    let _ = seed.exec("DROP TABLE fitz_orm_chain_test", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_order_limit_offset_first_count_e2e() {
    // E2E completo del subset 10.3.b3: ORDER BY DESC + LIMIT +
    // OFFSET + first + count, todo combinado.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_full_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_full_test (id bigint, name text, age int)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_full_test VALUES (1, 'kid', 10), (2, 'ada', 30), (3, 'alan', 42), (4, 'grace', 55), (5, 'admin', 99)",
        &[],
    )
    .await
    .unwrap();

    // Programa Fitz: top 2 adultos ordenados por edad DESC,
    // saltando el primero (offset 1). Esperamos: alan (42) y
    // ada (30) — porque grace (55) y admin (99) son los más
    // viejos, saltamos el primero (admin 99 con DESC), nos
    // quedamos con grace (55) y alan (42).
    //
    // Validamos también `first(db)` (devuelve un solo Instance)
    // y `count(db)` (devuelve Int).
    let src = format!(
        "@table(\"fitz_orm_full_test\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // Sub-query 1: top 2 adultos ordenados por edad DESC, offset 1.\n  \
             let pagina = User\n    \
                 .where(fn(u) => u.age > 18)\n    \
                 .order_by(fn(u) => -u.age)\n    \
                 .limit(2)\n    \
                 .offset(1)\n    \
                 .all(db).await?\n  \
             // Sub-query 2: el más joven adulto.\n  \
             let primero = User\n    \
                 .where(fn(u) => u.age > 18)\n    \
                 .order_by(fn(u) => u.age)\n    \
                 .first(db).await?\n  \
             // Sub-query 3: count de adultos.\n  \
             let total = User.where(fn(u) => u.age > 18).count(db).await?\n  \
             return Ok({{ \"pagina\": pagina, \"primero\": primero, \"total\": total }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    // Extraer y validar.
    let (pagina_names, primero_name, total) = {
        let m = match outer_map {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        };
        let map = m.lock();
        let pagina = map
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "pagina"))
            .map(|(_, v)| v.clone())
            .unwrap();
        let primero = map
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "primero"))
            .map(|(_, v)| v.clone())
            .unwrap();
        let total_v = map
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "total"))
            .map(|(_, v)| v.clone())
            .unwrap();

        // pagina: List<User>
        let pagina_names: Vec<String> = match pagina {
            Value::List(l) => {
                let users = l.lock();
                users
                    .iter()
                    .filter_map(|u| match u {
                        Value::Instance { fields, .. } => fields
                            .lock()
                            .iter()
                            .find(|(n, _)| n == "name")
                            .and_then(|(_, v)| match v {
                                Value::Str(s) => Some(s.clone()),
                                _ => None,
                            }),
                        _ => None,
                    })
                    .collect()
            }
            other => panic!("esperaba List, fue {:?}", other),
        };
        // primero: Instance
        let primero_name = match primero {
            Value::Instance { fields, .. } => fields
                .lock()
                .iter()
                .find(|(n, _)| n == "name")
                .and_then(|(_, v)| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap(),
            other => panic!("esperaba Instance, fue {:?}", other),
        };
        // total: Int
        let total = match total_v {
            Value::Int(n) => n,
            other => panic!("esperaba Int, fue {:?}", other),
        };
        (pagina_names, primero_name, total)
    };

    // pagina: order=DESC, limit=2, offset=1. Adultos DESC: admin(99),
    // grace(55), alan(42), ada(30). Skipeamos admin, nos quedamos
    // con grace y alan.
    assert_eq!(pagina_names, vec!["grace".to_string(), "alan".to_string()]);
    // primero (order ASC): ada (30) — el más joven adulto.
    assert_eq!(primero_name, "ada");
    // total: 4 adultos (ada, alan, grace, admin; kid queda fuera).
    assert_eq!(total, 4);

    let _ = seed.exec("DROP TABLE fitz_orm_full_test", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_insert_update_delete_e2e() {
    // Cycle completo: INSERT + UPDATE + DELETE end-to-end.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_crud_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_crud_test (id bigint, name text, age int)",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_crud_test\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // INSERT — crea 2 rows.\n  \
             let ada = User.insert(db, User {{ id: 1, name: \"ada\", age: 30 }}).await?\n  \
             let alan = User.insert(db, User {{ id: 2, name: \"alan\", age: 42 }}).await?\n  \
             // UPDATE — cambia el age de ada a 31.\n  \
             let updated = User\n    \
                 .where(fn(u) => u.id == 1)\n    \
                 .update(db, {{ \"age\": 31 }}).await?\n  \
             // Verificar via re-fetch.\n  \
             let ada_post = User\n    \
                 .where(fn(u) => u.id == 1)\n    \
                 .first(db).await?\n  \
             // DELETE — borra alan.\n  \
             let deleted = User\n    \
                 .where(fn(u) => u.id == 2)\n    \
                 .delete(db).await?\n  \
             // Count post-delete.\n  \
             let total = User.where(fn(u) => u.id > 0).count(db).await?\n  \
             return Ok({{ \"ada_inserted\": ada, \"updated_rows\": updated, \"ada_post\": ada_post, \"deleted_rows\": deleted, \"total\": total }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };

    let (updated, ada_age_post, deleted, total) = {
        let m = match outer_map {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        };
        let map = m.lock();
        let get_int = |key: &str| -> i64 {
            map.iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .and_then(|(_, v)| match v {
                    Value::Int(n) => Some(*n),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("key {key} no es Int en outer_map"))
        };
        let ada_age = map
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "ada_post"))
            .and_then(|(_, v)| match v {
                Value::Instance { fields, .. } => fields
                    .lock()
                    .iter()
                    .find(|(n, _)| n == "age")
                    .and_then(|(_, v)| match v {
                        Value::Int(n) => Some(*n),
                        _ => None,
                    }),
                _ => None,
            })
            .expect("ada_post.age");
        (
            get_int("updated_rows"),
            ada_age,
            get_int("deleted_rows"),
            get_int("total"),
        )
    };

    assert_eq!(updated, 1, "1 row debería haber updated");
    assert_eq!(ada_age_post, 31, "age de ada debería ser 31 tras update");
    assert_eq!(deleted, 1, "1 row debería haber deleted (alan)");
    assert_eq!(total, 1, "después del delete queda 1 (ada)");

    let _ = seed.exec("DROP TABLE fitz_orm_crud_test", &[]).await;
    seed.close().await.unwrap();
}

// =============================================================
// Fase 10.4.b — Navigation methods (belongs_to / has_many)
// =============================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_belongs_to_y_has_many_e2e() {
    // Setup: 2 tablas relacionadas. users + posts con FK author_id.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_posts_test", &[])
        .await;
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_users_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_users_test (id bigint PRIMARY KEY, name text)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "CREATE TABLE fitz_orm_posts_test (id bigint PRIMARY KEY, title text, author_id bigint REFERENCES fitz_orm_users_test(id))",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_users_test VALUES (1, 'ada'), (2, 'alan')",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_posts_test VALUES (10, 'about algorithms', 1), (11, 'on math', 1), (12, 'turing machines', 2)",
        &[],
    )
    .await
    .unwrap();

    // Programa Fitz que define los types con relations y navega.
    let src = format!(
        "@table(\"fitz_orm_posts_test\") type Post {{\n  \
             @primary\n  \
             id: Int = 0\n  \
             title: Str\n  \
             @belongs_to(\"User\")\n  \
             author_id: Int\n\
         }}\n\
         @table(\"fitz_orm_users_test\") type User {{\n  \
             @primary\n  \
             id: Int = 0\n  \
             name: Str\n  \
             @has_many(\"Post\", via=\"author_id\")\n  \
             posts: List<Post>\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // 1. BelongsTo: post.author(db) → User.\n  \
             let p10 = Post.where(fn(p) => p.id == 10).first(db).await?\n  \
             let author = p10.author_id(db).await?\n  \
             // 2. HasMany: user.posts(db) → List<Post>.\n  \
             let u1 = User.where(fn(u) => u.id == 1).first(db).await?\n  \
             let ada_posts = u1.posts(db).await?\n  \
             return Ok({{ \"author_name\": author.name, \"ada_post_count\": len(ada_posts) }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let (author_name, ada_post_count) = {
        let m = match outer_map {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        };
        let map = m.lock();
        let an = map
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "author_name"))
            .and_then(|(_, v)| match v {
                Value::Str(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap();
        let pc = map
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "ada_post_count"))
            .and_then(|(_, v)| match v {
                Value::Int(n) => Some(*n),
                _ => None,
            })
            .unwrap();
        (an, pc)
    };

    // post id=10 tiene author_id=1 (ada).
    assert_eq!(author_name, "ada");
    // ada (id=1) tiene 2 posts (id=10, 11).
    assert_eq!(ada_post_count, 2);

    // Cleanup respetando el orden (posts depende de users por FK).
    let _ = seed.exec("DROP TABLE fitz_orm_posts_test", &[]).await;
    let _ = seed.exec("DROP TABLE fitz_orm_users_test", &[]).await;
    seed.close().await.unwrap();
}

// =============================================================
// Fase 10.b.7 — Paridad fitz build ↔ fitz run sobre navigation
//
// El test de arriba (orm_belongs_to_y_has_many_e2e) valida el
// path del intérprete (`fitz run`). Este test paralelo compila el
// MISMO programa con `fitz build` y ejecuta el binario standalone,
// confirmando que las navigation queries SQL emitidas son
// equivalentes.
// =============================================================

#[test]
#[ignore]
fn orm_belongs_to_y_has_many_paridad_codegen_e2e() {
    use std::process::Command;
    let url = pg_url();

    // Setup idéntico al test del evaluator.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_posts_codegen_test", &[])
            .await;
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_users_codegen_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_users_codegen_test (id bigint PRIMARY KEY, name text)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "CREATE TABLE fitz_orm_posts_codegen_test (id bigint PRIMARY KEY, title text, author_id bigint REFERENCES fitz_orm_users_codegen_test(id))",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_users_codegen_test VALUES (1, 'ada'), (2, 'alan')",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_posts_codegen_test VALUES (10, 'about algorithms', 1), (11, 'on math', 1), (12, 'turing machines', 2)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    // Programa Fitz paralelo al test del evaluator pero con
    // `print(...)` final para que el binario emita el resultado a
    // stdout — así verificamos contra el output sin reconstruir un
    // env Fitz del lado Rust.
    let src = format!(
        "@table(\"fitz_orm_posts_codegen_test\") type Post {{\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             @belongs_to(\"User\") author_id: Int\n\
         }}\n\
         @table(\"fitz_orm_users_codegen_test\") type User {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             @has_many(\"Post\", via=\"author_id\") posts: List<Post>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let p10 = Post.where(fn(p) => p.id == 10).first(db).await?\n  \
             let author = p10.author_id(db).await?\n  \
             let u1 = User.where(fn(u) => u.id == 1).first(db).await?\n  \
             let ada_posts = u1.posts(db).await?\n  \
             return Ok(\"author={{author.name}} ada_count={{len(ada_posts)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    // Build + run inline (sin reusar build_and_run de compile_e2e.rs
    // — ese vive en otra crate de tests; el helper acá es minimal).
    let stem = "orm_nav_paridad_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, &src).expect("escribir .fitz");

    let fitz_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "fitz.exe" } else { "fitz" });
    let build = Command::new(&fitz_bin)
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        build.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);

    // El test del evaluator valida author_name=ada y ada_post_count=2.
    // Acá validamos el mismo invariante via stdout serializado.
    assert!(
        stdout.contains("author=ada") && stdout.contains("ada_count=2"),
        "esperaba `author=ada ada_count=2`, fue: {}",
        stdout
    );

    // Cleanup.
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE fitz_orm_posts_codegen_test", &[])
            .await;
        let _ = seed
            .exec("DROP TABLE fitz_orm_users_codegen_test", &[])
            .await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.10 — Paridad navigation con @column(name=) en FK source
//
// Cubre la deuda residual del 10.b.7: cross-type navigation cuando
// el FK source field tiene un @column(name=) override. El SELECT
// del row debe usar el sql_name (no el Fitz name), y la navigation
// debe seguir bindeando el value del field Fitz correctamente.
// =============================================================

#[test]
#[ignore]
fn orm_navigation_con_column_override_en_fk_source_paridad_codegen_e2e() {
    use std::process::Command;
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w10_posts_test", &[])
            .await;
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w10_users_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w10_users_test (id bigint PRIMARY KEY, name text)",
            &[],
        )
        .await
        .unwrap();
        // Schema con la columna FK en el SQL llamada `author_uid` —
        // distinta del field Fitz `user_id`.
        seed.exec(
            "CREATE TABLE fitz_orm_w10_posts_test (\
                 id bigint PRIMARY KEY, \
                 title text, \
                 author_uid bigint REFERENCES fitz_orm_w10_users_test(id))",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w10_users_test VALUES (1, 'ada'), (2, 'alan')",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w10_posts_test VALUES \
                 (10, 'on algorithms', 1), \
                 (11, 'on math', 2)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w10_posts_test\") type Post {{\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             @column(name=\"author_uid\") @belongs_to(\"User\") user_id: Int\n\
         }}\n\
         @table(\"fitz_orm_w10_users_test\") type User {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let p10 = Post.where(fn(p) => p.id == 10).first(db).await?\n  \
             let author = p10.user_id(db).await?\n  \
             return Ok(\"author={{author.name}} title={{p10.title}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    let stem = "orm_nav_col_override_paridad_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, &src).expect("escribir .fitz");

    let fitz_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "fitz.exe" } else { "fitz" });
    let build = Command::new(&fitz_bin)
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        build.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);

    assert!(
        stdout.contains("author=ada") && stdout.contains("title=on algorithms"),
        "esperaba `author=ada title=on algorithms`, fue: {}",
        stdout
    );

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w10_posts_test", &[]).await;
        let _ = seed.exec("DROP TABLE fitz_orm_w10_users_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.15 — Paridad eager loading (.preload) sobre HasMany
//
// `User.where(...).preload("posts").all(db).await?` ejecuta el query
// principal + 1 query batch al target con `WHERE fk IN (...)` y
// poblua cada parent con su slice de children. Reduce N+1.
// =============================================================

#[test]
#[ignore]
fn orm_preload_has_many_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w15_posts_test", &[])
            .await;
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w15_users_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w15_users_test (id bigint PRIMARY KEY, name text)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "CREATE TABLE fitz_orm_w15_posts_test (\
                 id bigint PRIMARY KEY, \
                 title text, \
                 user_id bigint REFERENCES fitz_orm_w15_users_test(id))",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w15_users_test VALUES (1, 'ada'), (2, 'alan'), (3, 'grace')",
            &[],
        )
        .await
        .unwrap();
        // ada: 3 posts, alan: 1 post, grace: 0 posts.
        seed.exec(
            "INSERT INTO fitz_orm_w15_posts_test VALUES \
                 (10, 'a',  1), \
                 (11, 'b',  1), \
                 (12, 'c',  1), \
                 (13, 'd',  2)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w15_posts_test\") type Post {{\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             user_id: Int\n\
         }}\n\
         @table(\"fitz_orm_w15_users_test\") type User {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             @has_many(\"Post\") posts: List<Post>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // Sin preload, u.posts es vacío (sentinel del codegen).\n  \
             // Con preload, u.posts viene poblado del batch.\n  \
             let users = User.preload(\"posts\").all(db).await?\n  \
             let u0 = users[0]\n  \
             let u1 = users[1]\n  \
             let u2 = users[2]\n  \
             return Ok(\"u0={{u0.name}}:{{len(u0.posts)}} u1={{u1.name}}:{{len(u1.posts)}} u2={{u2.name}}:{{len(u2.posts)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_preload_has_many_codegen", |stdout| {
        // ada (id 1): 3 posts, alan (id 2): 1, grace (id 3): 0.
        assert!(
            stdout.contains("u0=ada:3")
                && stdout.contains("u1=alan:1")
                && stdout.contains("u2=grace:0"),
            "esperaba `u0=ada:3 u1=alan:1 u2=grace:0`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w15_posts_test", &[]).await;
        let _ = seed.exec("DROP TABLE fitz_orm_w15_users_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Deuda residual #2 (v0.10.5) — BelongsTo eager via convention.
//
// `@belongs_to("User") user_id: Int` + sibling `user: User?` → el
// checker detecta el companion + `.preload("user")` hace batch
// SELECT inverso (target.id IN parent.fk distincts). Cierra la
// limitación heredada de Fase 10.b.15 que solo soportaba HasMany.
// =============================================================

#[test]
#[ignore]
fn orm_preload_belongs_to_companion_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w2_posts_test", &[])
            .await;
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w2_users_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w2_users_test (id bigint PRIMARY KEY, name text)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "CREATE TABLE fitz_orm_w2_posts_test (\
                 id bigint PRIMARY KEY, \
                 title text, \
                 user_id bigint REFERENCES fitz_orm_w2_users_test(id))",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w2_users_test VALUES (1, 'ada'), (2, 'alan')",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w2_posts_test VALUES (10, 'first', 1), (11, 'second', 2), (12, 'third', 1)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w2_users_test\") type User {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }}\n\
         @table(\"fitz_orm_w2_posts_test\") type Post {{\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             @belongs_to(\"User\") user_id: Int\n  \
             user: User?\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // Sin preload, p.user es None (sentinel del codegen).\n  \
             // Con preload, p.user viene poblado del batch.\n  \
             let posts = Post.preload(\"user\").order_by(fn(p) => p.id).all(db).await?\n  \
             let n = len(posts)\n  \
             // Smoke: el preload corre + el batch SELECT inverso\n  \
             // poblá los companions sin errors. Como Fitz hoy no\n  \
             // refina Nullable en match arms (deuda separada del\n  \
             // sistema de tipos), comparamos por null vs no-null y\n  \
             // contamos los non-null para verificar la hidratación.\n  \
             let p0_has_user = if (posts[0].user == null) {{ 0 }} else {{ 1 }}\n  \
             let p1_has_user = if (posts[1].user == null) {{ 0 }} else {{ 1 }}\n  \
             let p2_has_user = if (posts[2].user == null) {{ 0 }} else {{ 1 }}\n  \
             let total_loaded = p0_has_user + p1_has_user + p2_has_user\n  \
             return Ok(\"posts={{n}} preloaded={{total_loaded}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_preload_belongs_to_companion_codegen", |stdout| {
        // 3 posts cargados, los 3 deben tener user companion poblado
        // (todos apuntan a users que existen — sin orphans).
        assert!(
            stdout.contains("posts=3 preloaded=3"),
            "esperaba `posts=3 preloaded=3`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w2_posts_test", &[]).await;
        let _ = seed.exec("DROP TABLE fitz_orm_w2_users_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Deuda residual #4 (v0.10.5) — chain dinámico condicional de
// .where(), .order_by(), .limit(), .offset().
//
// Originalmente planteado como "no funciona" en la doc (drift),
// pero el codegen YA soportaba este patrón: `qb = qb.where(...)`
// adentro de un `if` compila y corre con paridad bit-a-bit. Este
// test es el regression guard para que no rompa en el futuro.
// =============================================================

#[test]
#[ignore]
fn orm_dynamic_chain_conditional_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w4_users_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w4_users_test (id bigint PRIMARY KEY, name text, age bigint, active boolean)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w4_users_test\") type User {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n  \
             active: Bool\n\
         }}\n\
         async fn search(min_age: Int, active_only: Bool, name_like: Str, db: DbConn) -> Result<List<User>> {{\n  \
             let qb = User.where(fn(u) => u.age >= min_age)\n  \
             if (active_only) {{\n    \
                 qb = qb.where(fn(u) => u.active)\n  \
             }}\n  \
             if (name_like != \"\") {{\n    \
                 qb = qb.where(fn(u) => u.name.like(name_like))\n  \
             }}\n  \
             return qb.order_by(fn(u) => u.id).all(db).await\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             User.insert(db, User {{ id: 1, name: \"ada\",   age: 35, active: true  }}).await?\n  \
             User.insert(db, User {{ id: 2, name: \"alan\",  age: 17, active: true  }}).await?\n  \
             User.insert(db, User {{ id: 3, name: \"abe\",   age: 42, active: false }}).await?\n  \
             User.insert(db, User {{ id: 4, name: \"grace\", age: 28, active: true  }}).await?\n  \
             User.insert(db, User {{ id: 5, name: \"bob\",   age: 50, active: true  }}).await?\n  \
             let r1 = search(18, false, \"\", db).await?\n  \
             let r2 = search(18, true,  \"\", db).await?\n  \
             let r3 = search(18, true,  \"a%\", db).await?\n  \
             let r4 = search(0,  true,  \"b%\", db).await?\n  \
             let n1 = len(r1)\n  \
             let n2 = len(r2)\n  \
             let n3 = len(r3)\n  \
             let n4 = len(r4)\n  \
             return Ok(\"n1={{n1}} n2={{n2}} n3={{n3}} n4={{n4}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_dynamic_chain_conditional_codegen", |stdout| {
        // n1: age>=18 → ada, abe, grace, bob = 4
        // n2: + active → ada, grace, bob = 3
        // n3: + name LIKE "a%" → ada = 1
        // n4: age>=0 + active + LIKE "b%" → bob = 1
        assert!(
            stdout.contains("n1=4")
                && stdout.contains("n2=3")
                && stdout.contains("n3=1")
                && stdout.contains("n4=1"),
            "esperaba `n1=4 n2=3 n3=1 n4=1`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w4_users_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Deuda residual #3 (v0.10.5) — JSON operators en .where(...).
//
// has_key / has_all_keys / has_any_keys / contains_json / get
// sobre fields jsonb (Map<Str, Any>) mapeados a operadores
// nativos Postgres (`?`, `?&`, `?|`, `@>`, `->>'k'`). Sin esto
// el user tenía que bajar a `db.query(...)` crudo.
// =============================================================

#[test]
#[ignore]
fn orm_jsonb_operators_in_where_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w3_events_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w3_events_test (id bigint PRIMARY KEY, name text, data jsonb)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w3_events_test\") type Event {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             data: Map<Str, Any>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // Heterogéneo en values fuerza Vec<(__FitzValue,\n  \
             // __FitzValue)> en codegen → matchea el jsonb shape.\n  \
             Event.insert(db, Event {{ id: 1, name: \"click\",  data: {{\"page\": \"/home\",  \"user\": \"ada\",   \"ts\": 100}} }}).await?\n  \
             Event.insert(db, Event {{ id: 2, name: \"submit\", data: {{\"page\": \"/login\", \"user\": \"alan\",  \"ts\": 200}} }}).await?\n  \
             Event.insert(db, Event {{ id: 3, name: \"view\",   data: {{\"page\": \"/home\",  \"user\": \"grace\", \"extra\": true}} }}).await?\n  \
             Event.insert(db, Event {{ id: 4, name: \"error\",  data: {{\"code\": 500,      \"kind\": \"fatal\",  \"retry\": false}} }}).await?\n  \
             // .has_key(\"page\") → 3 (todos menos error)\n  \
             let with_page = Event.where(fn(e) => e.data.has_key(\"page\")).all(db).await?\n  \
             // .has_all_keys([page, user]) → 3 (todos menos error)\n  \
             let with_both = Event.where(fn(e) => e.data.has_all_keys([\"page\", \"user\"])).all(db).await?\n  \
             // .has_any_keys([code, extra]) → 2 (view + error)\n  \
             let either = Event.where(fn(e) => e.data.has_any_keys([\"code\", \"extra\"])).all(db).await?\n  \
             // .contains_json({{\"page\": \"/home\"}}) → 2 (click + view)\n  \
             let home = Event.where(fn(e) => e.data.contains_json({{\"page\": \"/home\"}})).all(db).await?\n  \
             // .get(\"user\") == \"ada\" → 1 (click)\n  \
             let ada = Event.where(fn(e) => e.data.get(\"user\") == \"ada\").all(db).await?\n  \
             let n1 = len(with_page)\n  \
             let n2 = len(with_both)\n  \
             let n3 = len(either)\n  \
             let n4 = len(home)\n  \
             let n5 = len(ada)\n  \
             return Ok(\"has_key={{n1}} has_all={{n2}} has_any={{n3}} contains={{n4}} get={{n5}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_jsonb_operators_in_where_codegen", |stdout| {
        assert!(
            stdout.contains("has_key=3")
                && stdout.contains("has_all=3")
                && stdout.contains("has_any=2")
                && stdout.contains("contains=2")
                && stdout.contains("get=1"),
            "esperaba `has_key=3 has_all=3 has_any=2 contains=2 get=1`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w3_events_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.14 — Paridad GROUP BY + aggregate (Aggregated<Row>)
//
// `.group_by(...).count/sum/avg/min/max(...).await?` ahora compila a
// binario y devuelve `List<Map<Str, Any>>` con cada row = un grupo
// + el aggregate name. Cierre de la deuda más grande de Fase 10.b.
// =============================================================

#[test]
#[ignore]
fn orm_group_by_aggregate_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w14_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w14_test (\
                 id bigint PRIMARY KEY, \
                 region text, \
                 amount double precision)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w14_test VALUES \
                 (1, 'PAT', 100.0), \
                 (2, 'BUE',  50.0), \
                 (3, 'PAT', 200.0), \
                 (4, 'BUE', 150.0), \
                 (5, 'CBA',  80.0), \
                 (6, 'BUE', 250.0)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w14_test\") type Sale {{\n  \
             @primary id: Int = 0\n  \
             region: Str\n  \
             amount: Float\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let counts = Sale.group_by(fn(s) => s.region).count(db).await?\n  \
             let sums = Sale.group_by(fn(s) => s.region).sum(fn(s) => s.amount, db).await?\n  \
             return Ok(\"counts_groups={{len(counts)}} sums_groups={{len(sums)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_group_by_agg_codegen", |stdout| {
        // 3 grupos distintos (PAT, BUE, CBA).
        assert!(
            stdout.contains("counts_groups=3") && stdout.contains("sums_groups=3"),
            "esperaba `counts_groups=3 sums_groups=3`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w14_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.13 — Paridad navigation chain (sin db = QueryBuilder)
//
// `instance.posts()` ahora devuelve un QueryBuilder<Post> que admite
// chain: `.where(...).order_by(...).limit(N).all(db).await?`.
// Backward compat: `instance.posts(db)` sigue funcionando como
// terminal directo (`.all` para HasMany).
// =============================================================

#[test]
#[ignore]
fn orm_navigation_chain_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w13_posts_test", &[])
            .await;
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w13_users_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w13_users_test (id bigint PRIMARY KEY, name text)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "CREATE TABLE fitz_orm_w13_posts_test (\
                 id bigint PRIMARY KEY, \
                 title text, \
                 views bigint, \
                 user_id bigint REFERENCES fitz_orm_w13_users_test(id))",
            &[],
        )
        .await
        .unwrap();
        seed.exec("INSERT INTO fitz_orm_w13_users_test VALUES (1, 'ada')", &[])
            .await
            .unwrap();
        // 6 posts para user 1, con views variadas, para que el chain
        // discrimine: .where(views > 10).order_by(-views).limit(2).
        seed.exec(
            "INSERT INTO fitz_orm_w13_posts_test VALUES \
                 (10, 'a',  5,  1), \
                 (11, 'b', 50,  1), \
                 (12, 'c', 30,  1), \
                 (13, 'd',  8,  1), \
                 (14, 'e', 90,  1), \
                 (15, 'f', 20,  1)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w13_posts_test\") type Post {{\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             views: Int\n  \
             user_id: Int\n\
         }}\n\
         @table(\"fitz_orm_w13_users_test\") type User {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             @has_many(\"Post\") posts: List<Post>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let u = User.where(fn(u) => u.id == 1).first(db).await?\n  \
             // Path nuevo (chain): top 2 posts del user por views DESC, filtrando views>10.\n  \
             let top = u.posts().where(fn(p) => p.views > 10).order_by(fn(p) => -p.views).limit(2).all(db).await?\n  \
             let t0 = top[0]\n  \
             let t1 = top[1]\n  \
             // Path legacy: traer todos los posts del user (sin filtro).\n  \
             let all_posts = u.posts(db).await?\n  \
             return Ok(\"chain_count={{len(top)}} t0={{t0.id}}:{{t0.views}} t1={{t1.id}}:{{t1.views}} all_count={{len(all_posts)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_nav_chain_codegen", |stdout| {
        // chain top 2 DESC views>10: id=14 (90), id=11 (50).
        // all_posts: 6 rows.
        assert!(
            stdout.contains("chain_count=2")
                && stdout.contains("t0=14:90")
                && stdout.contains("t1=11:50")
                && stdout.contains("all_count=6"),
            "esperaba chain_count=2 t0=14:90 t1=11:50 all_count=6, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w13_posts_test", &[]).await;
        let _ = seed.exec("DROP TABLE fitz_orm_w13_users_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.12.b — Paridad `Map<Str, T>` concreto (JSONB tipado)
//
// El codegen ahora soporta `Map<Str, Int|Float|Str|Bool>` mapeado a
// columnas `jsonb` Postgres con validación de shape en deserialize
// (rechaza JSON object donde algún value no matchee T) y serialize
// directo a `serde_json::Value::Number/String/Bool` sin __FitzValue.
// =============================================================

#[test]
#[ignore]
fn orm_map_str_concreto_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w12b_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w12b_test (\
                 id bigint PRIMARY KEY, \
                 counts jsonb, \
                 attrs jsonb)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w12b_test VALUES \
                 (1, '{\"a\": 10, \"b\": 20, \"c\": 30}', \
                     '{\"name\": \"ada\", \"role\": \"admin\"}')",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w12b_test\") type Doc {{\n  \
             @primary id: Int = 0\n  \
             counts: Map<Str, Int>\n  \
             attrs: Map<Str, Str>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let d = Doc.where(fn(x) => x.id == 1).first(db).await?\n  \
             // INSERT round-trip con Map<Str, Int> + Map<Str, Str> literales.\n  \
             let _new = Doc.insert(db, Doc {{ \
                 id: 2, \
                 counts: {{\"x\": 100, \"y\": 200}}, \
                 attrs: {{\"k\": \"v\"}} \
             }}).await?\n  \
             let d2 = Doc.where(fn(x) => x.id == 2).first(db).await?\n  \
             return Ok(\"counts1={{len(d.counts)}} attrs1={{len(d.attrs)}} counts2={{len(d2.counts)}} attrs2={{len(d2.attrs)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_map_str_concreto_codegen", |stdout| {
        // counts1: 3 keys, attrs1: 2 keys.
        // counts2: 2 keys, attrs2: 1 key.
        assert!(
            stdout.contains("counts1=3")
                && stdout.contains("attrs1=2")
                && stdout.contains("counts2=2")
                && stdout.contains("attrs2=1"),
            "esperaba `counts1=3 attrs1=2 counts2=2 attrs2=1`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w12b_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.12.a — Paridad `List<scalar?>` (NULL adentro de arrays)
//
// Postgres permite arrays con NULL elements (`{1,NULL,3}`). El
// codegen ahora soporta `List<Int?>`/`List<Str?>`/etc. y materializa
// `Vec<Option<T>>` en el struct.
// =============================================================

#[test]
#[ignore]
fn orm_list_nullable_inner_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w12a_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w12a_test (id bigint PRIMARY KEY, tags int8[])",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w12a_test VALUES (1, '{10, NULL, 30, NULL, 50}')",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w12a_test\") type Item {{\n  \
             @primary id: Int = 0\n  \
             tags: List<Int?>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let it = Item.where(fn(i) => i.id == 1).first(db).await?\n  \
             let _inserted = Item.insert(db, Item {{ id: 2, tags: [100, null, 300] }}).await?\n  \
             let it2 = Item.where(fn(i) => i.id == 2).first(db).await?\n  \
             return Ok(\"len1={{len(it.tags)}} len2={{len(it2.tags)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_list_nullable_inner_codegen", |stdout| {
        assert!(
            stdout.contains("len1=5") && stdout.contains("len2=3"),
            "esperaba `len1=5 len2=3`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w12a_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.11 — Paridad `.update` con List literal + Map literal
//
// Cubre la deuda residual de 10.b.8.a/b: el `.update(db, {...})`
// con fields array/JSONB ahora emite el binding directo
// (PgValue::Array para List<scalar>, JSONB serializado via
// __FitzValue::Map para Map<Str, Any>), no el genérico
// __IntoPgValue::into_pg que solo servía para primitivos.
// =============================================================

#[test]
#[ignore]
fn orm_update_con_list_y_map_literal_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w11_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w11_test (\
                 id bigint PRIMARY KEY, \
                 tags int8[], \
                 meta jsonb)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w11_test VALUES (1, '{1,2,3}', '{\"v\": 0}')",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w11_test\") type Item {{\n  \
             @primary id: Int = 0\n  \
             tags: List<Int>\n  \
             meta: Map<Str, Any>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let n = Item.where(fn(i) => i.id == 1).update(db, \
                 {{\"tags\": [10, 20, 30], \"meta\": {{\"author\": \"ada\", \"version\": 7}}}}\
             ).await?\n  \
             let it = Item.where(fn(i) => i.id == 1).first(db).await?\n  \
             return Ok(\"n={{n}} tags_len={{len(it.tags)}} meta_keys={{len(it.meta)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_upd_list_map_codegen", |stdout| {
        assert!(
            stdout.contains("n=1")
                && stdout.contains("tags_len=3")
                && stdout.contains("meta_keys=2"),
            "esperaba `n=1 tags_len=3 meta_keys=2`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w11_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.10 — Tests paridad real Postgres exhaustivos
//
// Cobertura faltante de sub-pasos previos:
//   - 10.b.3: Type.all / .first / .count basics
//   - 10.b.4: insert con RETURNING preservando el id auto-asignado
//   - 10.b.5: chain order_by + limit + offset + .update + .delete
//   - 10.b.6: aggregates scalar (sum/avg/min/max) con valores conocidos
// =============================================================

#[test]
#[ignore]
fn orm_basics_all_first_count_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w10b_basics_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w10b_basics_test (id bigint PRIMARY KEY, name text)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w10b_basics_test VALUES (1, 'ada'), (2, 'alan'), (3, 'grace')",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w10b_basics_test\") type User {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let all = User.all(db).await?\n  \
             let first = User.first(db).await?\n  \
             let n = User.count(db).await?\n  \
             return Ok(\"all={{len(all)}} first={{first.name}} count={{n}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_basics_all_first_count_codegen", |stdout| {
        assert!(
            stdout.contains("all=3") && stdout.contains("count=3"),
            "esperaba `all=3 count=3`, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w10b_basics_test", &[]).await;
        seed.close().await.unwrap();
    });
}

#[test]
#[ignore]
fn orm_crud_lifecycle_insert_update_delete_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w10b_crud_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w10b_crud_test (\
                 id bigserial PRIMARY KEY, \
                 name text, \
                 age bigint)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w10b_crud_test\") type Person {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // INSERT: el server asigna el id (bigserial).\n  \
             let inserted = Person.insert(db, Person {{ id: 0, name: \"ada\", age: 35 }}).await?\n  \
             // Extraemos el id a una var local — el closure de .where()\n  \
             // solo admite `param.field` o vars externas simples (Ident).\n  \
             let new_id = inserted.id\n  \
             // UPDATE: age = 36.\n  \
             let updated = Person.where(fn(p) => p.id == new_id).update(db, {{\"age\": 36}}).await?\n  \
             // SELECT verify.\n  \
             let post_update = Person.where(fn(p) => p.id == new_id).first(db).await?\n  \
             let post_age = post_update.age\n  \
             // DELETE.\n  \
             let deleted = Person.where(fn(p) => p.id == new_id).delete(db).await?\n  \
             // SELECT verify deleted.\n  \
             let after = Person.count(db).await?\n  \
             return Ok(\"updated={{updated}} age_after_update={{post_age}} deleted={{deleted}} count_after={{after}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_crud_lifecycle_codegen", |stdout| {
        // Cada operación afecta 1 row; el SELECT post-update lee age=36;
        // el count tras el DELETE es 0.
        assert!(
            stdout.contains("updated=1")
                && stdout.contains("age_after_update=36")
                && stdout.contains("deleted=1")
                && stdout.contains("count_after=0"),
            "esperaba lifecycle CRUD completo, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w10b_crud_test", &[]).await;
        seed.close().await.unwrap();
    });
}

#[test]
#[ignore]
fn orm_order_by_limit_offset_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w10b_order_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w10b_order_test (id bigint PRIMARY KEY, score bigint)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w10b_order_test VALUES \
                 (1, 50), (2, 30), (3, 90), (4, 10), (5, 70)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w10b_order_test\") type Score {{\n  \
             @primary id: Int = 0\n  \
             score: Int\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // Top 2 scores DESC: 90 (id=3), 70 (id=5).\n  \
             let top = Score.order_by(fn(s) => -s.score).limit(2).all(db).await?\n  \
             let top0 = top[0]\n  \
             let top1 = top[1]\n  \
             // Skip 1 + take 2 ASC: id=2(30), id=1(50).\n  \
             let mid = Score.order_by(fn(s) => s.score).limit(2).offset(1).all(db).await?\n  \
             let mid0 = mid[0]\n  \
             let mid1 = mid[1]\n  \
             return Ok(\"top0={{top0.id}}:{{top0.score}} top1={{top1.id}}:{{top1.score}} mid0={{mid0.id}}:{{mid0.score}} mid1={{mid1.id}}:{{mid1.score}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_order_by_limit_offset_codegen", |stdout| {
        // DESC: top0=3:90, top1=5:70. ASC offset 1: ranked 10,30,50,70,90;
        // skip 1 → starts at 30 → mid0=2:30, mid1=1:50.
        assert!(
            stdout.contains("top0=3:90")
                && stdout.contains("top1=5:70")
                && stdout.contains("mid0=2:30")
                && stdout.contains("mid1=1:50"),
            "esperaba orden + paginación correcta, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w10b_order_test", &[]).await;
        seed.close().await.unwrap();
    });
}

#[test]
#[ignore]
fn orm_aggregates_scalar_paridad_codegen_e2e() {
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w10b_agg_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w10b_agg_test (\
                 id bigint PRIMARY KEY, \
                 price double precision)",
            &[],
        )
        .await
        .unwrap();
        // Sum = 100, Avg = 25, Min = 10, Max = 40.
        seed.exec(
            "INSERT INTO fitz_orm_w10b_agg_test VALUES \
                 (1, 10.0), (2, 20.0), (3, 30.0), (4, 40.0)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w10b_agg_test\") type Sale {{\n  \
             @primary id: Int = 0\n  \
             price: Float\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let sum_price = Sale.sum(fn(s) => s.price, db).await?\n  \
             let avg_price = Sale.avg(fn(s) => s.price, db).await?\n  \
             let min_price = Sale.min(fn(s) => s.price, db).await?\n  \
             let max_price = Sale.max(fn(s) => s.price, db).await?\n  \
             return Ok(\"sum={{sum_price}} avg={{avg_price}} min={{min_price}} max={{max_price}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    run_paridad_program(&src, "orm_aggregates_scalar_codegen", |stdout| {
        assert!(
            stdout.contains("sum=100")
                && stdout.contains("avg=25")
                && stdout.contains("min=10")
                && stdout.contains("max=40"),
            "esperaba sum=100 avg=25 min=10 max=40, fue: {}",
            stdout
        );
    });

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w10b_agg_test", &[]).await;
        seed.close().await.unwrap();
    });
}

/// Helper común para los tests de 10.b.10: compila el programa Fitz
/// con `fitz build`, ejecuta el binario, y le pasa el stdout al
/// closure de assertion del caller. Reduce ~50 LoC duplicadas por
/// test. Llamado desde los 4 tests E2E paridad de 10.b.10.
fn run_paridad_program(src: &str, stem: &str, assert_stdout: impl FnOnce(&str)) {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let fitz_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "fitz.exe" } else { "fitz" });
    let build = Command::new(&fitz_bin)
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        build.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);
    assert_stdout(&stdout);
}

// =============================================================
// Fase 10.b.9.a — Paridad fitz build sobre operadores `.where`
//
// Programa con varios operadores combinados (==/>=/AND/OR/like/
// is_null/is_in/NOT) sobre datos reales, validando que el binario
// standalone produce el mismo resultset que el evaluator.
// =============================================================

#[test]
#[ignore]
fn orm_where_combinatorio_paridad_codegen_e2e() {
    use std::process::Command;
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_where_codegen_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_where_codegen_test (\
                 id bigint PRIMARY KEY, \
                 name text, \
                 age bigint, \
                 score double precision, \
                 deleted_at text)",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_where_codegen_test VALUES \
                 (1, 'ada',    35, 85.0, NULL), \
                 (2, 'alan',   42, 92.5, NULL), \
                 (3, 'grace',  60, 78.0, NULL), \
                 (4, 'donald', 17, 45.0, '2026-01-01'), \
                 (5, 'edsger', 80, 95.0, NULL), \
                 (6, 'bob',    21, 30.0, '2025-12-15')",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_where_codegen_test\") type Person {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n  \
             score: Float\n  \
             deleted_at: Str?\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let activos = Person.where(fn(p) => \
                 p.age >= 18 and p.deleted_at.is_null() and p.score > 80.0\
             ).all(db).await?\n  \
             let with_a = Person.where(fn(p) => p.name.starts_with(\"a\")).all(db).await?\n  \
             let ids = Person.where(fn(p) => p.id.is_in([1, 3, 5])).all(db).await?\n  \
             let menores = Person.where(fn(p) => not (p.age >= 18)).all(db).await?\n  \
             return Ok(\"activos={{len(activos)}} a={{len(with_a)}} ids={{len(ids)}} menores={{len(menores)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    let stem = "orm_where_combinatorio_paridad_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, &src).expect("escribir .fitz");

    let fitz_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "fitz.exe" } else { "fitz" });
    let build = Command::new(&fitz_bin)
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        build.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);

    // Cardinalidad esperada según los seeds:
    //   - activos (age>=18 AND deleted_at IS NULL AND score>80):
    //     ada(35,85), alan(42,92.5), edsger(80,95) → 3
    //   - with_a (name starts_with 'a'): ada, alan → 2
    //   - ids IN (1,3,5): ada, grace, edsger → 3
    //   - menores (NOT age>=18): donald(17) → 1
    assert!(
        stdout.contains("activos=3")
            && stdout.contains("a=2")
            && stdout.contains("ids=3")
            && stdout.contains("menores=1"),
        "esperaba `activos=3 a=2 ids=3 menores=1`, fue: {}",
        stdout
    );

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE fitz_orm_where_codegen_test", &[])
            .await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.9.b — Paridad fitz build sobre between + Mod + var externa
// =============================================================

#[test]
#[ignore]
fn orm_where_between_mod_var_externa_paridad_codegen_e2e() {
    use std::process::Command;
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w9b_codegen_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w9b_codegen_test (\
                 id bigint PRIMARY KEY, \
                 name text, \
                 age bigint)",
            &[],
        )
        .await
        .unwrap();
        // 10 rows con ages 10..=100 step 10 — ideal para verificar
        // bandas y módulo.
        seed.exec(
            "INSERT INTO fitz_orm_w9b_codegen_test VALUES \
                 (1, 'a', 10), (2, 'b', 20), (3, 'c', 30), (4, 'd', 40), (5, 'e', 50), \
                 (6, 'f', 60), (7, 'g', 70), (8, 'h', 80), (9, 'i', 90), (10, 'j', 100)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w9b_codegen_test\") type Item {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // between (inclusive): age in [30, 60] → 30,40,50,60 → 4 rows.\n  \
             let b = Item.where(fn(i) => i.age.between(30, 60)).all(db).await?\n  \
             // mod: age % 20 == 0 → 20,40,60,80,100 → 5 rows.\n  \
             let m = Item.where(fn(i) => i.age % 20 == 0).all(db).await?\n  \
             // var externa capturada del scope outer.\n  \
             let limit = 50\n  \
             let v = Item.where(fn(i) => i.age >= limit).all(db).await?\n  \
             return Ok(\"between={{len(b)}} mod={{len(m)}} vext={{len(v)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    let stem = "orm_where_b_mod_vext_paridad_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, &src).expect("escribir .fitz");

    let fitz_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "fitz.exe" } else { "fitz" });
    let build = Command::new(&fitz_bin)
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        build.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);

    // Cardinalidad esperada:
    //   between [30,60] → 30,40,50,60 → 4
    //   mod 20 == 0 → 20,40,60,80,100 → 5
    //   age >= 50 (var ext) → 50..100 step 10 → 6 rows
    assert!(
        stdout.contains("between=4") && stdout.contains("mod=5") && stdout.contains("vext=6"),
        "esperaba `between=4 mod=5 vext=6`, fue: {}",
        stdout
    );

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w9b_codegen_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.9.c — Paridad fitz build sobre operadores de arrays
// (has / contains_all / contained_in)
// =============================================================

#[test]
#[ignore]
fn orm_where_array_ops_paridad_codegen_e2e() {
    use std::process::Command;
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_w9c_codegen_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_w9c_codegen_test (\
                 id bigint PRIMARY KEY, \
                 name text, \
                 tags int8[])",
            &[],
        )
        .await
        .unwrap();
        seed.exec(
            "INSERT INTO fitz_orm_w9c_codegen_test VALUES \
                 (1, 'alpha',   '{10,20,30}'), \
                 (2, 'beta',    '{20,40,60}'), \
                 (3, 'gamma',   '{10,30,50}'), \
                 (4, 'delta',   '{40,50,60}')",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    let src = format!(
        "@table(\"fitz_orm_w9c_codegen_test\") type Item {{\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             tags: List<Int>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let h = Item.where(fn(i) => i.tags.has(20)).all(db).await?\n  \
             let ca = Item.where(fn(i) => i.tags.contains_all([10, 30])).all(db).await?\n  \
             let ci = Item.where(fn(i) => i.tags.contained_in([40, 50, 60])).all(db).await?\n  \
             return Ok(\"has={{len(h)}} contains_all={{len(ca)}} contained_in={{len(ci)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    let stem = "orm_where_array_ops_paridad_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, &src).expect("escribir .fitz");

    let fitz_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "fitz.exe" } else { "fitz" });
    let build = Command::new(&fitz_bin)
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        build.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);

    // Cardinalidad esperada:
    //   has(20)        : alpha={10,20,30}, beta={20,40,60} → 2
    //   contains_all   : alpha (10+30), gamma (10+30) → 2
    //   contained_in   : delta {40,50,60} ⊆ {40,50,60} → 1
    assert!(
        stdout.contains("has=2")
            && stdout.contains("contains_all=2")
            && stdout.contains("contained_in=1"),
        "esperaba `has=2 contains_all=2 contained_in=1`, fue: {}",
        stdout
    );

    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed.exec("DROP TABLE fitz_orm_w9c_codegen_test", &[]).await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.b.8 — Paridad fitz build sobre arrays + JSONB
//
// Programa con `tags: List<Int>` (array Postgres) y `meta: Map<Str,
// Any>` (JSONB libre). Inserta una row, lee de vuelta, valida que el
// round-trip preserva los valores. Solo valida `fitz build` (el
// evaluator ya tiene cobertura propia desde 10.5.a/b).
// =============================================================

#[test]
#[ignore]
fn orm_arrays_y_jsonb_paridad_codegen_e2e() {
    use std::process::Command;
    let url = pg_url();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE IF EXISTS fitz_orm_docs_codegen_test", &[])
            .await;
        seed.exec(
            "CREATE TABLE fitz_orm_docs_codegen_test (\
                 id bigint PRIMARY KEY, \
                 title text, \
                 tags int8[], \
                 meta jsonb)",
            &[],
        )
        .await
        .unwrap();
        seed.close().await.unwrap();
    });

    // Programa Fitz: insert + select + print resumen. Usamos
    // `print` para emitir el resultado a stdout y validamos contra
    // el output del binario standalone.
    let src = format!(
        "@table(\"fitz_orm_docs_codegen_test\") type Doc {{\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             tags: List<Int>\n  \
             meta: Map<Str, Any>\n\
         }}\n\
         async fn run() -> Result<Str> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let d = Doc.insert(db, Doc {{ \
                 id: 1, \
                 title: \"hello\", \
                 tags: [10, 20, 30], \
                 meta: {{\"author\": \"ada\", \"version\": 7, \"draft\": false}} \
             }}).await?\n  \
             let xs = Doc.where(fn(x) => x.id == 1).all(db).await?\n  \
             let first = xs[0]\n  \
             return Ok(\"id={{first.id}} title={{first.title}} tags_len={{len(first.tags)}} meta_keys={{len(first.meta)}}\")\n\
         }}\n\
         async fn driver() -> Str {{\n  \
             return match run().await {{\n    \
                 Ok(s) => s\n    \
                 Err(e) => \"err: {{e}}\"\n  \
             }}\n\
         }}\n\
         print(driver().await)\n",
        url
    );

    let stem = "orm_arrays_jsonb_paridad_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, &src).expect("escribir .fitz");

    let fitz_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "fitz.exe" } else { "fitz" });
    let build = Command::new(&fitz_bin)
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        build.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);

    // tags se insertó como [10, 20, 30] → len = 3.
    // meta se insertó con 3 keys → len = 3.
    assert!(
        stdout.contains("id=1")
            && stdout.contains("title=hello")
            && stdout.contains("tags_len=3")
            && stdout.contains("meta_keys=3"),
        "esperaba `id=1 title=hello tags_len=3 meta_keys=3`, fue: {}",
        stdout
    );

    // Cleanup.
    rt.block_on(async {
        let seed = connect_url(&url).await.unwrap();
        let _ = seed
            .exec("DROP TABLE fitz_orm_docs_codegen_test", &[])
            .await;
        seed.close().await.unwrap();
    });
}

// =============================================================
// Fase 10.5.f1 — Agregados (sum / avg / min / max)
// =============================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_aggregates_sum_avg_min_max_e2e() {
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_agg_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_agg_test (id bigint, name text, age int)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_agg_test VALUES (1, 'kid', 10), (2, 'ada', 30), (3, 'alan', 42), (4, 'grace', 55)",
        &[],
    )
    .await
    .unwrap();

    // Programa Fitz: sum/min/max/avg sobre `age` con filtro adultos.
    // Esperado: sum = 30+42+55 = 127; min = 30; max = 55;
    // avg = 127/3 ≈ 42.33.
    let src = format!(
        "@table(\"fitz_orm_agg_test\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let adultos = User.where(fn(u) => u.age > 18)\n  \
             let total_age = adultos.sum(fn(u) => u.age, db).await?\n  \
             let min_age = adultos.min(fn(u) => u.age, db).await?\n  \
             let max_age = adultos.max(fn(u) => u.age, db).await?\n  \
             let avg_age = adultos.avg(fn(u) => u.age, db).await?\n  \
             return Ok({{ \"sum\": total_age, \"min\": min_age, \"max\": max_age, \"avg\": avg_age }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let (sum, min, max, avg) = {
        let m = match outer_map {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        };
        let map = m.lock();
        let get = |key: &str| {
            map.iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        (get("sum"), get("min"), get("max"), get("avg"))
    };

    // Sum/min/max sobre Int devuelven Int.
    assert_eq!(sum, Value::Int(127));
    assert_eq!(min, Value::Int(30));
    assert_eq!(max, Value::Int(55));
    // Avg emite CAST a float8 → Float.
    match avg {
        Value::Float(x) => {
            assert!(
                (x - 42.333_333).abs() < 0.01,
                "esperaba avg ≈ 42.33, fue {x}"
            );
        }
        other => panic!("esperaba Float, fue {:?}", other),
    }

    let _ = seed.exec("DROP TABLE fitz_orm_agg_test", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_aggregate_sobre_set_vacio_devuelve_null() {
    // SUM/AVG/MIN/MAX sobre 0 rows → Postgres devuelve NULL.
    // El ORM lo expone como Value::Null adentro de Result::Ok.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_empty_agg", &[])
        .await;
    seed.exec("CREATE TABLE fitz_orm_empty_agg (id bigint, age int)", &[])
        .await
        .unwrap();
    // Insertamos 1 row pero la filtramos con WHERE imposible.
    seed.exec("INSERT INTO fitz_orm_empty_agg VALUES (1, 10)", &[])
        .await
        .unwrap();

    let src = format!(
        "@table(\"fitz_orm_empty_agg\") type Row {{\n  \
             id: Int\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // WHERE imposible (age > 999) → set vacío.\n  \
             let q = Row.where(fn(r) => r.age > 999)\n  \
             let total = q.sum(fn(r) => r.age, db).await?\n  \
             return Ok({{ \"sum\": total }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        other => panic!("esperaba Ok, fue {:?}", other),
    };
    let sum = match outer_map {
        Value::Map(s) => {
            let map = s.lock();
            map.iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == "sum"))
                .map(|(_, v)| v.clone())
                .unwrap()
        }
        other => panic!("esperaba Map, fue {:?}", other),
    };
    assert_eq!(sum, Value::Null, "SUM sobre set vacío debe ser Null");

    let _ = seed.exec("DROP TABLE fitz_orm_empty_agg", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_group_by_count_y_sum_e2e() {
    // E2E del GROUP BY: tabla con users de varios países, group_by
    // country + count → cuántos users por país; group_by country +
    // sum(age) → suma de edades por país.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_groupby_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_groupby_test (id bigint, name text, country text, age int)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_groupby_test VALUES \
         (1, 'ada', 'UK', 30), \
         (2, 'alan', 'UK', 42), \
         (3, 'grace', 'US', 55), \
         (4, 'admin', 'US', 99), \
         (5, 'edsger', 'NL', 60)",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_groupby_test\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             country: Str\n  \
             age: Int\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // Count por país.\n  \
             let counts = User.where(fn(u) => u.id > 0)\n    \
                 .group_by(fn(u) => u.country)\n    \
                 .count(db).await?\n  \
             // Sum de edades por país.\n  \
             let sums = User.where(fn(u) => u.id > 0)\n    \
                 .group_by(fn(u) => u.country)\n    \
                 .sum(fn(u) => u.age, db).await?\n  \
             return Ok({{ \"counts\": counts, \"sums\": sums }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };

    // counts: [{country: "UK", count: 2}, {country: "US", count: 2}, {country: "NL", count: 1}]
    let (counts_by_country, sums_by_country) = {
        let m = match outer_map {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        };
        let map = m.lock();
        let get_list = |key: &str| -> Vec<(String, i64)> {
            let list_val = map
                .iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .map(|(_, v)| v.clone())
                .unwrap();
            let list = match list_val {
                Value::List(s) => s,
                other => panic!("esperaba List, fue {:?}", other),
            };
            let items = list.lock();
            items
                .iter()
                .map(|item| {
                    let mm = match item {
                        Value::Map(s) => s,
                        other => panic!("esperaba Map (row), fue {:?}", other),
                    };
                    let map = mm.lock();
                    let country = map
                        .iter()
                        .find(|(k, _)| matches!(k, Value::Str(s) if s == "country"))
                        .and_then(|(_, v)| match v {
                            Value::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap();
                    let agg = map
                        .iter()
                        .find(|(k, _)| matches!(k, Value::Str(s) if s == "count" || s == "sum"))
                        .and_then(|(_, v)| match v {
                            Value::Int(n) => Some(*n),
                            _ => None,
                        })
                        .unwrap();
                    (country, agg)
                })
                .collect()
        };
        (get_list("counts"), get_list("sums"))
    };

    // Verificamos sin asumir orden (Postgres no garantiza orden sin
    // ORDER BY explícito).
    let mut counts_sorted = counts_by_country.clone();
    counts_sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        counts_sorted,
        vec![
            ("NL".to_string(), 1),
            ("UK".to_string(), 2),
            ("US".to_string(), 2)
        ]
    );

    let mut sums_sorted = sums_by_country.clone();
    sums_sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        sums_sorted,
        vec![
            ("NL".to_string(), 60),
            ("UK".to_string(), 72),  // 30+42
            ("US".to_string(), 154), // 55+99
        ]
    );

    let _ = seed.exec("DROP TABLE fitz_orm_groupby_test", &[]).await;
    seed.close().await.unwrap();
}

// =============================================================
// Fase 10.5.a — JSONB con marshalling automático (Map ↔ jsonb)
// =============================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_jsonb_insert_select_round_trip() {
    // Tabla con una columna jsonb. El field Fitz `data: Map<Str, Any>`
    // se mapea automático. INSERT serializa el Map a JSON + cast
    // `::jsonb`. SELECT parsea el JSON text de vuelta a Map Fitz.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_jsonb_test", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_jsonb_test (id bigint PRIMARY KEY, name text, data jsonb)",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_jsonb_test\") type User {{\n  \
             @primary\n  \
             id: Int\n  \
             name: Str\n  \
             data: Map<Str, Any>\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // INSERT con un Map anidado.\n  \
             let payload = {{ \"role\": \"admin\", \"likes\": 42, \"prefs\": {{ \"theme\": \"dark\", \"lang\": \"es\" }} }}\n  \
             let inserted = User.insert(db, User {{ id: 1, name: \"ada\", data: payload }}).await?\n  \
             // SELECT por id.\n  \
             let fetched = User.where(fn(u) => u.id == 1).first(db).await?\n  \
             // UPDATE sobre el field jsonb.\n  \
             let updated = User.where(fn(u) => u.id == 1).update(db, {{ \"data\": {{ \"role\": \"user\", \"new_field\": true }} }}).await?\n  \
             let after_update = User.where(fn(u) => u.id == 1).first(db).await?\n  \
             return Ok({{ \"inserted_data\": inserted.data, \"fetched_data\": fetched.data, \"updated_rows\": updated, \"after_data\": after_update.data }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };

    // Helpers para inspeccionar shape JSON anidado.
    fn get_str_key(m: &Value, key: &str) -> Option<String> {
        if let Value::Map(s) = m {
            let map = s.lock();
            map.iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .and_then(|(_, v)| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
        } else {
            None
        }
    }
    fn get_int_key(m: &Value, key: &str) -> Option<i64> {
        if let Value::Map(s) = m {
            let map = s.lock();
            map.iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .and_then(|(_, v)| match v {
                    Value::Int(n) => Some(*n),
                    _ => None,
                })
        } else {
            None
        }
    }
    fn get_nested(m: &Value, key: &str) -> Option<Value> {
        if let Value::Map(s) = m {
            let map = s.lock();
            map.iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .map(|(_, v)| v.clone())
        } else {
            None
        }
    }

    let (inserted_data, fetched_data, updated_rows, after_data) = {
        let m = match outer_map {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        };
        let map = m.lock();
        let get = |key: &str| {
            map.iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        (
            get("inserted_data"),
            get("fetched_data"),
            get("updated_rows"),
            get("after_data"),
        )
    };

    // inserted_data: round-trip via RETURNING — Postgres devuelve el
    // JSON normalizado pero el shape se preserva.
    assert_eq!(
        get_str_key(&inserted_data, "role").as_deref(),
        Some("admin")
    );
    assert_eq!(get_int_key(&inserted_data, "likes"), Some(42));
    // Nested Map preservado.
    let prefs = get_nested(&inserted_data, "prefs").expect("prefs");
    assert_eq!(get_str_key(&prefs, "theme").as_deref(), Some("dark"));
    assert_eq!(get_str_key(&prefs, "lang").as_deref(), Some("es"));

    // fetched_data: re-read del server, mismo shape.
    assert_eq!(get_str_key(&fetched_data, "role").as_deref(), Some("admin"));
    assert_eq!(get_int_key(&fetched_data, "likes"), Some(42));

    // UPDATE afectó 1 row.
    assert_eq!(updated_rows, Value::Int(1));

    // after_data: el Map nuevo del UPDATE reemplazó al anterior
    // completo (UPDATE de un field jsonb sobreescribe).
    assert_eq!(get_str_key(&after_data, "role").as_deref(), Some("user"));
    // El "new_field": true debe estar.
    if let Value::Map(s) = &after_data {
        let map = s.lock();
        let found = map
            .iter()
            .any(|(k, v)| matches!((k, v), (Value::Str(s), Value::Bool(true)) if s == "new_field"));
        assert!(found, "esperaba new_field: true en after_data");
    }

    let _ = seed.exec("DROP TABLE fitz_orm_jsonb_test", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_jsonb_nullable_acepta_null() {
    // Field `Map<Str, Any>?` nullable: el Value::Null se manda como
    // NULL real (no como string "null") gracias al short-circuit
    // en `fitz_value_to_jsonb`.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_jsonb_null", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_jsonb_null (id bigint PRIMARY KEY, data jsonb)",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_jsonb_null\") type Row {{\n  \
             @primary\n  \
             id: Int\n  \
             data: Map<Str, Any>?\n\
         }}\n\
         async fn run() -> Result<Bool> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // Insert con data = null (nullable).\n  \
             let _ = Row.insert(db, Row {{ id: 1, data: null }}).await?\n  \
             // Verificar via SELECT que volvió Null.\n  \
             let fetched = Row.where(fn(r) => r.id == 1).first(db).await?\n  \
             // fetched.data debería ser Null.\n  \
             match fetched.data {{\n    \
                 null => return Ok(true)\n    \
                 _ => return Ok(false)\n  \
             }}\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => match *boxed {
            Value::Bool(true) => {}
            other => panic!("esperaba Ok(true), fue {:?}", other),
        },
        other => panic!("esperaba Ok, fue {:?}", other),
    }

    let _ = seed.exec("DROP TABLE fitz_orm_jsonb_null", &[]).await;
    seed.close().await.unwrap();
}

// =============================================================
// Fase 10.5.g — Operadores extendidos (and/or/not/is_in/filters)
// =============================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_where_and_or_not_e2e() {
    // Combinación booleana: (age > 18 AND country = "UK") OR
    // (NOT (deleted)). Validamos contra Postgres real.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed.exec("DROP TABLE IF EXISTS fitz_orm_bool", &[]).await;
    seed.exec(
        "CREATE TABLE fitz_orm_bool (id bigint, name text, country text, age int, deleted bool)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_bool VALUES \
         (1, 'ada', 'UK', 30, false), \
         (2, 'alan', 'UK', 42, false), \
         (3, 'edsger', 'NL', 60, true), \
         (4, 'grace', 'US', 55, false), \
         (5, 'kid', 'UK', 10, false)",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_bool\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             country: Str\n  \
             age: Int\n  \
             deleted: Bool\n\
         }}\n\
         async fn run() -> Result<List<User>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // (age > 18 AND country == \"UK\") OR (NOT deleted == false)\n  \
             // Reformulado: matchea UK adultos (ada, alan) + edsger\n  \
             // (deleted=true → NOT deleted=false es false; ¡ojo!).\n  \
             // Simplifico: UK adultos OR deleted=true → ada, alan, edsger.\n  \
             let result = User\n    \
                 .where(fn(u) => (u.country == \"UK\" and u.age > 18) or u.deleted == true)\n    \
                 .order_by(fn(u) => u.id)\n    \
                 .all(db).await?\n  \
             return Ok(result)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let users_list = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let names: Vec<String> = {
        let users_shared = match users_list {
            Value::List(s) => s,
            other => panic!("esperaba List, fue {:?}", other),
        };
        let users = users_shared.lock();
        users
            .iter()
            .filter_map(|u| match u {
                Value::Instance { fields, .. } => fields
                    .lock()
                    .iter()
                    .find(|(n, _)| n == "name")
                    .and_then(|(_, v)| match v {
                        Value::Str(s) => Some(s.clone()),
                        _ => None,
                    }),
                _ => None,
            })
            .collect()
    };
    // ada (UK, 30, false) + alan (UK, 42, false) + edsger (NL, 60, true).
    // Excluidos: grace (US, no UK, not deleted), kid (UK, no adulto).
    assert_eq!(
        names,
        vec!["ada".to_string(), "alan".to_string(), "edsger".to_string()]
    );

    let _ = seed.exec("DROP TABLE fitz_orm_bool", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_where_filters_completos_e2e() {
    // Test combinado de is_in / is_null / starts_with / contains.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_filters", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_filters (id bigint, name text, country text, deleted_at timestamp)",
        &[],
    )
    .await
    .unwrap();
    seed.exec(
        "INSERT INTO fitz_orm_filters VALUES \
         (1, 'ada lovelace', 'UK', null), \
         (2, 'alan turing', 'UK', null), \
         (3, 'edsger dijkstra', 'NL', '2002-08-06'), \
         (4, 'grace hopper', 'US', null), \
         (5, 'admin user', 'XX', '2020-01-01')",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_filters\") type User {{\n  \
             id: Int\n  \
             name: Str\n  \
             country: Str\n  \
             deleted_at: Str?\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             // is_in + is_null: usuarios activos (deleted_at NULL)\n  \
             // en países seleccionados.\n  \
             let active_uk_us = User\n    \
                 .where(fn(u) => u.country.is_in([\"UK\", \"US\"]) and u.deleted_at.is_null())\n    \
                 .order_by(fn(u) => u.id)\n    \
                 .all(db).await?\n  \
             // starts_with: nombres que arrancan con \"a\".\n  \
             let starts_a = User\n    \
                 .where(fn(u) => u.name.starts_with(\"a\"))\n    \
                 .order_by(fn(u) => u.id)\n    \
                 .all(db).await?\n  \
             // contains: nombres que contienen \"o\".\n  \
             let contains_o = User\n    \
                 .where(fn(u) => u.name.contains(\"o\"))\n    \
                 .order_by(fn(u) => u.id)\n    \
                 .all(db).await?\n  \
             return Ok({{ \"active_uk_us\": active_uk_us, \"starts_a\": starts_a, \"contains_o\": contains_o }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let outer_map = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };

    fn names_in_map(outer: &Value, key: &str) -> Vec<String> {
        let m = match outer {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        };
        let map = m.lock();
        let list_val = map
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
            .map(|(_, v)| v.clone())
            .unwrap();
        let list = match list_val {
            Value::List(s) => s,
            other => panic!("esperaba List, fue {:?}", other),
        };
        let items = list.lock();
        items
            .iter()
            .filter_map(|u| match u {
                Value::Instance { fields, .. } => fields
                    .lock()
                    .iter()
                    .find(|(n, _)| n == "name")
                    .and_then(|(_, v)| match v {
                        Value::Str(s) => Some(s.clone()),
                        _ => None,
                    }),
                _ => None,
            })
            .collect()
    }

    let active = names_in_map(&outer_map, "active_uk_us");
    let starts_a = names_in_map(&outer_map, "starts_a");
    let contains_o = names_in_map(&outer_map, "contains_o");

    // active_uk_us: ada (UK, null), alan (UK, null), grace (US, null).
    assert_eq!(
        active,
        vec![
            "ada lovelace".to_string(),
            "alan turing".to_string(),
            "grace hopper".to_string()
        ]
    );
    // starts_a: ada, alan, admin user.
    assert_eq!(
        starts_a,
        vec![
            "ada lovelace".to_string(),
            "alan turing".to_string(),
            "admin user".to_string()
        ]
    );
    // contains_o: ada lovelace (lOvelace), grace hopper (hOpper).
    // alan turing: NO contiene 'o' (a-l-a-n-t-u-r-i-n-g).
    // edsger dijkstra: NO contiene 'o'.
    // admin user: NO contiene 'o'.
    assert_eq!(
        contains_o,
        vec!["ada lovelace".to_string(), "grace hopper".to_string()]
    );

    let _ = seed.exec("DROP TABLE fitz_orm_filters", &[]).await;
    seed.close().await.unwrap();
}

// =============================================================
// Fase 10.5.b — Arrays nativos (List<T> ↔ T[])
// =============================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_arrays_int_e2e() {
    // Tabla con columna `tags bigint[]`. El field Fitz `tags: List<Int>`
    // se mapea automático: INSERT serializa a `{1,2,3}::int8[]`,
    // SELECT parsea el text de vuelta a Value::List.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_arr_int", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_arr_int (id bigint PRIMARY KEY, name text, tags bigint[])",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_arr_int\") type Item {{\n  \
             @primary\n  \
             id: Int\n  \
             name: Str\n  \
             tags: List<Int>\n\
         }}\n\
         async fn run() -> Result<Item> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let inserted = Item.insert(db, Item {{ id: 1, name: \"alpha\", tags: [10, 20, 30] }}).await?\n  \
             let fetched = Item.where(fn(i) => i.id == 1).first(db).await?\n  \
             return Ok(fetched)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let item = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => *boxed,
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let inst_shared = match item {
        Value::Instance { fields, .. } => fields,
        other => panic!("esperaba Instance, fue {:?}", other),
    };
    {
        let inst = inst_shared.lock();
        let tags = inst
            .iter()
            .find(|(n, _)| n == "tags")
            .map(|(_, v)| v.clone())
            .unwrap();
        let items_shared = match tags {
            Value::List(s) => s,
            other => panic!("esperaba List, fue {:?}", other),
        };
        let items = items_shared.lock();
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0], Value::Int(10)));
        assert!(matches!(items[1], Value::Int(20)));
        assert!(matches!(items[2], Value::Int(30)));
    }

    let _ = seed.exec("DROP TABLE fitz_orm_arr_int", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_arrays_text_e2e() {
    // Tabla con columna `tags text[]`. Strings con caracteres
    // especiales (commas, quotes) round-trip OK.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_arr_text", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_arr_text (id bigint PRIMARY KEY, tags text[])",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_arr_text\") type Item {{\n  \
             @primary\n  \
             id: Int\n  \
             tags: List<Str>\n\
         }}\n\
         async fn run() -> Result<Item> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let inserted = Item.insert(db, Item {{ id: 1, tags: [\"hello\", \"a,b,c\", \"con espacios\"] }}).await?\n  \
             let fetched = Item.where(fn(i) => i.id == 1).first(db).await?\n  \
             return Ok(fetched)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let inst = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => match *boxed {
            Value::Instance { fields, .. } => fields,
            other => panic!("esperaba Instance, fue {:?}", other),
        },
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let strs: Vec<String> = {
        let inst = inst.lock();
        let tags = inst
            .iter()
            .find(|(n, _)| n == "tags")
            .map(|(_, v)| v.clone())
            .unwrap();
        let items_shared = match tags {
            Value::List(s) => s,
            other => panic!("esperaba List, fue {:?}", other),
        };
        let items = items_shared.lock();
        items
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.clone(),
                other => panic!("esperaba Str, fue {:?}", other),
            })
            .collect()
    };
    assert_eq!(strs, vec!["hello", "a,b,c", "con espacios"]);

    let _ = seed.exec("DROP TABLE fitz_orm_arr_text", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_arrays_vacio_y_nullable_e2e() {
    // (a) Array vacío `[]` round-trip OK como `{}`.
    // (b) Nullable `List<Int>?` con `null` → NULL real en Postgres.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_arr_null", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_arr_null (id bigint PRIMARY KEY, ints bigint[], opt_ints bigint[])",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_arr_null\") type Row {{\n  \
             @primary\n  \
             id: Int\n  \
             ints: List<Int>\n  \
             opt_ints: List<Int>?\n\
         }}\n\
         async fn run() -> Result<Map<Str, Any>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let _ = Row.insert(db, Row {{ id: 1, ints: [], opt_ints: null }}).await?\n  \
             let r = Row.where(fn(x) => x.id == 1).first(db).await?\n  \
             return Ok({{ \"ints\": r.ints, \"opt_ints\": r.opt_ints }})\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let m = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => match *boxed {
            Value::Map(s) => s,
            other => panic!("esperaba Map, fue {:?}", other),
        },
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let (ints, opt_ints) = {
        let m = m.lock();
        let ints = m
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "ints"))
            .map(|(_, v)| v.clone())
            .unwrap();
        let opt_ints = m
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == "opt_ints"))
            .map(|(_, v)| v.clone())
            .unwrap();
        (ints, opt_ints)
    };

    // ints = [] vacío
    match ints {
        Value::List(s) => {
            let items = s.lock();
            assert!(items.is_empty(), "esperaba lista vacía");
        }
        other => panic!("esperaba List, fue {:?}", other),
    }
    // opt_ints = null (NULL real)
    assert!(matches!(opt_ints, Value::Null));

    let _ = seed.exec("DROP TABLE fitz_orm_arr_null", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_arrays_update_e2e() {
    // UPDATE con un campo array reemplaza el array completo.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_arr_upd", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_arr_upd (id bigint PRIMARY KEY, tags text[])",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_arr_upd\") type Item {{\n  \
             @primary\n  \
             id: Int\n  \
             tags: List<Str>\n\
         }}\n\
         async fn run() -> Result<Item> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let _ = Item.insert(db, Item {{ id: 1, tags: [\"a\", \"b\"] }}).await?\n  \
             let _ = Item.where(fn(i) => i.id == 1).update(db, {{ \"tags\": [\"c\", \"d\", \"e\"] }}).await?\n  \
             let updated = Item.where(fn(i) => i.id == 1).first(db).await?\n  \
             return Ok(updated)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let inst = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => match *boxed {
            Value::Instance { fields, .. } => fields,
            other => panic!("esperaba Instance, fue {:?}", other),
        },
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let strs: Vec<String> = {
        let inst = inst.lock();
        let tags = inst
            .iter()
            .find(|(n, _)| n == "tags")
            .map(|(_, v)| v.clone())
            .unwrap();
        let items_shared = match tags {
            Value::List(s) => s,
            _ => panic!(),
        };
        let items = items_shared.lock();
        items
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.clone(),
                _ => panic!(),
            })
            .collect()
    };
    assert_eq!(strs, vec!["c", "d", "e"]);

    let _ = seed.exec("DROP TABLE fitz_orm_arr_upd", &[]).await;
    seed.close().await.unwrap();
}

// =============================================================
// Fase 10.5.c — Date/Time/Timestamp/UUID (round-trip como Str)
// =============================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_date_time_timestamp_e2e() {
    // Tabla con columnas date, time, timestamp, timestamptz. Fitz no
    // tiene tipos nativos para fechas — el ORM las round-trip como
    // `Str` con el formato ISO 8601 que Postgres emite y acepta.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed.exec("DROP TABLE IF EXISTS fitz_orm_dt", &[]).await;
    seed.exec(
        "CREATE TABLE fitz_orm_dt (\
            id bigint PRIMARY KEY,\
            d date,\
            t time,\
            ts timestamp,\
            tsz timestamptz\
        )",
        &[],
    )
    .await
    .unwrap();

    let src = format!(
        "@table(\"fitz_orm_dt\") type Event {{\n  \
             @primary\n  \
             id: Int\n  \
             d: Str\n  \
             t: Str\n  \
             ts: Str\n  \
             tsz: Str\n\
         }}\n\
         async fn run() -> Result<Event> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let _ = Event.insert(db, Event {{\n    \
                 id: 1,\n    \
                 d: \"2026-05-25\",\n    \
                 t: \"14:30:00\",\n    \
                 ts: \"2026-05-25 14:30:00\",\n    \
                 tsz: \"2026-05-25 14:30:00+00\"\n  \
             }}).await?\n  \
             let r = Event.where(fn(e) => e.id == 1).first(db).await?\n  \
             return Ok(r)\n\
         }}\n\
         let result = run().await",
        url
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let inst = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => match *boxed {
            Value::Instance { fields, .. } => fields,
            other => panic!("esperaba Instance, fue {:?}", other),
        },
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let (d, t, ts, tsz) = {
        let inst = inst.lock();
        let get_str = |name: &str| -> String {
            inst.iter()
                .find(|(n, _)| n == name)
                .and_then(|(_, v)| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("esperaba field `{}` como Str", name))
        };
        (get_str("d"), get_str("t"), get_str("ts"), get_str("tsz"))
    };

    // Date — Postgres emite `YYYY-MM-DD` siempre.
    assert_eq!(d, "2026-05-25");
    // Time — Postgres emite `HH:MM:SS` (sin microseconds si son 0).
    assert_eq!(t, "14:30:00");
    // Timestamp — Postgres emite `YYYY-MM-DD HH:MM:SS`.
    assert_eq!(ts, "2026-05-25 14:30:00");
    // Timestamptz — Postgres emite con timezone resuelto. El formato
    // exacto depende del session timezone; aceptamos cualquier valor
    // que comience con la fecha base.
    assert!(
        tsz.starts_with("2026-05-25"),
        "esperaba que tsz arranque con la fecha, fue '{}'",
        tsz
    );

    let _ = seed.exec("DROP TABLE fitz_orm_dt", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_uuid_e2e() {
    // Tabla con columna uuid. Fitz la trata como Str con el formato
    // canonical de UUID (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`).
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed.exec("DROP TABLE IF EXISTS fitz_orm_uuid", &[]).await;
    seed.exec(
        "CREATE TABLE fitz_orm_uuid (id uuid PRIMARY KEY, name text)",
        &[],
    )
    .await
    .unwrap();

    let uuid_canonical = "550e8400-e29b-41d4-a716-446655440000";
    let src = format!(
        "@table(\"fitz_orm_uuid\") type Item {{\n  \
             @primary\n  \
             id: Str\n  \
             name: Str\n\
         }}\n\
         async fn run() -> Result<Item> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let _ = db.exec(\"INSERT INTO fitz_orm_uuid (id, name) VALUES ($1::uuid, $2)\", [\"{}\", \"alpha\"]).await?\n  \
             let r = Item.where(fn(i) => i.id == \"{}\").first(db).await?\n  \
             return Ok(r)\n\
         }}\n\
         let result = run().await",
        url, uuid_canonical, uuid_canonical
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let inst = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => match *boxed {
            Value::Instance { fields, .. } => fields,
            other => panic!("esperaba Instance, fue {:?}", other),
        },
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let (id_val, name_val) = {
        let inst = inst.lock();
        let id_val = inst
            .iter()
            .find(|(n, _)| n == "id")
            .map(|(_, v)| v.clone())
            .unwrap();
        let name_val = inst
            .iter()
            .find(|(n, _)| n == "name")
            .map(|(_, v)| v.clone())
            .unwrap();
        (id_val, name_val)
    };
    match id_val {
        Value::Str(s) => assert_eq!(s, uuid_canonical),
        other => panic!("esperaba Str(UUID), fue {:?}", other),
    }
    match name_val {
        Value::Str(s) => assert_eq!(s, "alpha"),
        other => panic!("esperaba Str(alpha), fue {:?}", other),
    }

    let _ = seed.exec("DROP TABLE fitz_orm_uuid", &[]).await;
    seed.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn orm_uuid_array_e2e() {
    // Combina 10.5.b (arrays) + 10.5.c (UUID): columna `uuid[]`.
    // El driver parsea como `Array { elem_oid: UUID, values: [Text, ...] }`
    // y el ORM lo round-trip como `List<Str>` Fitz.
    let url = pg_url();
    let seed = connect_url(&url).await.unwrap();
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_orm_uuid_arr", &[])
        .await;
    seed.exec(
        "CREATE TABLE fitz_orm_uuid_arr (id bigint PRIMARY KEY, ids uuid[])",
        &[],
    )
    .await
    .unwrap();

    let u1 = "550e8400-e29b-41d4-a716-446655440000";
    let u2 = "550e8400-e29b-41d4-a716-446655440001";
    let src = format!(
        "async fn run() -> Result<List<Str>> {{\n  \
             let db = db.connect(\"{}\").await?\n  \
             let _ = db.exec(\"INSERT INTO fitz_orm_uuid_arr (id, ids) VALUES ($1, ARRAY[$2, $3]::uuid[])\", [1, \"{}\", \"{}\"]).await?\n  \
             let rows = db.query(\"SELECT ids FROM fitz_orm_uuid_arr WHERE id = $1\", [1]).await?\n  \
             // rows: List<Map<Str, Any>>. Tomamos rows[0][\"ids\"].\n  \
             let first = rows[0]\n  \
             let ids = first[\"ids\"]\n  \
             return Ok(ids)\n\
         }}\n\
         let result = run().await",
        url, u1, u2
    );
    let tokens = tokenize(&src).expect("tokenize OK");
    let program = parse(tokens).expect("parse OK");
    let env = new_repl_env();
    eval_program_with_env(
        program,
        std::env::current_dir().unwrap(),
        env.clone(),
        Default::default(),
    )
    .await
    .expect("eval OK");

    let result_val = env.lock().get("result").unwrap();
    let items = match result_val {
        Value::Result(fitz::value::ResultVariant::Ok(boxed)) => match *boxed {
            Value::List(s) => s,
            other => panic!("esperaba List, fue {:?}", other),
        },
        Value::Result(fitz::value::ResultVariant::Err(boxed)) => {
            panic!("esperaba Ok, fue Err({:?})", boxed)
        }
        other => panic!("esperaba Result, fue {:?}", other),
    };
    let strs: Vec<String> = {
        let items = items.lock();
        items
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.clone(),
                other => panic!("esperaba Str(UUID), fue {:?}", other),
            })
            .collect()
    };
    assert_eq!(strs, vec![u1.to_string(), u2.to_string()]);

    let _ = seed.exec("DROP TABLE fitz_orm_uuid_arr", &[]).await;
    seed.close().await.unwrap();
}

// =====================================================================
// v0.10.28 (Tier S, sub-paso 1) — `fitz db inspect`: introspect end-to-
// end contra Postgres real. Crea 2 tables con PK + partial unique
// index + FK ON DELETE CASCADE, corre `introspect_schema`, valida
// que el text + json formatters incluyen todo.
// =====================================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn inspect_schema_text_y_json_contienen_todo_el_shape() {
    let url = pg_url();
    let seed = connect_url(&url).await.expect("connect");

    // Cleanup previo (por si una corrida anterior dejó basura).
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_inspect_e2e_posts", &[])
        .await;
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_inspect_e2e_users", &[])
        .await;

    // Setup: users con PK + nullable col + unique partial index;
    // posts con FK ON DELETE CASCADE.
    seed.exec(
        "CREATE TABLE fitz_inspect_e2e_users (\
             id bigserial PRIMARY KEY, \
             email text NOT NULL, \
             deleted_at timestamptz\
         )",
        &[],
    )
    .await
    .expect("CREATE users");
    seed.exec(
        "CREATE UNIQUE INDEX idx_fitz_inspect_e2e_users_email_active \
         ON fitz_inspect_e2e_users(email) WHERE deleted_at IS NULL",
        &[],
    )
    .await
    .expect("CREATE partial index");
    seed.exec(
        "CREATE TABLE fitz_inspect_e2e_posts (\
             id bigserial PRIMARY KEY, \
             author_id bigint NOT NULL REFERENCES fitz_inspect_e2e_users(id) ON DELETE CASCADE, \
             title text NOT NULL\
         )",
        &[],
    )
    .await
    .expect("CREATE posts");

    // Introspect el schema real.
    let schema = fitz::migrations::introspect_schema(&seed)
        .await
        .expect("introspect_schema OK");

    // Text formatter — filtramos por table porque public puede
    // tener basura ajena al test en la DB del dev.
    let text_users =
        fitz::migrations::format_inspection_text(&schema, None, Some("fitz_inspect_e2e_users"));
    assert!(
        text_users.contains("Table: fitz_inspect_e2e_users (3 cols)"),
        "header con count: {text_users}"
    );
    assert!(text_users.contains("id"), "col id presente: {text_users}");
    assert!(text_users.contains("PK"), "PK tag presente: {text_users}");
    assert!(
        text_users.contains("idx_fitz_inspect_e2e_users_email_active"),
        "index name presente: {text_users}"
    );
    assert!(
        text_users.contains("UNIQUE (email)"),
        "UNIQUE label + cols presente: {text_users}"
    );
    assert!(
        text_users.contains("WHERE (deleted_at IS NULL)"),
        "WHERE de partial index presente: {text_users}"
    );

    let text_posts =
        fitz::migrations::format_inspection_text(&schema, None, Some("fitz_inspect_e2e_posts"));
    assert!(
        text_posts.contains("Table: fitz_inspect_e2e_posts (3 cols)"),
        "header posts: {text_posts}"
    );
    assert!(
        text_posts.contains("ON DELETE CASCADE"),
        "FK CASCADE presente: {text_posts}"
    );
    assert!(
        text_posts.contains("author_id -> fitz_inspect_e2e_users(id)"),
        "FK target presente: {text_posts}"
    );

    // JSON formatter — shape estable, parseable, contiene todo.
    let json_str =
        fitz::migrations::format_inspection_json(&schema, None, Some("fitz_inspect_e2e_users"))
            .expect("format json OK");
    let v: serde_json::Value = serde_json::from_str(&json_str).expect("valid JSON");
    assert_eq!(v["schema"], "public");
    let tables = v["tables"].as_array().expect("tables array");
    assert_eq!(tables.len(), 1, "filter por table devuelve 1");
    let users_v = &tables[0];
    assert_eq!(users_v["name"], "fitz_inspect_e2e_users");
    assert_eq!(users_v["primary_key"], serde_json::json!(["id"]));
    let cols = users_v["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 3);
    // id es PK + bigint + NOT NULL.
    let id_col = cols.iter().find(|c| c["name"] == "id").expect("id col");
    assert_eq!(id_col["sql_type"], "bigint");
    assert_eq!(id_col["nullable"], false);
    assert_eq!(id_col["is_primary"], true);
    // deleted_at es NULL.
    let del = cols
        .iter()
        .find(|c| c["name"] == "deleted_at")
        .expect("deleted_at");
    assert_eq!(del["nullable"], true);
    // Index con where_clause.
    let idx = users_v["indexes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "idx_fitz_inspect_e2e_users_email_active")
        .expect("partial index in json");
    assert_eq!(idx["unique"], true);
    assert!(idx["where_clause"]
        .as_str()
        .unwrap()
        .contains("deleted_at IS NULL"));

    // Cleanup.
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_inspect_e2e_posts", &[])
        .await;
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_inspect_e2e_users", &[])
        .await;
    seed.close().await.unwrap();
}

// =====================================================================
// v0.10.28 (Tier S, sub-paso 2) — @index(col, using="gin") method
// override. Validamos round-trip: el SQL emitido por changes_to_sql
// se aplica OK contra Postgres real (sintaxis válida), y la introspect
// devuelve el method (`gin`) en el `using` del Index.
// =====================================================================

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn introspect_y_diff_round_trip_using_gin_method() {
    use fitz::migrations::{Change, Index, TableRef};

    let url = pg_url();
    let seed = connect_url(&url).await.expect("connect");

    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_using_e2e_docs", &[])
        .await;

    // Setup: table con tsvector col (target natural de gin).
    seed.exec(
        "CREATE TABLE fitz_using_e2e_docs (\
             id bigserial PRIMARY KEY, \
             body tsvector NOT NULL\
         )",
        &[],
    )
    .await
    .expect("CREATE table");

    // Aplicamos el SQL que el migrator emitiría a partir de
    // `@index("body", using="gin")` — validamos que la sintaxis es
    // válida contra Postgres real (no solo unit test del string).
    let create_change = Change::CreateIndex {
        table: TableRef {
            schema: None,
            name: "fitz_using_e2e_docs".to_string(),
        },
        index: Index {
            name: "idx_fitz_using_e2e_docs_body_gin".to_string(),
            columns: vec!["body".to_string()],
            unique: false,
            where_clause: None,
            using: Some("gin".to_string()),
        },
    };
    let sql = fitz::migrations::changes_to_sql(&[create_change]);
    seed.exec(&sql, &[])
        .await
        .expect("CREATE INDEX ... USING gin debe ser válido");

    // Introspect debe reportar el method "gin".
    let schema = fitz::migrations::introspect_schema(&seed)
        .await
        .expect("introspect");
    let docs_table = schema
        .tables
        .iter()
        .find(|t| t.name == "fitz_using_e2e_docs")
        .expect("table fitz_using_e2e_docs presente");
    let idx = docs_table
        .indexes
        .iter()
        .find(|i| i.name == "idx_fitz_using_e2e_docs_body_gin")
        .expect("gin index introspectado");
    assert_eq!(
        idx.using.as_deref(),
        Some("gin"),
        "introspect debería reportar using=gin, got {:?}",
        idx.using
    );

    // Cross-check: format_inspection_text muestra USING gin.
    let text = fitz::migrations::format_inspection_text(&schema, None, Some("fitz_using_e2e_docs"));
    assert!(
        text.contains("USING gin"),
        "inspect text debería contener `USING gin`: {text}"
    );

    // Cross-check: format_inspection_json devuelve "using":"gin".
    let json_str =
        fitz::migrations::format_inspection_json(&schema, None, Some("fitz_using_e2e_docs"))
            .expect("json");
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(v["tables"][0]["indexes"][0]["using"], "gin");

    // Cleanup.
    let _ = seed
        .exec("DROP TABLE IF EXISTS fitz_using_e2e_docs", &[])
        .await;
    seed.close().await.unwrap();
}
