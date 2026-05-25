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
