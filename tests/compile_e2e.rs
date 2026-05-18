// compile_e2e.rs — Tests integration de Fase 5b.1.
//
// Toma un programa Fitz, llama a `fitz build`, ejecuta el binario y
// chequea stdout / exit code. Los tests usan un directorio temporal
// único por test (concatenando el nombre del test) para no pisar
// builds entre corridas.
//
// Importante: estos tests **invocan rustc** internamente vía `fitz
// build`. Son más lentos que los unitarios; cada uno toma ~2s.

use std::process::Command;
use std::sync::Mutex;

/// Mutex global: los tests de este archivo corren serializados.
/// `fitz build` invoca rustc, y múltiples rustc en paralelo sobre
/// el mismo target dir pueden chocar (los runs paralelos
/// observados producían cross-talk entre binarios). Cada test
/// toma el lock antes de buildear/ejecutar.
static SERIAL: Mutex<()> = Mutex::new(());

/// Path al binario de `fitz` que cargo construye para los
/// integration tests (depende de CARGO_BIN_EXE_<bin>).
fn fitz_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fitz")
}

/// Crea un directorio temporal único para el test, escribe el
/// .fitz, compila con `fitz build`, ejecuta el binario y devuelve
/// (stdout, exit_code).
fn build_and_run(test_name: &str, src: &str) -> (String, i32) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    // Build.
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        output.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Path del binario generado: adyacente al .fitz.
    let bin_name = if cfg!(windows) { "prog.exe" } else { "prog" };
    let bin = dir.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

/// Como `build_and_run` pero asume que el build va a fallar.
/// Devuelve el stderr del fitz build.
fn build_expect_fail(test_name: &str, src: &str) -> String {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        !output.status.success(),
        "esperaba que fitz build fallara, pero salió OK:\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_lines(stdout: &str, expected: &[&str]) {
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        expected.len(),
        "esperaba {} líneas, hubo {}: {:?}",
        expected.len(),
        lines.len(),
        lines
    );
    for (i, (l, e)) in lines.iter().zip(expected.iter()).enumerate() {
        assert_eq!(l, e, "línea {} difiere", i + 1);
    }
}

// ---------------------------------------------------------------------------
// Tests del criterio de éxito y casos secundarios
// ---------------------------------------------------------------------------

#[test]
fn criterio_de_exito_hello_world_compilado() {
    let src = "\
let name = \"Fitz\"
let x = 10 + 5
print(\"Hola, {name}, x es {x}\")

fn double(n: Int) -> Int => n * 2
print(double(x))
";
    let (stdout, exit) = build_and_run("hello-world", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["Hola, Fitz, x es 15", "30"]);
}

#[test]
fn if_else_funciona_en_binario() {
    let src = "\
let x = 5
if (x > 0) { print(\"pos\") } else { print(\"neg\") }
";
    let (stdout, exit) = build_and_run("if-else", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["pos"]);
}

#[test]
fn while_y_reasignacion_funcionan_en_binario() {
    let src = "\
let n = 0
while (n < 3) {
    print(n)
    n = n + 1
}
";
    let (stdout, exit) = build_and_run("while-reassign", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["0", "1", "2"]);
}

#[test]
fn for_in_range_funciona_en_binario() {
    let src = "\
for i in 0..4 {
    print(i)
}
";
    let (stdout, exit) = build_and_run("for-range", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["0", "1", "2", "3"]);
}

#[test]
fn coercion_int_a_float_funciona_en_binario() {
    let src = "\
let pi: Float = 3
let r: Float = pi * 2.0
print(r)
";
    let (stdout, exit) = build_and_run("coerce-int-float", src);
    assert_eq!(exit, 0);
    // 5b.2 alinea el formato de Float con el intérprete: las
    // fracciones .0 se imprimen explícitas (`6.0`, no `6`).
    assert_lines(&stdout, &["6.0"]);
}

#[test]
fn recursion_funciona_en_binario() {
    let src = "\
fn fact(n: Int) -> Int {
    if (n <= 1) { return 1 }
    return n * fact(n - 1)
}
print(fact(5))
";
    let (stdout, exit) = build_and_run("recursion", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["120"]);
}

// ---------------------------------------------------------------------------
// Fase 5b.2 — tipos custom
// ---------------------------------------------------------------------------

#[test]
fn instancia_basica_round_trip_compilado() {
    let src = "\
type User { id: Int, name: Str }
let u = User { id: 1, name: \"Fitz\" }
print(u.id)
print(u.name)
";
    let (stdout, exit) = build_and_run("instance-basic", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["1", "Fitz"]);
}

#[test]
fn defaults_aplicados_en_binario() {
    let src = "\
type Config { host: Str, port: Int = 3000, debug: Bool = false }
let c = Config { host: \"localhost\" }
print(c.port)
print(c.debug)
";
    let (stdout, exit) = build_and_run("defaults", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["3000", "false"]);
}

#[test]
fn nullable_omitido_imprime_null_en_binario() {
    let src = "\
type User { id: Int, email: Str? }
let u = User { id: 1 }
print(u.email)
";
    let (stdout, exit) = build_and_run("nullable-omitted", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["null"]);
}

#[test]
fn field_mutation_visible_via_alias_compilado() {
    // Semántica de referencia compartida: mutar a través de un
    // alias se ve en la variable original. Mismo modelo que el
    // intérprete (Rc<RefCell<>>).
    let src = "\
type User { id: Int, name: Str }
let a = User { id: 1, name: \"uno\" }
let b = a
b.name = \"dos\"
print(a.name)
print(b.name)
";
    let (stdout, exit) = build_and_run("alias-mutation", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["dos", "dos"]);
}

#[test]
fn fn_que_muta_param_refleja_afuera_compilado() {
    let src = "\
type User { name: Str }
fn rename(u: User, n: Str) {
    u.name = n
}
let u = User { name: \"uno\" }
rename(u, \"dos\")
print(u.name)
";
    let (stdout, exit) = build_and_run("fn-mutates-param", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["dos"]);
}

#[test]
fn print_instance_formato_canonico_compilado() {
    // El Display de `UserData` debe reproducir el formato del
    // intérprete: `User { id: 1, name: "Fitz", email: null }`.
    let src = "\
type User { id: Int, name: Str, email: Str? }
let u = User { id: 1, name: \"Fitz\" }
print(u)
";
    let (stdout, exit) = build_and_run("instance-display", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["User { id: 1, name: \"Fitz\", email: null }"],
    );
}

#[test]
fn igualdad_estructural_entre_instancias_compilado() {
    // Dos instancias con los mismos campos comparan true; con
    // un campo distinto, false. Recursa adentro de campos
    // nominales anidados gracias al derive(PartialEq) de Rust
    // (Rc<RefCell<T>> compara por contenido).
    let src = "\
type Address { city: Str }
type User { id: Int, name: Str, addr: Address? }

let a1 = User { id: 1, name: \"x\", addr: Address { city: \"El Chaltén\" } }
let a2 = User { id: 1, name: \"x\", addr: Address { city: \"El Chaltén\" } }
let b  = User { id: 2, name: \"x\", addr: Address { city: \"El Chaltén\" } }
let c  = User { id: 1, name: \"x\", addr: Address { city: \"Otro\" } }
print(a1 == a2)
print(a1 == b)
print(a1 == c)
print(a1 != c)
";
    let (stdout, exit) = build_and_run("instance-eq", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["true", "false", "false", "true"]);
}

#[test]
fn tipos_anidados_round_trip_compilado() {
    let src = "\
type User { name: Str }
type Order { id: Int, user: User? }
let u = User { name: \"Fitz\" }
let o = Order { id: 7, user: u }
print(o)
print(o.user)
";
    let (stdout, exit) = build_and_run("nested-types", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "Order { id: 7, user: User { name: \"Fitz\" } }",
            "User { name: \"Fitz\" }",
        ],
    );
}

// ---------------------------------------------------------------------------
// 5b.2+: if como expresión con valor
// ---------------------------------------------------------------------------

#[test]
fn if_como_expresion_con_else_compilado() {
    let src = "\
let active = true
let status = if (active) { \"on\" } else { \"off\" }
print(status)
";
    let (stdout, exit) = build_and_run("if-expr", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["on"]);
}

#[test]
fn if_else_if_chain_como_expresion_compilado() {
    let src = "\
let n = -3
let sign = if (n > 0) { \"positivo\" } else if (n < 0) { \"negativo\" } else { \"cero\" }
print(sign)
";
    let (stdout, exit) = build_and_run("if-elseif-expr", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["negativo"]);
}

#[test]
fn if_expresion_bloque_multilinea_compilado() {
    let src = "\
let total = if (true) {
    let a = 10
    let b = 20
    a + b
} else {
    0
}
print(total)
";
    let (stdout, exit) = build_and_run("if-multiline-expr", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["30"]);
}

#[test]
fn if_expresion_unifica_int_y_float_compilado() {
    let src = "\
let n = 5
let r = if (n > 0) { 1 } else { 2.5 }
print(r)
";
    let (stdout, exit) = build_and_run("if-int-float", src);
    assert_eq!(exit, 0);
    // Int en la rama then se coerciona a Float; la salida sigue la
    // convención del intérprete (`1.0`, no `1`).
    assert_lines(&stdout, &["1.0"]);
}

// ---------------------------------------------------------------------------
// 5b.2+: métodos built-in sobre Str
// ---------------------------------------------------------------------------

#[test]
fn str_len_chars_unicode_compilado() {
    let src = "\
let s = \"Chaltén\"
print(s.len())
";
    let (stdout, exit) = build_and_run("str-len", src);
    assert_eq!(exit, 0);
    // 7 caracteres Unicode, no 8 bytes UTF-8.
    assert_lines(&stdout, &["7"]);
}

#[test]
fn str_upper_lower_round_trip_compilado() {
    let src = "\
let s = \"Hola Mundo\"
print(s.upper())
print(s.lower())
print(s.upper().lower())
";
    let (stdout, exit) = build_and_run("str-upper-lower", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["HOLA MUNDO", "hola mundo", "hola mundo"]);
}

// ---------------------------------------------------------------------------
// Failure modes
// ---------------------------------------------------------------------------

#[test]
fn build_aborta_con_errores_de_tipo_strict() {
    let stderr = build_expect_fail(
        "strict-type-error",
        "let x: Int = \"no soy int\"\nprint(x)\n",
    );
    assert!(
        stderr.contains("error(es) de tipo"),
        "esperaba mensaje de error de tipo, fue: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Fase 5b.3 — listas, mapas, indexing, métodos built-in
// ---------------------------------------------------------------------------

#[test]
fn lista_basica_push_len_iteracion_compilado() {
    let src = "\
let xs: List<Int> = [1, 2, 3]
print(xs)
print(xs.len())
xs.push(4)
print(xs)
print(len(xs))
for v in xs {
    print(v)
}
";
    let (stdout, exit) = build_and_run("list-basic", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "[1, 2, 3]",
            "3",
            "[1, 2, 3, 4]",
            "4",
            "1",
            "2",
            "3",
            "4",
        ],
    );
}

#[test]
fn lista_indexing_y_pop_compilado() {
    let src = "\
let xs: List<Int> = [10, 20, 30]
print(xs[0])
print(xs[2])
let last = xs.pop()
print(last)
print(xs)
";
    let (stdout, exit) = build_and_run("list-index-pop", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["10", "30", "30", "[10, 20]"]);
}

#[test]
fn mapa_basico_has_keys_values_len_compilado() {
    let src = "\
let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}
print(m)
print(m.len())
print(m[\"a\"])
print(m.has(\"a\"))
print(m.has(\"z\"))
print(m.keys())
print(m.values())
";
    let (stdout, exit) = build_and_run("map-basic", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "{\"a\": 1, \"b\": 2, \"c\": 3}",
            "3",
            "1",
            "true",
            "false",
            "[\"a\", \"b\", \"c\"]",
            "[1, 2, 3]",
        ],
    );
}

#[test]
fn lista_de_instancias_con_map_filter_y_alias_compilado() {
    // Reproduce un fragmento del cap 13 (sin `find`, que llega en 5b.4).
    // Cubre: lista de Nominal, push, map con FnExpr→Str, filter con
    // FnExpr→Bool y método encadenado `.lower()`, mutación via alias
    // `xs[i].name = ...`.
    let src = "\
type User { id: Int, name: Str }
let usuarios: List<User> = [
    User { id: 1, name: \"Fitz\" },
    User { id: 2, name: \"Roy\" },
]
usuarios.push(User { id: 3, name: \"Cerro\" })
print(usuarios.len())
let nombres = usuarios.map(fn(u) => u.name)
print(nombres)
let solo_roy = usuarios.filter(fn(u) => u.name.lower() == \"roy\")
print(solo_roy)
let primer = usuarios[0]
primer.name = \"Patagonia\"
print(usuarios)
";
    let (stdout, exit) = build_and_run("list-instances", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "3",
            "[\"Fitz\", \"Roy\", \"Cerro\"]",
            "[User { id: 2, name: \"Roy\" }]",
            "[User { id: 1, name: \"Patagonia\" }, User { id: 2, name: \"Roy\" }, User { id: 3, name: \"Cerro\" }]",
        ],
    );
}

#[test]
fn chain_de_metodos_funciona_compilado() {
    // `.map(...).map(...)` y `.filter(...).map(...)` — los métodos de
    // List devuelven Rc<RefCell<Vec<_>>>, así que se pueden encadenar
    // como cualquier expresión. Ojo: cada método toma el receptor por
    // valor (clone del Rc) y devuelve una colección nueva.
    let src = "\
let xs: List<Int> = [1, 2, 3, 4]
let resultado = xs.filter(fn(x) => x > 1).map(fn(x) => x * 10)
print(resultado)
";
    let (stdout, exit) = build_and_run("chain-methods", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["[20, 30, 40]"]);
}

#[test]
fn lista_de_floats_con_int_promueve_a_float_compilado() {
    // El lub de items hace que `[1, 2.5, 3]` sea `List<Float>` y los
    // Int se inserten como `(N as f64)`.
    let src = "\
let xs = [1, 2.5, 3]
print(xs)
";
    let (stdout, exit) = build_and_run("list-promote-float", src);
    assert_eq!(exit, 0);
    // Float `1.0`, `2.5`, `3.0` con el formato del intérprete.
    assert_lines(&stdout, &["[1.0, 2.5, 3.0]"]);
}

#[test]
fn lista_heterogenea_aborta_build() {
    let stderr = build_expect_fail(
        "unsupported-heterogeneous-list",
        "let xs = [1, \"dos\"]\nprint(xs)\n",
    );
    assert!(
        stderr.contains("homogénea") || stderr.contains("incompatibles"),
        "esperaba mensaje sobre lista homogénea, fue: {}",
        stderr
    );
}

#[test]
fn async_fn_con_sleep_compilable_y_correcta() {
    // Fase 6.6: async fn Fitz con `sleep(...).await` adentro compila
    // a binario nativo y corre con tokio runtime current_thread.
    // El programa NO usa `.await` top-level (el checker lo prohíbe);
    // toda la maquinaria async está adentro de fns nombradas. La
    // `fn main` implícita (generada por el codegen sobre los stmts
    // top-level) es `#[tokio::main(flavor = "current_thread")]` por
    // detección automática.
    let src = "\
        async fn double(n: Int) -> Int {\n\
            let _ = sleep(0).await\n\
            return n * 2\n\
        }\n\
        async fn run() -> Int {\n\
            return double(21).await\n\
        }\n\
        print(\"async ok\")\n\
    ";
    let (stdout, exit) = build_and_run("async_sleep_compilable", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert_eq!(stdout.trim(), "async ok");
}

// Fase 8.7.1 — el viejo test `build_aborta_sobre_from_python_import`
// (8.1.5) quedó obsoleto: `fitz build` ahora acepta `from python
// import` y emite código Rust con pyo3 linkeado. El test nuevo
// (`build_python_import_math_extrae_pi`) vive bajo
// `#[cfg(feature = "python")]` más abajo, validando el caso real con
// ejecución del binario.

#[test]
fn build_aborta_si_codegen_no_soporta_feature() {
    // 5b.6 abrió @get/@post/etc., F11 abrió state HTTP compartido.
    // La feature que apuntamos acá pasa a ser **decorator HTTP custom
    // sobre `fn main`** — el codegen lo rechaza con mensaje claro
    // (regla R1 que pide handlers con nombre distinto a `main`).
    let stderr = build_expect_fail(
        "unsupported-http-main-decorator",
        "@get(\"/\") fn main() => 0\n",
    );
    assert!(
        stderr.contains("`fn main` solo admite `@server"),
        "esperaba mensaje sobre fn main + decorator HTTP, fue: {}",
        stderr
    );
}

// MW.3 — E2E: middleware user-fn + cors compilados + server + request real.
// Cada test usa puertos únicos (rango 43370-43399) para que las corridas
// no choquen entre tests serializados.

#[test]
fn http_mw3_middleware_passthrough_responde_200() {
    // Middleware que no corta — request llega al handler.
    let src = "\
fn logger(req: Request) {}

@server(43370)
fn main() => 0

@middleware(logger)
@get(\"/x\")
fn h() -> Str => \"ok\"
";
    let (status, body) = build_spawn_request(
        "mw3-passthrough",
        src,
        43370,
        "GET",
        "/x",
        None,
    );
    assert_eq!(status, 200);
    assert!(body.contains("ok"));
}

#[test]
fn http_mw3_middleware_short_circuita_con_401() {
    // Middleware corta con 401 — handler NO se invoca.
    let src = "\
fn auth(req: Request) {
    return 401 {\"error\": \"sin autorizacion\"}
}

@server(43371)
fn main() => 0

@middleware(auth)
@get(\"/protected\")
fn h() -> Str => \"NO DEBERIA APARECER\"
";
    let (status, body) = build_spawn_request(
        "mw3-shortcircuit",
        src,
        43371,
        "GET",
        "/protected",
        None,
    );
    assert_eq!(status, 401);
    assert!(body.contains("sin autorizacion"), "body fue: {}", body);
    assert!(!body.contains("NO DEBERIA APARECER"));
}

#[test]
fn http_mw3_cors_preflight_options_devuelve_204_con_headers() {
    let src = "\
@server(43372)
fn main() => 0

@middleware(cors({\"allow_origin\": \"https://app.x.com\", \"max_age\": 600}))
@get(\"/api\")
fn list_items() -> Str => \"[]\"
";
    let (status, raw_headers) = build_spawn_request_raw(
        "mw3-preflight",
        src,
        43372,
        "OPTIONS",
        "/api",
    );
    assert_eq!(status, 204);
    let headers_lower = raw_headers.to_lowercase();
    assert!(
        headers_lower.contains("access-control-allow-origin: https://app.x.com"),
        "headers preflight no llevan allow-origin custom: {}",
        raw_headers
    );
    assert!(
        headers_lower.contains("access-control-allow-methods"),
        "headers preflight no llevan allow-methods: {}",
        raw_headers
    );
    assert!(
        headers_lower.contains("access-control-max-age: 600"),
        "headers preflight no llevan max-age: {}",
        raw_headers
    );
}

#[test]
fn http_q3_cors_set_echo_origin_en_response_real() {
    // Q.3: cors({"allow_origin": [...]}) build-time. Una request con
    // Origin en la lista permitida → echo del origin en la response.
    let src = "\
@server(43380)
fn main() => 0

@middleware(cors({\"allow_origin\": [\"https://a.com\", \"https://b.com\"]}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "q3-cors-set-match",
        src,
        43380,
        "GET",
        "/api",
        &[("Origin", "https://b.com")],
    );
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        lower.contains("access-control-allow-origin: https://b.com"),
        "esperaba echo del Origin permitido, fue: {}",
        raw_headers
    );
}

#[test]
fn http_q3_cors_set_omite_origin_si_request_no_matchea() {
    let src = "\
@server(43381)
fn main() => 0

@middleware(cors({\"allow_origin\": [\"https://a.com\"]}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "q3-cors-set-miss",
        src,
        43381,
        "GET",
        "/api",
        &[("Origin", "https://evil.com")],
    );
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        !lower.contains("access-control-allow-origin"),
        "el header allow-origin NO debe emitirse con origin no permitido: {}",
        raw_headers
    );
    // El resto de headers CORS sí.
    assert!(
        lower.contains("access-control-allow-methods"),
        "esperaba allow-methods igual: {}",
        raw_headers
    );
}

#[test]
fn http_q3_cors_set_preflight_echo_y_miss() {
    let src = "\
@server(43382)
fn main() => 0

@middleware(cors({\"allow_origin\": [\"https://a.com\"]}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    // Preflight con Origin permitido → 204 + echo.
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "q3-cors-preflight-match",
        src,
        43382,
        "OPTIONS",
        "/api",
        &[("Origin", "https://a.com")],
    );
    assert_eq!(status, 204);
    let lower = raw_headers.to_lowercase();
    assert!(
        lower.contains("access-control-allow-origin: https://a.com"),
        "preflight: esperaba echo, fue: {}",
        raw_headers
    );
}

/// Variante de `build_spawn_request_raw` que envía headers HTTP
/// custom (`Origin: ...`). Usado por los tests Q.3 de CORS.
fn build_spawn_request_raw_with_headers(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        output.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) { "prog.exe" } else { "prog" };
    let bin = dir.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server no abrió el puerto {} en 3s", port);
    }

    use std::io::{Read, Write};
    let mut extra = String::new();
    for (k, v) in headers {
        extra.push_str(&format!("{}: {}\r\n", k, v));
    }
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
        method, path, addr, extra
    );
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let _ = child.kill();
    let _ = child.wait();

    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers_end = raw.find("\r\n\r\n").unwrap_or(raw.len());
    let headers_section = raw[..headers_end].to_string();
    (status, headers_section)
}

#[test]
fn http_mw3_cors_response_real_lleva_headers_inyectados() {
    let src = "\
@server(43373)
fn main() => 0

@middleware(cors())
@get(\"/api\")
fn list_items() -> Str => \"ok\"
";
    let (status, raw_headers) = build_spawn_request_raw(
        "mw3-cors-real",
        src,
        43373,
        "GET",
        "/api",
    );
    assert_eq!(status, 200);
    let headers_lower = raw_headers.to_lowercase();
    assert!(
        headers_lower.contains("access-control-allow-origin: *"),
        "headers de la response real no llevan allow-origin default: {}",
        raw_headers
    );
}

/// Como `build_spawn_request` pero devuelve la sección entera de headers
/// crudos (todo lo que va entre la status line y el `\r\n\r\n`). Usado
/// por los tests CORS de MW.3 para verificar `Access-Control-Allow-*`.
fn build_spawn_request_raw(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
) -> (u16, String) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        output.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) { "prog.exe" } else { "prog" };
    let bin = dir.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server no abrió el puerto {} en 3s", port);
    }

    use std::io::{Read, Write};
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        method, path, addr
    );
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let _ = child.kill();
    let _ = child.wait();

    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers_end = raw.find("\r\n\r\n").unwrap_or(raw.len());
    let headers_section = raw[..headers_end].to_string();
    (status, headers_section)
}

// ---------------------------------------------------------------------------
// Fase 5b.4 — Result, `?`, match
// ---------------------------------------------------------------------------

#[test]
fn result_ok_err_match_completo_compilado() {
    // Cap 14 entero adaptado: `divide` retorna Result; consumimos los
    // dos resultados con `match` Ok/Err. La salida debe matchear
    // bit-a-bit lo que produce el intérprete.
    let src = "\
fn divide(a: Int, b: Int) -> Result<Int> {
    if (b == 0) {
        return Err(\"división por cero\")
    }
    return Ok(a / b)
}

match divide(10, 2) {
    Ok(v) => print(\"ok: {v}\")
    Err(e) => print(\"err: {e}\")
}

match divide(10, 0) {
    Ok(v) => print(\"ok: {v}\")
    Err(e) => print(\"err: {e}\")
}
";
    let (stdout, exit) = build_and_run("result-divide-match", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["ok: 5", "err: división por cero"]);
}

#[test]
fn try_operator_propaga_err_compilado() {
    // `?` adentro de fn `Result<T>` propaga el Err. Replicamos el
    // segundo bloque del cap 14: find_user / describe_user.
    let src = "\
type User { id: Int, name: Str }

fn find_user(id: Int) -> Result<User> {
    if (id == 1) {
        return Ok(User { id: 1, name: \"Fitz\" })
    }
    return Err(\"usuario no encontrado\")
}

fn describe_user(id: Int) -> Result<Str> {
    let u = find_user(id)?
    return Ok(\"#{u.id} es {u.name}\")
}

match describe_user(1) {
    Ok(desc) => print(desc)
    Err(e) => print(\"falló: {e}\")
}

match describe_user(42) {
    Ok(desc) => print(desc)
    Err(e) => print(\"falló: {e}\")
}
";
    let (stdout, exit) = build_and_run("try-propagation", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["#1 es Fitz", "falló: usuario no encontrado"],
    );
}

#[test]
fn list_find_devuelve_result_y_se_consume_con_match_compilado() {
    // Cap 13 con find: `.find` devuelve Ok(item) o Err. Match Ok/Err.
    let src = "\
type User { id: Int, name: Str }
let usuarios: List<User> = [
    User { id: 1, name: \"Fitz\" },
    User { id: 2, name: \"Roy\" },
]
let primero = usuarios.find(fn(u) => u.id == 1)
match primero {
    Ok(u)  => print(\"hola, {u.name}!\")
    Err(e) => print(\"no debería pasar: {e}\")
}
let nadie = usuarios.find(fn(u) => u.id == 99)
match nadie {
    Ok(u)  => print(\"insólito: {u.name}\")
    Err(e) => print(\"falta: {e}\")
}
";
    let (stdout, exit) = build_and_run("list-find-match", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["hola, Fitz!", "falta: no encontrado"],
    );
}

#[test]
fn map_get_devuelve_result_con_mensaje_compilado() {
    // `m.get(k)` con clave faltante: Err con mensaje "clave no encontrada: <k>"
    // — formato idéntico al intérprete. Importante: la clave se formatea
    // con Display (Value), no inline — Str va SIN comillas en el mensaje.
    let src = "\
let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}
match m.get(\"a\") {
    Ok(v)  => print(\"a vale {v}\")
    Err(e) => print(\"err: {e}\")
}
match m.get(\"z\") {
    Ok(v)  => print(\"insólito {v}\")
    Err(e) => print(\"err: {e}\")
}
";
    let (stdout, exit) = build_and_run("map-get-match", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["a vale 1", "err: clave no encontrada: z"],
    );
}

#[test]
fn print_de_result_compilado_matchea_interprete() {
    // `print(Ok(v))` y `print(Err(e))` deben producir `Ok(42)` y
    // `Err("texto")` (Err con comillas dobles) bit-a-bit.
    let src = "\
fn ok42() -> Result<Int> { return Ok(42) }
fn boom() -> Result<Int> { return Err(\"explotó\") }
print(ok42())
print(boom())
";
    let (stdout, exit) = build_and_run("print-result", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["Ok(42)", "Err(\"explotó\")"]);
}

// ---------------------------------------------------------------------------
// Fase 5b.5 — módulos / import
// ---------------------------------------------------------------------------
//
// Los tests de 5b.5 necesitan MÚLTIPLES archivos en el tempdir (no solo
// `prog.fitz`). Helper específico para escribir varios archivos antes
// del build.

fn build_and_run_multi(
    test_name: &str,
    main_src: &str,
    extra_files: &[(&str, &str)],
) -> (String, i32) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, main_src).expect("escribir .fitz");
    for (name, content) in extra_files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("crear subdir");
        }
        std::fs::write(&p, content).expect("escribir extra .fitz");
    }
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        output.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) { "prog.exe" } else { "prog" };
    let bin = dir.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
    let run = Command::new(&bin).output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

fn build_expect_fail_multi(
    test_name: &str,
    main_src: &str,
    extra_files: &[(&str, &str)],
) -> String {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, main_src).expect("escribir .fitz");
    for (name, content) in extra_files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("crear subdir");
        }
        std::fs::write(&p, content).expect("escribir extra .fitz");
    }
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        !output.status.success(),
        "esperaba que fitz build fallara, pero salió OK:\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn from_import_type_y_fn_compilado() {
    // Reproduce el patrón de `examples/guide/16-modulos.fitz`:
    // - `import utils` expone `utils.greet(...)` como namespace.
    // - `from utils import User` trae el tipo al scope para
    //   construir con `User { ... }`.
    let main = "\
import utils
from utils import User
let u = User { id: 7, name: \"Fitz\" }
print(utils.greet(u.name))
print(u)
";
    let utils = "\
let PREFIX = \"saludos, \"
fn greet(name: Str) -> Str => \"{PREFIX}{name}\"
type User { id: Int, name: Str }
";
    let (stdout, exit) = build_and_run_multi(
        "module-basic",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["saludos, Fitz", "User { id: 7, name: \"Fitz\" }"],
    );
}

#[test]
fn from_import_const_str_compilado() {
    // `from utils import PREFIX` trae una constante de Str al scope.
    let main = "\
from utils import PREFIX
print(PREFIX)
";
    let utils = "let PREFIX = \"prefijo\"";
    let (stdout, exit) = build_and_run_multi(
        "module-import-const",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["prefijo"]);
}

#[test]
fn import_namespace_con_fn_solo_compilado() {
    // `import utils` y luego `utils.greet(...)` sin importar nada
    // específico. El namespace queda disponible vía path Rust.
    let main = "\
import utils
print(utils.greet(\"Patagonia\"))
";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"";
    let (stdout, exit) = build_and_run_multi(
        "module-namespace-only",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Patagonia"]);
}

#[test]
fn from_import_type_con_default_referencia_const_del_modulo() {
    // PreF8.3: el `type User` del módulo tiene un default
    // `id: Int = MAX` donde `MAX` es una const del propio módulo.
    // El importer NO importa `MAX`, solo `User`. Pre-PreF8.3 el
    // codegen del struct lit fallaba "variable desconocida en
    // codegen: MAX" porque resolvía el default_expr en el ctx del
    // importer. Post-fix, el módulo emite `pub fn __default_User_id()
    // -> i64 { MAX }` y el importer llama a `utils::__default_User_id()`.
    let main = "\
from utils import User
let u = User {}
print(u.id)
print(u.name)
";
    let utils = "\
let MAX = 99
let HELLO = \"saludos\"
type User { id: Int = MAX, name: Str = HELLO }
";
    let (stdout, exit) = build_and_run_multi(
        "module-default-with-const",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["99", "saludos"]);
}

#[test]
fn import_namespace_con_alias_compila() {
    // PreF8.4: `import utils as u` y luego `u.greet(...)`. El alias
    // queda como binding local; el módulo se carga normalmente.
    let main = "\
import utils as u
print(u.greet(\"Fitz\"))
";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"";
    let (stdout, exit) = build_and_run_multi(
        "module-import-alias-ns",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz"]);
}

#[test]
fn from_import_con_alias_simple_compila() {
    // PreF8.4: `from utils import greet as g` y `g(...)`.
    let main = "\
from utils import greet as g
print(g(\"Fitz\"))
";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"";
    let (stdout, exit) = build_and_run_multi(
        "module-import-alias-fn",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz"]);
}

#[test]
fn from_import_alias_de_tipo_y_struct_lit() {
    // PreF8.4: `from utils import User as Person`. El struct lit
    // `Person { ... }` instancia el tipo del módulo. El Display
    // muestra el nombre original `User` (paridad con fitz run, que
    // usa el name canónico del Value::Type del módulo).
    let main = "\
from utils import User as Person
let p = Person { id: 7, name: \"Fitz\" }
print(p)
";
    let utils = "type User { id: Int, name: Str }";
    let (stdout, exit) = build_and_run_multi(
        "module-import-alias-type",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["User { id: 7, name: \"Fitz\" }"]);
}

#[test]
fn from_import_alias_de_const_no_choca_con_let_local() {
    // PreF8.4 — caso para el que el alias es útil: el importer
    // tiene una `let PREFIX = "local"` y necesita la `PREFIX` del
    // módulo bajo otro nombre. Sin alias chocarían en el codegen
    // (el `use utils::PREFIX` colisionaría con el `static PREFIX`
    // del importer). Con alias funciona.
    let main = "\
from utils import PREFIX as REMOTE
let PREFIX = \"local\"
print(PREFIX)
print(REMOTE)
";
    let utils = "let PREFIX = \"remoto\"";
    let (stdout, exit) = build_and_run_multi(
        "module-import-alias-const",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["local", "remoto"]);
}

#[test]
fn modulo_inexistente_aborta_build() {
    let stderr = build_expect_fail_multi(
        "module-not-found",
        "import inexistente\nprint(0)\n",
        &[],
    );
    assert!(
        stderr.contains("no se encontró el módulo")
            || stderr.contains("inexistente"),
        "esperaba mensaje de módulo no encontrado, fue: {}",
        stderr
    );
}

#[test]
fn modulo_con_import_propio_es_error_transitivo() {
    // 5b.5: los imports transitivos no se soportan todavía.
    // Si el módulo cargado tiene su propio `import`, el loader aborta.
    let main = "\
import primero
print(primero.x())
";
    let primero = "\
import segundo
fn x() -> Int => 1
";
    let segundo = "fn y() -> Int => 2";
    let stderr = build_expect_fail_multi(
        "module-transitivo",
        main,
        &[("primero.fitz", primero), ("segundo.fitz", segundo)],
    );
    assert!(
        stderr.contains("transitivos") || stderr.contains("5b.5"),
        "esperaba mensaje sobre imports transitivos / 5b.5, fue: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Fase 5b.6 — HTTP / @server / handlers
// ---------------------------------------------------------------------------

/// Helper: build de un programa HTTP, spawn del binario, request HTTP
/// crudo (sin reqwest para evitar dep extra en tests), y stop. Devuelve
/// (status_line, body) leídos del socket.
fn build_spawn_request(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        output.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) { "prog.exe" } else { "prog" };
    let bin = dir.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    // Spawn del server en background. Le damos tiempo a abrir el
    // puerto antes de la primera request.
    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Esperar a que el puerto esté escuchando (hasta 3s).
    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server no abrió el puerto {} en 3s", port);
    }

    // Construir la request HTTP a mano (sin reqwest).
    use std::io::{Read, Write};
    let request = match body {
        Some(b) => format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            method,
            path,
            addr,
            b.len(),
            b
        ),
        None => format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            method, path, addr
        ),
    };
    let mut stream =
        std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let _ = child.kill();
    let _ = child.wait();

    // Parsear status + body. Formato: "HTTP/1.1 <code> <reason>\r\n...\r\n\r\n<body>"
    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = raw
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(raw.len());
    let body = raw[body_start..].to_string();
    (status, body)
}

#[test]
fn http_get_simple_responde_200_y_body() {
    // El criterio mínimo de 5b.6: un handler GET que devuelve un Str
    // produce 200 + JSON con el string.
    let src = "@server(43210)\nfn main() => 0\n\
               @get(\"/\") fn index() -> Str => \"Fitz HTTP corriendo\"\n";
    let (status, body) = build_spawn_request(
        "http-get-simple",
        src,
        43210,
        "GET",
        "/",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"Fitz HTTP corriendo\"");
}

#[test]
fn http_get_con_path_param_int() {
    let src = "@server(43211)\nfn main() => 0\n\
               @get(\"/double/{n}\") fn double(n: Int) -> Int => n * 2\n";
    let (status, body) =
        build_spawn_request("http-path-int", src, 43211, "GET", "/double/21", None);
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "42");
}

#[test]
fn http_async_handler_con_sleep_responde_200() {
    // Fase 6.6: handler `async fn` con `sleep(...).await` adentro.
    // El wrapper de axum await-ea el future del handler antes de
    // serializar el resultado. End-to-end con tokio current_thread.
    let src = "@server(43219)\nfn main() => 0\n\
               @get(\"/pause\") async fn pause() -> Str {\n\
                   let _ = sleep(0).await\n\
                   return \"done\"\n\
               }\n";
    let (status, body) =
        build_spawn_request("http-async-handler", src, 43219, "GET", "/pause", None);
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"done\"");
}

#[test]
fn http_result_ok_responde_200_err_responde_500() {
    let src = "@server(43212)\nfn main() => 0\n\
               @get(\"/d/{a}/{b}\") fn divide(a: Int, b: Int) -> Result<Int> {\n\
                   if (b == 0) { return Err(\"div por cero\") }\n\
                   return Ok(a / b)\n\
               }\n";
    let (status_ok, body_ok) =
        build_spawn_request("http-result-ok", src, 43212, "GET", "/d/10/2", None);
    assert_eq!(status_ok, 200);
    assert_eq!(body_ok.trim(), "5");
    let (status_err, body_err) = build_spawn_request(
        "http-result-err",
        src,
        43212,
        "GET",
        "/d/10/0",
        None,
    );
    assert_eq!(status_err, 500);
    assert!(
        body_err.contains("\"error\":\"div por cero\""),
        "esperaba error JSON con mensaje, fue: {}",
        body_err
    );
}

#[test]
fn http_post_body_deserializa_tipo_custom() {
    let src = "@server(43213)\nfn main() => 0\n\
               type Input { msg: Str, times: Int = 1 }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request(
        "http-post-body",
        src,
        43213,
        "POST",
        "/echo",
        Some("{\"msg\":\"hola\",\"times\":3}"),
    );
    assert_eq!(status, 200);
    assert!(
        body.contains("\"msg\":\"hola\"") && body.contains("\"times\":3"),
        "esperaba body con msg y times, fue: {}",
        body
    );
}

#[test]
fn http_post_body_aplica_defaults_a_campos_faltantes() {
    let src = "@server(43214)\nfn main() => 0\n\
               type Input { msg: Str, times: Int = 7 }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request(
        "http-post-default",
        src,
        43214,
        "POST",
        "/echo",
        Some("{\"msg\":\"sin times\"}"),
    );
    assert_eq!(status, 200);
    assert!(
        body.contains("\"times\":7"),
        "esperaba default `times: 7` aplicado, fue: {}",
        body
    );
}

#[test]
fn http_post_body_extra_field_es_400() {
    let src = "@server(43215)\nfn main() => 0\n\
               type Input { msg: Str }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request(
        "http-post-extra",
        src,
        43215,
        "POST",
        "/echo",
        Some("{\"msg\":\"x\",\"extra\":\"nope\"}"),
    );
    assert_eq!(status, 400);
    assert!(
        body.contains("campo no declarado"),
        "esperaba mensaje sobre campo no declarado, fue: {}",
        body
    );
}

// ---------------------------------------------------------------------------
// F12 — higher-order completo (closures, fn como valor/param/retorno)
// ---------------------------------------------------------------------------

#[test]
fn fn_anonima_asignada_a_var_se_invoca() {
    let src = "\
let f: Fn(Int) -> Int = fn(n: Int) => n * 2
print(f(21))
";
    let (stdout, exit) = build_and_run("f12-fnexpr-var", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["42"]);
}

#[test]
fn fn_nombrada_como_valor_se_invoca() {
    let src = "\
fn square(n: Int) -> Int => n * n
let g: Fn(Int) -> Int = square
print(g(7))
";
    let (stdout, exit) = build_and_run("f12-fn-nombrada-valor", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["49"]);
}

#[test]
fn apply_con_fn_y_fnexpr_inline() {
    let src = "\
fn square(n: Int) -> Int => n * n
fn apply(f: Fn(Int) -> Int, x: Int) -> Int => f(x)
print(apply(square, 7))
print(apply(fn(n: Int) => n * 10, 7))
";
    let (stdout, exit) = build_and_run("f12-apply", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["49", "70"]);
}

#[test]
fn closure_con_captura_int_funciona() {
    // make_adder(x) retorna una closure que captura x por valor.
    let src = "\
fn make_adder(x: Int) -> Fn(Int) -> Int {
    return fn(y: Int) => x + y
}
let add5: Fn(Int) -> Int = make_adder(5)
print(add5(3))
print(add5(10))
";
    let (stdout, exit) = build_and_run("f12-make-adder", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["8", "15"]);
}

#[test]
fn closure_que_captura_str_clona_afuera() {
    // El codegen debe clonar `saludo` antes del closure para que la
    // var siga disponible en el caller después de pasarla a la
    // closure (move la consumiría sin el clone).
    let src = "\
let saludo = \"hola\"
let f: Fn(Str) -> Str = fn(n: Str) => \"{saludo}, {n}!\"
print(f(\"Fitz\"))
print(saludo)
";
    let (stdout, exit) = build_and_run("f12-capture-str", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz!", "hola"]);
}

#[test]
fn fnexpr_sin_anotacion_de_param_aborta_build() {
    // Param sin anotar → error claro (deuda 5b.1).
    let stderr = build_expect_fail(
        "f12-fnexpr-sin-anot",
        "let f: Fn(Int) -> Int = fn(x) => x * 2\n",
    );
    assert!(
        stderr.contains("anónima") && stderr.contains("anotación"),
        "esperaba mensaje sobre fn anónima sin anotación, fue: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Fase 5b.7 — smoke test: todos los ejemplos guía marcados como
// compilables deben compilar con `fitz build` sin error
// ---------------------------------------------------------------------------

/// Lista de ejemplos guía que el cap 18 declara compilables con
/// `fitz build`. Los que NO están acá tienen una razón documentada
/// (state HTTP compartido, lista heterogénea, higher-order completo,
/// error intencional, etc.) — están listados en el cap 18 también.
const GUIDE_EXAMPLES_COMPILE: &[&str] = &[
    "02-hola.fitz",
    "03-variables.fitz",
    "03b-numeros-legibles.fitz",
    "03c-bases-numericas.fitz",
    "04-operadores.fitz",
    "04b-operadores-bit.fitz",
    "04c-asignacion-compuesta-bit.fitz",
    "05-strings.fitz",
    "05b-format-specs.fitz",
    "06-logica.fitz",
    "07-if.fitz",
    "08-loops.fitz",
    "08b-loops-avanzados.fitz",
    "09b-indexing-slicing.fitz",
    "09c-tuples.fitz",
    "09d-comprehensions.fitz",
    "09e-for-map.fitz",
    "10-match.fitz",
    "10b-match-tuple-subpatterns.fitz",
    "11-funciones.fitz",
    "12-type.fitz",
    "13-metodos.fitz",
    "13b-metodos-custom.fitz",
    "13c-metodos-extras.fitz",
    "13d-iteradores.fitz",
    "14-result.fitz",
    "14b-errores-tipados.fitz",
    "14c-result-tipado.fitz",
    "16-modulos.fitz",
    "17-http.fitz",
    "17b-middleware.fitz",
    "18-docs.fitz",
    "19-async.fitz",
    "19b-paralelismo.fitz",
    "20-build.fitz",
    "23-fmt-ejemplo.fitz",
    "24-tests.fitz",
];

#[test]
fn smoke_ejemplos_guia_compilables_compilan() {
    // Smoke test del cap 18: cada ejemplo de la lista compila a binario
    // con `fitz build`. Costoso (cada ejemplo invoca cargo + rustc),
    // pero corre serializado por el `SERIAL` mutex y vale para
    // prevenir regresiones futuras del codegen sobre la guía.
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let project_root = std::env::current_dir().expect("cwd");
    let guide_dir = project_root.join("examples").join("guide");

    let mut failures: Vec<String> = Vec::new();
    for name in GUIDE_EXAMPLES_COMPILE {
        let src_path = guide_dir.join(name);
        assert!(
            src_path.exists(),
            "ejemplo no existe: {}",
            src_path.display()
        );
        let output = Command::new(fitz_bin())
            .args(["build"])
            .arg(&src_path)
            .output()
            .expect("invocar fitz build");
        if !output.status.success() {
            failures.push(format!(
                "{}\n--- stderr ---\n{}",
                name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        // Limpiar el binario adyacente para no dejar `.exe` colgados
        // (memoria del workflow: examples/ no debe contener generados).
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap();
        let bin_name = if cfg!(windows) {
            format!("{}.exe", stem)
        } else {
            stem.to_string()
        };
        let _ = std::fs::remove_file(guide_dir.join(&bin_name));
        let _ = std::fs::remove_file(guide_dir.join(format!("{}.pdb", stem)));
    }

    assert!(
        failures.is_empty(),
        "ejemplos que no compilaron ({}):\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}

// ---------------------------------------------------------------------------
// F11 — state HTTP compartido (thread_local + tokio current_thread)
// ---------------------------------------------------------------------------

/// Helper F11: build + spawn + **secuencia** de requests sobre el mismo
/// binario corriendo. Valida que el state compartido persiste entre
/// llamadas. La secuencia es `(method, path, body) -> (status, body)`.
fn build_spawn_requests(
    test_name: &str,
    src: &str,
    port: u16,
    sequence: &[(&str, &str, Option<&str>)],
) -> Vec<(u16, String)> {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", test_name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build");
    assert!(
        output.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) { "prog.exe" } else { "prog" };
    let bin = dir.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server no abrió el puerto {} en 3s", port);
    }

    use std::io::{Read, Write};
    let mut results: Vec<(u16, String)> = Vec::with_capacity(sequence.len());
    for (method, path, body) in sequence {
        let request = match body {
            Some(b) => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                method,
                path,
                addr,
                b.len(),
                b
            ),
            None => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                method, path, addr
            ),
        };
        let mut stream =
            std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();

        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw
            .find("\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        results.push((status, body));
    }

    let _ = child.kill();
    let _ = child.wait();
    results
}

#[test]
fn http_state_get_lista_compartida() {
    // F11: una `let users = [...]` top-level referenciada por un
    // handler GET. El binario debe servir la lista y los items que
    // contiene preservan el formato JSON del intérprete.
    let src = "@server(43320)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }, User { id: 2, name: \"luis\" }]\n\
               @get(\"/users\") fn list_users() -> List<User> => users\n";
    let results = build_spawn_requests(
        "http-state-get-list",
        src,
        43320,
        &[("GET", "/users", None)],
    );
    let (status, body) = &results[0];
    assert_eq!(*status, 200);
    assert!(
        body.contains("\"id\":1") && body.contains("\"name\":\"ana\"")
            && body.contains("\"id\":2") && body.contains("\"name\":\"luis\""),
        "esperaba lista con ambos users, body fue: {}",
        body
    );
}

#[test]
fn http_state_post_persiste_entre_requests() {
    // F11: POST agrega a la lista. Un GET posterior **al mismo binario**
    // debe ver el nuevo item — confirmación clave de que el state
    // compartido funciona via `thread_local!` + tokio current_thread.
    let src = "@server(43321)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               type UserInput { name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }]\n\
               @get(\"/users\") fn list_users() -> List<User> => users\n\
               @post(\"/users\") fn create_user(body: UserInput) -> User {\n\
                   let u = User { id: users.len() + 1, name: body.name }\n\
                   users.push(u)\n\
                   return u\n\
               }\n";
    let results = build_spawn_requests(
        "http-state-post-persist",
        src,
        43321,
        &[
            ("GET", "/users", None),
            ("POST", "/users", Some(r#"{"name":"sofi"}"#)),
            ("GET", "/users", None),
        ],
    );
    assert_eq!(results[0].0, 200);
    assert!(
        !results[0].1.contains("sofi"),
        "primer GET no debería tener `sofi`, body: {}",
        results[0].1
    );
    assert_eq!(results[1].0, 200);
    assert!(
        results[1].1.contains("\"name\":\"sofi\""),
        "POST debería devolver el nuevo user, body: {}",
        results[1].1
    );
    assert_eq!(results[2].0, 200);
    assert!(
        results[2].1.contains("\"name\":\"sofi\"") && results[2].1.contains("\"name\":\"ana\""),
        "GET final debería ver ambos users (state persiste), body: {}",
        results[2].1
    );
}

#[test]
fn http_state_put_mutacion_de_campos() {
    // F11: PUT muta campos de un user existente. El siguiente GET
    // debe ver la mutación. Mismo binary, locks consecutivos sobre el
    // thread_local.
    let src = "@server(43322)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               type UserInput { name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }]\n\
               @get(\"/users/{id}\") fn get_user(id: Int) -> Result<User> {\n\
                   return users.find(fn(u) => u.id == id)\n\
               }\n\
               @put(\"/users/{id}\") fn update_user(id: Int, body: UserInput) -> Result<User> {\n\
                   let u = users.find(fn(u) => u.id == id)?\n\
                   u.name = body.name\n\
                   return Ok(u)\n\
               }\n";
    let results = build_spawn_requests(
        "http-state-put-mutate",
        src,
        43322,
        &[
            ("PUT", "/users/1", Some(r#"{"name":"ana actualizada"}"#)),
            ("GET", "/users/1", None),
        ],
    );
    assert_eq!(results[0].0, 200);
    assert!(
        results[0].1.contains("\"name\":\"ana actualizada\""),
        "PUT debería devolver el user mutado, body: {}",
        results[0].1
    );
    assert_eq!(results[1].0, 200);
    assert!(
        results[1].1.contains("\"name\":\"ana actualizada\""),
        "GET posterior debería ver la mutación, body: {}",
        results[1].1
    );
}

#[test]
fn http_state_delete_reconstruccion_lista() {
    // F11: DELETE que reconstruye la lista (filter + while pop + for
    // push) — patrón canónico para "borrar de una lista compartida sin
    // perder la referencia". El siguiente GET ve la lista achicada.
    let src = "@server(43323)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }, User { id: 2, name: \"luis\" }]\n\
               @get(\"/users\") fn list_users() -> List<User> => users\n\
               @delete(\"/users/{id}\") fn delete_user(id: Int) -> Result<Str> {\n\
                   let kept = users.filter(fn(u) => u.id != id)\n\
                   while (users.len() > 0) { users.pop() }\n\
                   for u in kept { users.push(u) }\n\
                   return Ok(\"borrado\")\n\
               }\n";
    let results = build_spawn_requests(
        "http-state-delete-rebuild",
        src,
        43323,
        &[
            ("DELETE", "/users/2", None),
            ("GET", "/users", None),
        ],
    );
    assert_eq!(results[0].0, 200);
    assert_eq!(results[1].0, 200);
    assert!(
        results[1].1.contains("\"name\":\"ana\"")
            && !results[1].1.contains("\"name\":\"luis\""),
        "GET post-delete debería tener solo ana, body: {}",
        results[1].1
    );
}

#[test]
fn http_state_var_no_referenciada_no_se_promueve() {
    // F11: una var top-level que NO es referenciada por ningún handler
    // no debe afectar el codegen — debe ejecutarse como expresión normal
    // dentro de `fn main()` y el server arranca igual.
    let src = "@server(43324)\nfn main() => 0\n\
               let saludo = \"ignorada\"\n\
               @get(\"/\") fn index() -> Str => \"ok\"\n";
    let results = build_spawn_requests(
        "http-state-unused",
        src,
        43324,
        &[("GET", "/", None)],
    );
    assert_eq!(results[0].0, 200);
    assert!(
        results[0].1.contains("ok"),
        "GET / debería devolver `ok`, body: {}",
        results[0].1
    );
}

#[test]
fn match_sobre_int_con_rango_compilado() {
    // Pattern `0..10` con guard, más wildcard catch-all.
    let src = "\
let n = 5
let s = match n {
    0..10 => \"chico\"
    _ => \"grande\"
}
print(s)
let m = 50
let t = match m {
    0..10 => \"chico\"
    _ => \"grande\"
}
print(t)
";
    let (stdout, exit) = build_and_run("match-range", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["chico", "grande"]);
}

// ---------------------------------------------------------------------------
// Status codes custom (return <int> { ... })
// ---------------------------------------------------------------------------

#[test]
fn http_status_codes_custom_401_y_body_json() {
    // Sintaxis del spec: `return 401 { ... }` adentro de un handler HTTP
    // emite status 401 con el body serializado como JSON. End-to-end con
    // curl-equivalente: status line + body matchean.
    let src = "@server(43400)\nfn main() => 0\n\
               @get(\"/protected\") fn protected() -> Str {\n\
                   return 401 {\"message\": \"no autorizado\"}\n\
               }\n";
    let (status, body) = build_spawn_request(
        "http-status-401",
        src,
        43400,
        "GET",
        "/protected",
        None,
    );
    assert_eq!(status, 401, "esperaba status 401");
    assert!(
        body.contains("\"message\":\"no autorizado\""),
        "body debería contener `message`, fue: {}",
        body
    );
}

#[test]
fn http_status_codes_polimorfico_mix_ok_y_404() {
    // Spec polimórfico: el handler retorna `-> Str` declarado pero
    // adentro mezcla `return "alice"` (Str → 200) con `return 404
    // {...}` (status custom). Cada uno produce la respuesta esperada.
    let src = "@server(43401)\nfn main() => 0\n\
               @get(\"/u/{id}\") fn get_user(id: Int) -> Str {\n\
                   if (id == 1) { return \"alice\" }\n\
                   return 404 {\"error\": \"no encontrado\"}\n\
               }\n";
    let (status_ok, body_ok) = build_spawn_request(
        "http-status-mix-ok",
        src,
        43401,
        "GET",
        "/u/1",
        None,
    );
    assert_eq!(status_ok, 200);
    assert_eq!(body_ok.trim(), "\"alice\"");

    let (status_404, body_404) = build_spawn_request(
        "http-status-mix-404",
        src,
        43401,
        "GET",
        "/u/2",
        None,
    );
    assert_eq!(status_404, 404);
    assert!(
        body_404.contains("\"error\":\"no encontrado\""),
        "body 404 debería contener `error`, fue: {}",
        body_404
    );
}

// ---------------------------------------------------------------------------
// Query params HTTP (?key={name})
// ---------------------------------------------------------------------------

#[test]
fn http_query_params_obligatorios_extraen_y_coercen() {
    // `?limit={limit}&offset={offset}` con ambos `Int` obligatorios.
    // El handler los recibe coercionados, los inscrusta en la response.
    let src = "@server(43500)\nfn main() => 0\n\
               @get(\"/items?limit={limit}&offset={offset}\")\n\
               fn list_items(limit: Int, offset: Int) -> Str {\n\
                   return \"limit={limit} offset={offset}\"\n\
               }\n";
    let (status, body) = build_spawn_request(
        "http-query-required",
        src,
        43500,
        "GET",
        "/items?limit=10&offset=20",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"limit=10 offset=20\"");
}

#[test]
fn http_query_param_obligatorio_faltante_es_400() {
    let src = "@server(43501)\nfn main() => 0\n\
               @get(\"/items?limit={limit}\")\n\
               fn list_items(limit: Int) -> Int => limit\n";
    let (status, body) = build_spawn_request(
        "http-query-missing-required",
        src,
        43501,
        "GET",
        "/items",
        None,
    );
    assert_eq!(status, 400);
    assert!(
        body.contains("query param 'limit'") && body.contains("obligatorio"),
        "body debería decir 'obligatorio', fue: {}",
        body
    );
}

#[test]
fn http_query_param_nullable_falta_devuelve_null() {
    // `name: Str?` → si falta en la query, el handler ve `Null`. El
    // print/interpolación lo serializa como `null`.
    let src = "@server(43502)\nfn main() => 0\n\
               @get(\"/items?name={name}\")\n\
               fn list_items(name: Str?) -> Str {\n\
                   return \"name={name}\"\n\
               }\n";
    let (status_falta, body_falta) = build_spawn_request(
        "http-query-nullable-missing",
        src,
        43502,
        "GET",
        "/items",
        None,
    );
    assert_eq!(status_falta, 200);
    assert_eq!(body_falta.trim(), "\"name=null\"");

    let (status_present, body_present) = build_spawn_request(
        "http-query-nullable-present",
        src,
        43502,
        "GET",
        "/items?name=fitz",
        None,
    );
    assert_eq!(status_present, 200);
    assert_eq!(body_present.trim(), "\"name=fitz\"");
}

#[test]
fn http_query_path_y_body_combinados() {
    // Caso completo: path param + query param + body. Cada categoría
    // se extrae del lugar correcto y el handler los recibe en el
    // orden declarado.
    let src = "@server(43503)\nfn main() => 0\n\
               type Patch { value: Int }\n\
               @put(\"/items/{id}?dry_run={dry_run}\")\n\
               fn update_item(id: Int, dry_run: Bool, body: Patch) -> Str {\n\
                   return \"id={id} dry_run={dry_run} value={body.value}\"\n\
               }\n";
    let (status, body) = build_spawn_request(
        "http-query-path-body",
        src,
        43503,
        "PUT",
        "/items/42?dry_run=true",
        Some(r#"{"value":7}"#),
    );
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"id=42 dry_run=true value=7\"");
}

#[test]
fn http_query_param_parse_error_es_400() {
    // `limit: Int` recibe `"abc"` → 400 con mensaje de coerción.
    let src = "@server(43504)\nfn main() => 0\n\
               @get(\"/items?limit={limit}\")\n\
               fn list_items(limit: Int) -> Int => limit\n";
    let (status, body) = build_spawn_request(
        "http-query-parse-error",
        src,
        43504,
        "GET",
        "/items?limit=abc",
        None,
    );
    assert_eq!(status, 400);
    assert!(
        body.contains("query param 'limit'"),
        "body debería mencionar el query param, fue: {}",
        body
    );
}

// ---------------------------------------------------------------------
// Fase 8.7.1 — codegen interop Python (F19 parcial)
// ---------------------------------------------------------------------
//
// Gated por `#[cfg(feature = "python")]` porque el binario generado
// linkea pyo3, lo cual exige Python disponible al build time +
// runtime. Para correr: `cargo test --features python --test
// compile_e2e fase_8_7_1`.

#[cfg(feature = "python")]
#[test]
fn fase_8_7_1_build_python_import_math_extrae_pi() {
    // Criterio mínimo 8.7.1: el codegen acepta `from python import
    // math`, compila a binario nativo standalone (con pyo3 linkeado),
    // el binario corre y produce el output esperado bit-a-bit con
    // `fitz run`. Validamos extracción primitiva PyAny → Float via
    // anotación destino en el `let pi: Float = math.pi`.
    let src = "from python import math\n\
               let pi: Float = math.pi\n\
               print(\"pi = {pi}\")\n";
    let (stdout, exit) = build_and_run("fase_8_7_1_math_pi", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    // `print` con Float usa __fitz_fmt_float que matchea el repr
    // canónico de Python para floats con fracción no trivial.
    assert_eq!(stdout.trim(), "pi = 3.141592653589793");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_1_build_python_import_sin_anotacion_es_opaco() {
    // Sin anotación destino, `let m = math.pi` queda como
    // `__FitzPyObject` opaco. `print(m)` delega al Display del newtype
    // que invoca `__str__` Python — paridad bit-a-bit con `fitz run`.
    let src = "from python import math\n\
               let pi = math.pi\n\
               print(\"pi = {pi}\")\n";
    let (stdout, exit) = build_and_run("fase_8_7_1_math_pi_opaco", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "pi = 3.141592653589793");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_math_sqrt_devuelve_result_ok() {
    // Criterio canónico 8.7.2: `math.sqrt(16.0)` desde `fitz build`
    // produce un binario que matchea bit-a-bit con `fitz run`.
    //
    // Como `call` Python devuelve `Result<PyAny>` (8.3), el binding
    // tiene que destrancarlo con `match` o `?`. Acá usamos `match` para
    // mantener el ejemplo top-level (sin envolver en fn `Result<...>`).
    let src = "from python import math\n\
               let raw = math.sqrt(16.0)\n\
               match raw {\n\
                 Ok(v) => print(\"sqrt(16) = {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_math_sqrt", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert_eq!(stdout.trim(), "sqrt(16) = 4.0");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_python_excepcion_es_err() {
    // `math.sqrt(-1)` lanza ValueError. El call devuelve
    // `Err(Str("ValueError: ..."))` que el match destrancla.
    let src = "from python import math\n\
               let raw = math.sqrt(-1.0)\n\
               match raw {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(e) => print(\"caught: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_math_sqrt_neg", src);
    assert_eq!(exit, 0);
    assert!(
        stdout.contains("caught: ValueError"),
        "output debería citar `ValueError`, fue: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_python_marshalla_list_fitz_a_list_python() {
    // `json.dumps([1, 2, 3])` Fitz → arg de json.dumps recibe una
    // List<Int> Fitz que se marshalla a list Python via el impl
    // genérico `__FitzToPy for Arc<Mutex<Vec<T>>>`.
    let src = "from python import json\n\
               let xs: List<Int> = [1, 2, 3]\n\
               let raw = json.dumps(xs)\n\
               match raw {\n\
                 Ok(s) => print(\"serializado = {s}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_marshal_list", src);
    assert_eq!(exit, 0);
    // El Ok(s) tiene `s: __FitzPyObject` (sin annot); Display delega
    // a `__str__` Python → cita literal del JSON entre comillas.
    assert!(
        stdout.contains("[1, 2, 3]"),
        "esperaba salida con [1, 2, 3], fue: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_marshalla_instance_fitz_a_dict_python() {
    // Caso canónico del roadmap 8.5: una Instance Fitz (User) pasa a
    // una función Python como dict. `json.dumps(user)` → JSON con los
    // fields de la instancia preservando orden de declaración.
    let src = "type User { id: Int, name: Str }\n\
               from python import json\n\
               let u = User { id: 1, name: \"Ada\" }\n\
               let raw = json.dumps(u)\n\
               match raw {\n\
                 Ok(s) => print(\"serializado = {s}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_marshal_instance", src);
    assert_eq!(exit, 0);
    assert!(
        stdout.contains("{\\\"id\\\": 1, \\\"name\\\": \\\"Ada\\\"}")
            || stdout.contains("{\"id\": 1, \"name\": \"Ada\"}"),
        "esperaba JSON con id+name preservando orden, fue: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_python_propagacion_con_try() {
    // Operador `?` Fitz adentro de fn que retorna `Result<Float>`
    // propaga el Err Python. La fn `root_safe` extrae el Float via
    // anotación destino sobre el Ok.
    let src = "from python import math\n\
               fn root_safe(x: Float) -> Result<Float> {\n\
                 let v: Float = math.sqrt(x)?\n\
                 return Ok(v)\n\
               }\n\
               match root_safe(25.0) {\n\
                 Ok(r) => print(\"r = {r}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_root_safe", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "r = 5.0");
}

// ---------------------------------------------------------------------
// Fase 8.7.3 — bridge async tokio ↔ asyncio
// ---------------------------------------------------------------------

#[cfg(feature = "python")]
#[test]
fn fase_8_7_3_build_await_corutina_asyncio_sleep() {
    // Criterio canónico 8.7.3: patrón canónico Fitz `<py_call>?.await`.
    // El `?` desempaca el `Result<Any>` del call (per 8.4 → 8.3); el
    // `.await` ejecuta la corutina vía `tokio::spawn_blocking` +
    // `asyncio.run_until_complete`. Paridad bit-a-bit con `fitz run`:
    // el intérprete usa el mismo patrón.
    let src = "from python import asyncio\n\
               async fn run() -> Result<Str> {\n\
                 let _ = asyncio.sleep(0.001)?.await\n\
                 return Ok(\"done\")\n\
               }\n\
               match run().await {\n\
                 Ok(v) => print(\"got = {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_3_asyncio_sleep", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert_eq!(stdout.trim(), "got = done");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_3_build_pipeline_con_multiples_awaits() {
    // Múltiples awaits encadenados sobre corutinas distintas — cada
    // uno ejecuta su propia `run_until_complete`. Paridad con el
    // ejemplo canónico 8.6.
    let src = "from python import asyncio\n\
               async fn pipeline(start: Int) -> Result<Int> {\n\
                 let _ = asyncio.sleep(0)?.await\n\
                 let a = start + 1\n\
                 let _ = asyncio.sleep(0)?.await\n\
                 let b = a * 2\n\
                 return Ok(b + 100)\n\
               }\n\
               match pipeline(10).await {\n\
                 Ok(v) => print(\"result = {v}\"),\n\
                 Err(_) => print(\"(no debería)\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_3_pipeline", src);
    assert_eq!(exit, 0);
    // (10+1) * 2 + 100 = 122
    assert_eq!(stdout.trim(), "result = 122");
}

// ---- Mini-tanda C — list comprehensions ----

#[test]
fn mini_tanda_c_comprehension_sobre_lista_compila_y_doblea() {
    // `[x * 2 for x in [1, 2, 3]]` debe producir `[2, 4, 6]` y el
    // binario nativo lo imprime con el formato canónico bit-a-bit
    // igual que `fitz run`.
    let src = "let r: List<Int> = [x * 2 for x in [1, 2, 3]]\nprint(r)\n";
    let (stdout, exit) = build_and_run("mini_tanda_c_comp_simple", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]");
}

#[test]
fn mini_tanda_c_comprehension_sobre_range_con_filter() {
    // `[n for n in 0..10 if n % 2 == 0]` filtra pares. La anotación
    // `List<Int>` ayuda al codegen a tipar concreto el iter Int.
    let src =
        "let r: List<Int> = [n for n in 0..10 if n % 2 == 0]\nprint(r)\n";
    let (stdout, exit) = build_and_run("mini_tanda_c_comp_filter", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[0, 2, 4, 6, 8]");
}

// ---- Mini-tanda Fm — format specifiers ----

#[test]
fn mini_tanda_fm_float_con_precision_decimal() {
    // `{ratio:.2f}` debe producir el mismo output bit-a-bit que
    // `fitz run` (es decir "0.50" para 0.5).
    let src = "let ratio: Float = 0.5\nprint(\"{ratio:.2f}\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_float_precision", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0.50");
}

#[test]
fn mini_tanda_fm_int_con_width_y_zero_pad() {
    // `{n:05d}` produce "00042".
    let src = "let n: Int = 42\nprint(\"{n:05d}\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_int_zero_pad", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "00042");
}

#[test]
fn mini_tanda_fm_hex_con_alternate() {
    // `{n:#x}` produce "0xff".
    let src = "let n: Int = 255\nprint(\"{n:#x}\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_hex_alt", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0xff");
}

#[test]
fn mini_tanda_fm_alignment_right_con_fill_default() {
    // `{x:>5}` padding con espacios a la derecha (right alignment).
    let src = "let x: Int = 42\nprint(\"[{x:>5}]\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_align_right", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[   42]");
}

// ---- Mini-tanda Md — for con Pattern (Map destructuring) ----

#[test]
fn mini_tanda_md_for_sobre_map_destructura_pares() {
    // `for (k, v) in m` itera sobre el Map bindeando k y v.
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
               let total: Int = 0\n\
               for (_, v) in m {\n\
                 total = total + v\n\
               }\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("mini_tanda_md_for_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "6");
}

#[test]
fn mini_tanda_md_for_wildcard_pattern_compila() {
    // `for _ in 0..5` ignora el elemento.
    let src = "let count: Int = 0\n\
               for _ in 0..5 {\n\
                 count = count + 1\n\
               }\n\
               print(count)\n";
    let (stdout, exit) = build_and_run("mini_tanda_md_wildcard", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "5");
}

// ---- Mini-tanda It — enumerate / zip / chain ----

#[test]
fn mini_tanda_it_enumerate_con_for_destructuring() {
    // `for (i, x) in xs.enumerate() { ... }` — caso canónico que
    // motiva la mini-tanda. Encaja con Md (tuple destructuring).
    let src = "let xs: List<Str> = [\"a\", \"b\", \"c\"]\n\
               for (i, x) in xs.enumerate() {\n\
                 print(\"{i}={x}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_it_enumerate", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0=a\n1=b\n2=c");
}

// ---- Mini-tanda Bits — operadores bit-a-bit ----

#[test]
fn mini_tanda_bits_and_or_xor_y_shifts() {
    // Operadores básicos sobre hex literales (combinación Lit + Bits).
    let src = "let mask: Int = 0xFF\n\
               let raw: Int = 0xABCD\n\
               let lo: Int = raw & mask\n\
               let hi: Int = (raw >> 8) & mask\n\
               let recombined: Int = (hi << 8) | lo\n\
               let xored: Int = raw ^ 0xFFFF\n\
               print(lo)\n\
               print(hi)\n\
               print(recombined)\n\
               print(xored)\n";
    let (stdout, exit) = build_and_run("mini_tanda_bits_basicos", src);
    assert_eq!(exit, 0);
    // lo=0xCD=205, hi=0xAB=171, recombined=0xABCD=43981, xored=0xFFFF^0xABCD=0x5432=21554.
    assert_eq!(stdout.trim(), "205\n171\n43981\n21554");
}

// ---- Mini-tanda Err+ — `Err` con tipos no-Str + `?` mensaje propio ----

#[test]
fn mini_tanda_err_plus_err_int_compila_y_corre() {
    // `Err(Int)` se coerce a Str via `format!("{}", n)` en codegen.
    // El value se preserva en el mensaje pero pierde el tipo (Result
    // sigue siendo Result<T, String> pinned).
    let src = "fn fail() -> Result<Int> {\n\
                 return Err(404)\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_err_plus_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: 404");
}

// ---- Mini-tanda Rt — tuple patterns con sub-pattern Str/Range/Or ----

#[test]
fn mini_tanda_rt_tuple_con_str_literal_subpattern() {
    // `("ada", n)` en codegen: Str literal como sub-pattern de Tuple.
    let src = "fn name(p: (Str, Int)) -> Str {\n\
                 return match p {\n\
                   (\"ada\", n) => \"ada n={n}\",\n\
                   (other, n) => \"{other} n={n}\"\n\
                 }\n\
               }\n\
               print(name((\"ada\", 5)))\n\
               print(name((\"bob\", 7)))\n";
    let (stdout, exit) = build_and_run("mini_tanda_rt_str_sub", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "ada n=5\nbob n=7");
}

#[test]
fn mini_tanda_rt_tuple_con_or_pattern_subpattern() {
    // `(name, 1 | 2)` en codegen: Or-pattern como sub-pattern.
    let src = "fn clasif(p: (Str, Int)) -> Str {\n\
                 return match p {\n\
                   (n, 1 | 2) => \"{n}: chico\",\n\
                   (n, m) => \"{n}: {m}\"\n\
                 }\n\
               }\n\
               print(clasif((\"ada\", 1)))\n\
               print(clasif((\"ada\", 2)))\n\
               print(clasif((\"bob\", 42)))\n";
    let (stdout, exit) = build_and_run("mini_tanda_rt_or_sub", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "ada: chico\nada: chico\nbob: 42");
}

#[test]
fn mini_tanda_rt_tuple_con_range_subpattern() {
    // `(name, 0..10)` en codegen: Range como sub-pattern.
    let src = "fn band(p: (Str, Int)) -> Str {\n\
                 return match p {\n\
                   (n, 0..10) => \"{n}: dig\",\n\
                   (n, 10..100) => \"{n}: dec\",\n\
                   (n, _) => \"{n}: big\"\n\
                 }\n\
               }\n\
               print(band((\"a\", 5)))\n\
               print(band((\"b\", 42)))\n\
               print(band((\"c\", 500)))\n";
    let (stdout, exit) = build_and_run("mini_tanda_rt_range_sub", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "a: dig\nb: dec\nc: big");
}

// ---- Mini-tanda Cmp — ops compuestos bit-a-bit + prefijos mayúscula ----

#[test]
fn mini_tanda_cmp_ops_compuestos_bit_a_bit() {
    let src = "let flags: Int = 0b0000_0101\n\
               flags |= 0b0010\n\
               print(flags)\n\
               flags &= 0b1110\n\
               print(flags)\n\
               flags ^= 0b0100\n\
               print(flags)\n\
               flags <<= 2\n\
               print(flags)\n\
               flags >>= 1\n\
               print(flags)\n";
    let (stdout, exit) = build_and_run("mini_tanda_cmp_compuestos", src);
    assert_eq!(exit, 0);
    // 0b101=5 | 0b010=2 → 0b111=7
    // 7 & 0b1110=14 → 0b110=6
    // 6 ^ 0b100=4 → 0b010=2
    // 2 << 2 → 8
    // 8 >> 1 → 4
    assert_eq!(stdout.trim(), "7\n6\n2\n8\n4");
}

#[test]
fn mini_tanda_cmp_prefijos_mayuscula() {
    let src = "let h: Int = 0XFF\n\
               let b: Int = 0B1010\n\
               let o: Int = 0O755\n\
               print(h)\n\
               print(b)\n\
               print(o)\n";
    let (stdout, exit) = build_and_run("mini_tanda_cmp_prefijos", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "255\n10\n493");
}

// ---- Mini-tanda Re+ — Result<T, E> tipado en codegen ----

#[test]
fn mini_tanda_re_plus_err_instance_fields_accesibles_en_build() {
    // El caso canónico que motiva Re+: tras anotar `Result<T, E>`
    // con E concreto, el binding `Err(e)` tipa con E real (no Str),
    // así que podés acceder a sus fields en `fitz build`.
    let src = "type ApiError { status: Int, msg: Str }\n\
               fn fetch() -> Result<Int, ApiError> {\n\
                 return Err(ApiError { status: 503, msg: \"unavailable\" })\n\
               }\n\
               match fetch() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(e) => print(\"err {e.status}: {e.msg}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_re_plus_fields", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err 503: unavailable");
}

#[test]
fn mini_tanda_re_plus_err_int_bindea_int_no_str() {
    // Antes de Re+, `Err(e)` siempre bindeaba `e: Str` en codegen.
    // Ahora con `Result<T, Int>` explícito, `e` tipa como Int.
    let src = "fn fail() -> Result<Str, Int> {\n\
                 return Err(404)\n\
               }\n\
               let code: Int = match fail() {\n\
                 Ok(_) => 0,\n\
                 Err(e) => e\n\
               }\n\
               print(code)\n";
    let (stdout, exit) = build_and_run("mini_tanda_re_plus_int_binding", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "404");
}

#[test]
fn mini_tanda_re_plus_legacy_result_t_sin_e_sigue_funcionando() {
    // Regresión: `Result<T>` sin E sigue funcionando (default Str).
    let src = "fn div(a: Int, b: Int) -> Result<Int> {\n\
                 if b == 0 { return Err(\"zero\") }\n\
                 return Ok(a / b)\n\
               }\n\
               match div(10, 0) {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_re_plus_legacy", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: zero");
}

#[test]
fn mini_tanda_err_plus_err_instance_compila_y_preserva_display() {
    // `Err(Instance)` deref del Arc<Mutex<TData>> antes del format!
    // — paridad bit-a-bit con el Display del intérprete.
    let src = "type ApiError { status: Int, msg: Str }\n\
               fn fetch() -> Result<Int> {\n\
                 return Err(ApiError { status: 503, msg: \"unavailable\" })\n\
               }\n\
               match fetch() {\n\
                 Ok(v) => print(\"ok\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_err_plus_instance", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: ApiError { status: 503, msg: \"unavailable\" }");
}

#[test]
fn mini_tanda_bits_not_unario() {
    // ~0 = -1, ~0xFF = -256 (i64 con signo).
    let src = "let a: Int = ~0\n\
               let b: Int = ~0xFF\n\
               print(a)\n\
               print(b)\n";
    let (stdout, exit) = build_and_run("mini_tanda_bits_not", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "-1\n-256");
}

#[test]
fn mini_tanda_it_zip_trunca_y_chain_concatena() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20]\n\
               let zipped: List<(Int, Int)> = xs.zip(ys)\n\
               print(zipped.len())\n\
               let chained: List<Int> = xs.chain(ys)\n\
               print(chained.len())\n\
               print(chained[4])\n";
    let (stdout, exit) = build_and_run("mini_tanda_it_zip_chain", src);
    assert_eq!(exit, 0);
    // zip truncado al más corto (len 2), chain concatena (3 + 2 = 5),
    // último elemento del chain es 20.
    assert_eq!(stdout.trim(), "2\n5\n20");
}
