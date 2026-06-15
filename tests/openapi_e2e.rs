// openapi_e2e.rs — Tests integration de Fase 7.1.
//
// Ejecuta `fitz openapi archivo.fitz` y valida que el JSON producido
// contenga la estructura esperada (estructura top-level OpenAPI 3.1
// + las rutas declaradas en el .fitz).
//
// Estos tests no invocan rustc — solo el binario `fitz`. Son rápidos
// (~10ms cada uno) y no requieren serialización con `cargo build`.

use std::process::Command;

fn fitz_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fitz")
}

/// Escribe un .fitz en un tempdir único, ejecuta `fitz openapi` y
/// devuelve el JSON parseado.
fn run_openapi(test_name: &str, src: &str) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("fitz-openapi-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["openapi"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz openapi");
    assert!(
        output.status.success(),
        "fitz openapi failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "fitz openapi stdout is not valid JSON: {}\nstdout:\n{}",
            e, stdout
        )
    })
}

#[test]
fn fitz_openapi_basic_crud_emits_complete_schema() {
    // CRUD chiquito con un tipo custom, un GET con path param y
    // return Result, un POST con body. Cubre lo necesario para
    // verificar el cableado E2E del subcomando.
    let src = r#"
type User { id: Int, name: Str }
@get("/users/{id}")
fn get_user(id: Int) -> Result<User> => Ok(User { id: id, name: "Fitz" })
@post("/users")
fn create_user(body: User) -> User => body
"#;
    let schema = run_openapi("crud_basico", src);

    // Top-level: OpenAPI 3.1 + info estándar.
    assert_eq!(schema["openapi"], serde_json::json!("3.1.0"));
    assert_eq!(schema["info"]["title"], serde_json::json!("Fitz API"));

    // components.schemas.User con properties id + name + required.
    let user_schema = &schema["components"]["schemas"]["User"];
    assert_eq!(user_schema["type"], serde_json::json!("object"));
    assert!(user_schema["properties"]["id"].is_object());
    assert!(user_schema["properties"]["name"].is_object());
    let required = user_schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("id")));
    assert!(required.contains(&serde_json::json!("name")));

    // GET /users/{id} con path param Int y responses 200 + 500 (Result).
    let get = &schema["paths"]["/users/{id}"]["get"];
    assert_eq!(get["operationId"], serde_json::json!("get_user"));
    let params = get["parameters"].as_array().unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["name"], serde_json::json!("id"));
    assert_eq!(params[0]["in"], serde_json::json!("path"));
    assert!(get["responses"]["200"].is_object());
    assert!(get["responses"]["500"].is_object());

    // POST /users con body de User (ref a components.schemas.User).
    let post = &schema["paths"]["/users"]["post"];
    assert_eq!(post["operationId"], serde_json::json!("create_user"));
    let body_schema = &post["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(
        body_schema,
        &serde_json::json!({ "$ref": "#/components/schemas/User" })
    );
}

#[test]
fn fitz_openapi_program_without_routes_emits_empty_paths() {
    // Programa sin decoradores HTTP — válido, pero el schema viene
    // con `paths` vacío. Las definiciones top-level (un `type`) sí
    // entran a `components.schemas` aunque ningún handler las use.
    //
    // Nota: el comando ejecuta el programa entero antes de emitir el
    // schema (los decoradores HTTP son side-effects del top-level).
    // Para mantener el stdout limpio el src no debe imprimir nada.
    let src = "type Empty { id: Int }\nlet x = 42\n";
    let schema = run_openapi("sin_rutas", src);
    assert_eq!(schema["openapi"], serde_json::json!("3.1.0"));
    let paths = schema["paths"].as_object().unwrap();
    assert!(paths.is_empty());
    // El tipo declarado sí aparece en components.schemas.
    assert!(schema["components"]["schemas"]["Empty"].is_object());
}

#[test]
fn fitz_openapi_aborts_with_type_errors() {
    // Programa con error de tipo: el handler retorna Int pero la
    // anotación dice Str. `fitz openapi` corre en modo strict (igual
    // que `fitz build`) — no tiene sentido emitir schema de un
    // programa que no tipa.
    let src = "@get(\"/x\")\nfn h() -> Str => 42\n";
    let dir = std::env::temp_dir().join("fitz-openapi-typecheck-fail");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("write .fitz");

    let output = Command::new(fitz_bin())
        .args(["openapi"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz openapi");
    assert!(
        !output.status.success(),
        "expected fitz openapi to fail due to type error, exited OK"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error") && stderr.contains("type"),
        "expected stderr mentioning type errors, was: {}",
        stderr
    );
}
