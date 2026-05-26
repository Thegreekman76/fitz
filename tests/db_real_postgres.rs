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

#[tokio::test]
#[ignore]
async fn pool_queries_concurrentes_no_se_serializan() {
    // Test del pool: lanzamos N queries en paralelo y validamos
    // que el tiempo total << N * tiempo_individual (serializado).
    // Cada query hace pg_sleep(0.1) para forzar I/O wait.
    use std::sync::Arc;
    let url = pg_url();
    let conn = Arc::new(connect_url(&url).await.unwrap());

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
async fn seed_orm_test_table(url: &str) -> fitz::db::DbConnHandle {
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
