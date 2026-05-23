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

/// Como `build_and_run` pero permite setear env vars sobre el child
/// que ejecuta el binario. Útil para tests de la mini-fase env builtin
/// — el binario hace `std::env::var(...)` y la var inyectada via
/// `Command::env` queda visible. Mini-fase env builtin (2026-05-22).
fn build_and_run_with_env(test_name: &str, src: &str, env_vars: &[(&str, &str)]) -> (String, i32) {
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

    let mut cmd = Command::new(&bin);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let run = cmd.output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
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
    assert_lines(&stdout, &["User { id: 1, name: \"Fitz\", email: null }"]);
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
        &["[1, 2, 3]", "3", "[1, 2, 3, 4]", "4", "1", "2", "3", "4"],
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
    // F13 cerrado (heterogéneos `Int/Float/Str/Bool/Null/Bytes/
    // Nominal` cubiertos con `__FitzValue` tagged runtime) — la
    // lista `[1, "dos"]` SÍ compila ahora y produce output
    // bit-a-bit con `fitz run`. El test mantiene el nombre
    // histórico (era pre-F13: heterogéneo abortaba); ahora valida
    // que la paridad se preserva.
    let src = "let xs = [1, \"dos\"]\nprint(xs)\n";
    let (stdout, exit) = build_and_run("heterogeneous-list-f13", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["[1, \"dos\"]"]);
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
    let (status, body) = build_spawn_request("mw3-passthrough", src, 43370, "GET", "/x", None);
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
    let (status, body) =
        build_spawn_request("mw3-shortcircuit", src, 43371, "GET", "/protected", None);
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
    let (status, raw_headers) =
        build_spawn_request_raw("mw3-preflight", src, 43372, "OPTIONS", "/api");
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
fn r_bug_options_preflight_duplicado_en_fitz_build_paridad_con_fitz_run() {
    // Regresión del bug "Overlapping method route. Handler for `OPTIONS
    // /tasks` already exists" (2026-05-22). Cuando varios handlers
    // comparten path con CORS, `fitz build` paniqueaba en boot del
    // binario al construir el axum::Router (mismo bug que en `fitz run`,
    // fixeado en codegen también).
    //
    // Caso del 6to boilerplate (api-fullstack-postgres): `/tasks` con
    // @get + @post + `/tasks/{id}` con @get + @put + @delete, todos
    // con CORS. Pre-fix el binario emitido salía con exit code 101.
    let src = "\
@server(43373)
fn main() => 0

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"GET\", \"OPTIONS\"]}))
@get(\"/tasks\")
fn list_tasks() -> Str => \"[]\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"POST\", \"OPTIONS\"]}))
@post(\"/tasks\")
fn create_task() -> Str => \"created\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"GET\", \"OPTIONS\"]}))
@get(\"/tasks/{id}\")
fn get_task(id: Int) -> Str => \"one\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"PUT\", \"OPTIONS\"]}))
@put(\"/tasks/{id}\")
fn update_task(id: Int) -> Str => \"updated\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"DELETE\", \"OPTIONS\"]}))
@delete(\"/tasks/{id}\")
fn delete_task(id: Int) -> Str => \"deleted\"
";
    // El binario debe arrancar y responder OPTIONS /tasks con 204 +
    // headers Access-Control-Allow-* (merge: GET + POST + OPTIONS).
    let (status, raw_headers) = build_spawn_request_raw(
        "r-bug-options-preflight-duplicado",
        src,
        43373,
        "OPTIONS",
        "/tasks",
    );
    assert_eq!(status, 204, "headers: {}", raw_headers);
    let h_lower = raw_headers.to_lowercase();
    let methods_line = h_lower
        .lines()
        .find(|l| l.starts_with("access-control-allow-methods:"))
        .unwrap_or_else(|| panic!("falta Access-Control-Allow-Methods: {}", raw_headers))
        .to_string();
    assert!(
        methods_line.contains("get") && methods_line.contains("post"),
        "merged methods debe incluir GET y POST: {}",
        methods_line
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
    let (status, raw_headers) = build_spawn_request_raw("mw3-cors-real", src, 43373, "GET", "/api");
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
    assert_lines(&stdout, &["#1 es Fitz", "falló: usuario no encontrado"]);
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
    assert_lines(&stdout, &["hola, Fitz!", "falta: no encontrado"]);
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
    assert_lines(&stdout, &["a vale 1", "err: clave no encontrada: z"]);
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
    let (stdout, exit) = build_and_run_multi("module-basic", main, &[("utils.fitz", utils)]);
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
    let (stdout, exit) = build_and_run_multi("module-import-const", main, &[("utils.fitz", utils)]);
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
    let (stdout, exit) =
        build_and_run_multi("module-namespace-only", main, &[("utils.fitz", utils)]);
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
    let (stdout, exit) =
        build_and_run_multi("module-default-with-const", main, &[("utils.fitz", utils)]);
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
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-ns", main, &[("utils.fitz", utils)]);
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
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-fn", main, &[("utils.fitz", utils)]);
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
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-type", main, &[("utils.fitz", utils)]);
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
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-const", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["local", "remoto"]);
}

#[test]
fn modulo_inexistente_aborta_build() {
    let stderr = build_expect_fail_multi("module-not-found", "import inexistente\nprint(0)\n", &[]);
    assert!(
        stderr.contains("no se encontró el módulo") || stderr.contains("inexistente"),
        "esperaba mensaje de módulo no encontrado, fue: {}",
        stderr
    );
}

#[test]
fn modulo_con_import_propio_compila_via_import_transitivo() {
    // F15: imports transitivos ahora compilan. Un módulo cargado puede
    // tener su propio `import` que el codegen sigue recursivamente.
    let main = "\
import primero
print(primero.x())
";
    let primero = "\
from segundo import dos
fn x() -> Int => dos()
";
    let segundo = "fn dos() -> Int => 2";
    let (stdout, exit) = build_and_run_multi(
        "module-transitivo-ok",
        main,
        &[("primero.fitz", primero), ("segundo.fitz", segundo)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["2"]);
}

#[test]
fn f15_import_transitivo_namespace_y_named_mixto() {
    // Cobertura más amplia: un módulo cargado usa `import` (namespace)
    // y otro módulo usa `from ... import` (named) — ambos transitivos.
    let main = "\
import a
print(a.compose(\"Fitz\"))
";
    let a = "\
import b
from c import upper
fn compose(name: Str) -> Str => upper(b.greet(name))
";
    let b = "\
let PREFIX = \"hola, \"
fn greet(name: Str) -> Str => \"{PREFIX}{name}\"
";
    let c = "fn upper(s: Str) -> Str => s.upper()";
    let (stdout, exit) = build_and_run_multi(
        "f15-namespace-y-named-mixto",
        main,
        &[("a.fitz", a), ("b.fitz", b), ("c.fitz", c)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["HOLA, FITZ"]);
}

#[test]
fn f15_ciclo_de_imports_transitivos_aborta_con_error_claro() {
    // F15: ciclo de imports detectado por el loader del codegen
    // (paralelo al evaluator). a → b → a debe abortar el build.
    let main = "\
import a
print(a.x())
";
    let a = "\
import b
fn x() -> Int => 1
";
    let b = "\
import a
fn y() -> Int => 2
";
    let stderr =
        build_expect_fail_multi("f15-ciclo-imports", main, &[("a.fitz", a), ("b.fitz", b)]);
    assert!(
        stderr.contains("ciclo de imports"),
        "esperaba mensaje sobre ciclo de imports, fue: {}",
        stderr
    );
}

#[test]
fn f15_import_transitivo_con_type_compartido() {
    // F15: un type definido en C, usado por B, expuesto via fn de A.
    // El codegen del módulo A necesita conocer la sig de `User` para
    // que `from c import User` resuelva al `pub fn` que retorna.
    let main = "\
import a
print(a.make_user(7))
";
    let a = "\
from c import User
fn make_user(id: Int) -> User => User { id: id }
";
    let _b = ""; // sin uso
    let c = "type User { id: Int = 0 }";
    let (stdout, exit) =
        build_and_run_multi("f15-type-compartido", main, &[("a.fitz", a), ("c.fitz", c)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["User { id: 7 }"]);
}

// ---------------------------------------------------------------------------
// Mini-tanda F14 — `let X = <expr>` no-literal a nivel top de módulo.
// Const-eval → `pub const X: T = <rhs>;`. Runtime → `pub fn X() -> T`.
// ---------------------------------------------------------------------------

#[test]
fn f14_modulo_let_const_eval_compila_y_devuelve_valor_inlineado() {
    // Const-eval: `let X = 60 * 60` y `let Y = X / 36`.
    // El segundo no es const-eval estricto (referencia un Ident), pero
    // F14 lo emite como accessor fn — el call site `utils.Y` se
    // traduce a `utils::Y()`.
    let main = "\
import utils
print(utils.SECONDS)
print(utils.MAX)
";
    let utils = "\
let SECONDS: Int = 60 * 60
let MAX: Int = SECONDS / 36
";
    let (stdout, exit) = build_and_run_multi("f14-let-const-eval", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["3600", "100"]);
}

#[test]
fn f14_modulo_let_runtime_str_concat_compila() {
    // Str concat no es const-eval (Rust no acepta `String + String`
    // en const). F14 lo emite como `pub fn GREETING() -> String`,
    // el call site `utils.GREETING` se traduce a `utils::GREETING()`.
    let main = "\
import utils
print(utils.GREETING)
";
    let utils = "let GREETING: Str = \"hola, \" + \"Fitz\"";
    let (stdout, exit) = build_and_run_multi("f14-let-str-concat", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz"]);
}

#[test]
fn f14_modulo_let_runtime_struct_lit_via_fn_call() {
    // RHS = fn call que retorna instancia. F14 emite accessor fn que
    // re-evalúa la RHS en cada referencia.
    let main = "\
from utils import User
import utils
let u = utils.DEFAULT_USER
print(u)
";
    let utils = "\
type User { id: Int = 0, name: Str = \"anon\" }
fn make() -> User => User {}
let DEFAULT_USER: User = make()
";
    let (stdout, exit) = build_and_run_multi(
        "f14-let-struct-lit-via-call",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["User { id: 0, name: \"anon\" }"]);
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

    // Parsear status + body. Formato: "HTTP/1.1 <code> <reason>\r\n...\r\n\r\n<body>"
    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    let body = raw[body_start..].to_string();
    (status, body)
}

#[test]
fn http_get_simple_responde_200_y_body() {
    // El criterio mínimo de 5b.6: un handler GET que devuelve un Str
    // produce 200 + JSON con el string.
    let src = "@server(43210)\nfn main() => 0\n\
               @get(\"/\") fn index() -> Str => \"Fitz HTTP corriendo\"\n";
    let (status, body) = build_spawn_request("http-get-simple", src, 43210, "GET", "/", None);
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
    let (status_err, body_err) =
        build_spawn_request("http-result-err", src, 43212, "GET", "/d/10/0", None);
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

/// Mini-tanda UC + HA — variante del helper que permite especificar
/// el Content-Type explícitamente, para los tests de urlencoded y 415.
#[allow(clippy::too_many_arguments)]
fn build_spawn_request_with_ct(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    content_type: Option<&str>,
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
    let request = match body {
        Some(b) => {
            let ct_header = match content_type {
                Some(ct) => format!("Content-Type: {}\r\n", ct),
                None => String::new(),
            };
            format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\n{}\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                method,
                path,
                addr,
                ct_header,
                b.len(),
                b
            )
        }
        None => format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            method, path, addr
        ),
    };
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
    let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    let body = raw[body_start..].to_string();
    (status, body)
}

#[test]
fn uc_http_post_urlencoded_parsea_a_map_str_str() {
    // Mini-tanda UC: `fitz build` debe aceptar bodies
    // `application/x-www-form-urlencoded` y parsearlos como
    // `Map<Str, Str>` igual que el intérprete.
    let src = "@server(43240)\nfn main() => 0\n\
               @post(\"/echo\") fn echo(body: Map<Str, Str>) -> Map<Str, Str> => body\n";
    let (status, body) = build_spawn_request_with_ct(
        "uc-post-urlenc-basic",
        src,
        43240,
        "POST",
        "/echo",
        Some("name=Fitz&age=25"),
        Some("application/x-www-form-urlencoded"),
    );
    assert_eq!(
        status, 200,
        "esperaba 200, fue: status={} body={}",
        status, body
    );
    assert!(
        body.contains("\"name\":\"Fitz\"") && body.contains("\"age\":\"25\""),
        "esperaba body parsea pares name/age, fue: {}",
        body
    );
}

#[test]
fn uc_http_post_urlencoded_con_url_encoding() {
    // URL-decoding: `+` → espacio, `%20` → espacio.
    let src = "@server(43241)\nfn main() => 0\n\
               @post(\"/echo\") fn echo(body: Map<Str, Str>) -> Map<Str, Str> => body\n";
    let (status, body) = build_spawn_request_with_ct(
        "uc-post-urlenc-decoded",
        src,
        43241,
        "POST",
        "/echo",
        Some("greeting=hola+mundo&place=Fitz%20Roy"),
        Some("application/x-www-form-urlencoded"),
    );
    assert_eq!(status, 200);
    assert!(
        body.contains("\"greeting\":\"hola mundo\"") && body.contains("\"place\":\"Fitz Roy\""),
        "esperaba URL-decoding aplicado, fue: {}",
        body
    );
}

#[test]
fn ha_http_content_type_text_plain_es_415_con_msg_claro() {
    // Mini-tanda HA: el msg del 415 cubre los formatos no soportados.
    // Mini-tanda MP-Build — multipart ya es soportado por el codegen
    // también (paridad bit-a-bit con `fitz run`). Probamos con
    // `text/plain` que sigue siendo rechazado.
    let src = "@server(43242)\nfn main() => 0\n\
               type Input { msg: Str }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request_with_ct(
        "ha-post-415-text-plain",
        src,
        43242,
        "POST",
        "/echo",
        Some("hola mundo"),
        Some("text/plain"),
    );
    assert_eq!(
        status, 415,
        "esperaba 415, fue: status={} body={}",
        status, body
    );
    assert!(
        body.contains("Content-Type no soportado"),
        "esperaba `Content-Type no soportado`, fue: {}",
        body
    );
    assert!(
        body.contains("application/json")
            && body.contains("application/x-www-form-urlencoded")
            && body.contains("multipart/form-data"),
        "esperaba que el mensaje mencione los 3 CTs soportados, fue: {}",
        body
    );
}

// ---------------------------------------------------------------------------
// Mini-tandas DZ + CT + OAPI — paridad chica run↔build
// ---------------------------------------------------------------------------

#[test]
fn dz_division_int_por_cero_compila_y_panica_con_msg_alineado() {
    // Pre-DZ: `print(10 / 0)` rechaza rustc con `unconditional_panic`.
    // Post-DZ: compila y panica en runtime con el mismo msg que el
    // intérprete ("división por cero").
    let dir = std::env::temp_dir().join("fitz-e2e-dz-int");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    std::fs::write(&src_path, "print(10 / 0)\n").expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out.status.success(),
        "esperaba que `fitz build` con `10/0` compile (no const-eval reject), stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bin = dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    assert!(bin.exists(), "binario no existe: {}", bin.display());
    let run = Command::new(&bin).output().expect("run prog");
    assert!(
        !run.status.success(),
        "esperaba exit code != 0 (panic), fue: {:?}",
        run.status
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("división por cero"),
        "esperaba msg `división por cero`, stderr: {}",
        stderr
    );
}

#[test]
fn dz_division_float_por_cero_compila_y_panica_con_msg_alineado() {
    let dir = std::env::temp_dir().join("fitz-e2e-dz-float");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    std::fs::write(&src_path, "print(3.14 / 0.0)\n").expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(out.status.success());
    let bin = dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    let run = Command::new(&bin).output().expect("run");
    assert!(!run.status.success(), "esperaba panic");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("división por cero"),
        "esperaba msg `división por cero`, stderr: {}",
        stderr
    );
}

#[test]
fn ct_comparar_int_vs_str_compila_y_devuelve_false() {
    // `1 == "1"` y `1 != "1"` ahora compilan a literal false/true.
    let src = "print(1 == \"1\")\nprint(1 != \"1\")\nprint(true == 0)\n";
    let (stdout, exit) = build_and_run("ct-incompat", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["false", "true", "false"]);
}

#[test]
fn ct_paridad_bit_a_bit_run_vs_build_comparaciones_incompatibles() {
    // Paridad bit-a-bit `fitz run` ↔ `fitz build` para `==`/`!=`
    // entre tipos primitivos incompatibles.
    let dir = std::env::temp_dir().join("fitz-e2e-ct-parity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    let src = "\
print(1 == \"1\")
print(\"x\" != 1.5)
print(true == null)
print(false != \"f\")
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(out_run.status.success());
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(out_build.status.success());
    let bin = dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    let exec = Command::new(&bin).output().expect("run prog");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "esperaba paridad bit-a-bit `run` ↔ `build`"
    );
    assert!(run_stdout.contains("false"));
    assert!(run_stdout.contains("true"));
}

#[test]
fn mp_build_multipart_text_field_compila_y_parsea() {
    // Mini-tanda MP-Build — multipart text-only en `fitz build`.
    // Paridad bit-a-bit con `fitz run` (que ya lo soportaba en MP2).
    let src = "@server(43370)\nfn main() => 0\n\
               @post(\"/form\") fn form(body: Map<Str, Str>) -> Str {\n\
                   let n = body[\"name\"]\n\
                   return \"got \" + n\n\
               }\n";
    let (status, body) = build_spawn_request_with_ct(
        "mp-build-text",
        src,
        43370,
        "POST",
        "/form",
        Some("--X\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nFitz\r\n--X--"),
        Some("multipart/form-data; boundary=X"),
    );
    assert_eq!(status, 200, "esperaba 200, fue: {} body={}", status, body);
    assert!(
        body.contains("got Fitz"),
        "esperaba body con `got Fitz`, fue: {}",
        body
    );
}

#[test]
fn mp_build_multipart_file_field_compila_y_parsea() {
    // Mini-tanda MP-Build — multipart con file field. El handler
    // lee `len(f.content)` (no usa `f.name` que es Str? — para
    // evitar el caveat de narrowing del checker en este test E2E).
    let src = "@server(43371)\nfn main() => 0\n\
               @post(\"/upload\") fn upload(body: Map<Str, File>) -> Str {\n\
                   let f = body[\"doc\"]\n\
                   let n = len(f.content)\n\
                   return \"size={n}\"\n\
               }\n";
    let (status, body) = build_spawn_request_with_ct(
        "mp-build-file",
        src,
        43371,
        "POST",
        "/upload",
        Some("--X\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nfile contents\r\n--X--"),
        Some("multipart/form-data; boundary=X"),
    );
    assert_eq!(status, 200, "esperaba 200, fue: {} body={}", status, body);
    assert!(
        body.contains("size=13"),
        "esperaba `size=13` (len de 'file contents'), fue: {}",
        body
    );
}

#[test]
fn mp_build_multipart_sin_boundary_es_400() {
    let src = "@server(43372)\nfn main() => 0\n\
               @post(\"/form\") fn form(body: Map<Str, Str>) -> Str => \"ok\"\n";
    let (status, body) = build_spawn_request_with_ct(
        "mp-build-no-boundary",
        src,
        43372,
        "POST",
        "/form",
        Some("--X\r\n--X--"),
        Some("multipart/form-data"),
    );
    assert_eq!(status, 400, "esperaba 400, fue: {} body={}", status, body);
    assert!(
        body.contains("boundary"),
        "esperaba mención de boundary, fue: {}",
        body
    );
}

#[test]
fn f13_spike_lista_heterogenea_compila_y_paridad_bit_a_bit() {
    // F13 SPIKE — última residual del bloque post-Fase-8.
    // Listas heterogéneas (`[1, "dos", true]`) ya compilan a binario
    // nativo y producen output bit-a-bit idéntico a `fitz run`.
    // Antes del SPIKE el codegen rechazaba con "homogénea requerida".
    let dir = std::env::temp_dir().join("fitz-e2e-f13-spike");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    let src = "\
let xs = [1, \"dos\", true]
let ys = [42, 3.14, null, false, \"hola\"]
print(xs)
print(ys)
print(len(xs))
print(len(ys))
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(
        out_run.status.success(),
        "fitz run falló: {}",
        String::from_utf8_lossy(&out_run.stderr)
    );
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out_build.status.success(),
        "fitz build falló (F13 SPIKE no aplicó): {}",
        String::from_utf8_lossy(&out_build.stderr)
    );
    let bin = dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    let exec = Command::new(&bin).output().expect("ejecutar binario");
    assert!(exec.status.success(), "binario fallló al ejecutar");
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "F13 SPIKE: esperaba paridad bit-a-bit `run` ↔ `build`"
    );
    assert!(run_stdout.contains("[1, \"dos\", true]"));
    assert!(run_stdout.contains("3.14"));
}

#[test]
fn f13_a_bytes_y_nominal_en_lista_heterogenea_paridad_run_vs_build() {
    // F13.A + F13.B — Bytes y Nominales adentro de listas
    // heterogéneas. Paridad bit-a-bit `fitz run` ↔ `fitz build`.
    let dir = std::env::temp_dir().join("fitz-e2e-f13-a-b");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    let src = "\
type User { id: Int, name: Str }
let u = User { id: 1, name: \"ana\" }
let xs = [1, b\"raw\", u, true]
print(xs)
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(out_run.status.success());
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out_build.status.success(),
        "build falló: {}",
        String::from_utf8_lossy(&out_build.stderr)
    );
    let bin = dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    let exec = Command::new(&bin).output().expect("run prog");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "F13.A+B: esperaba paridad bit-a-bit"
    );
    assert!(run_stdout.contains("b\"raw\""));
    assert!(run_stdout.contains("User { id: 1, name: \"ana\" }"));
}

#[test]
fn f13_a_map_heterogeneo_paridad_run_vs_build() {
    // F13.A — Map heterogéneo (values mixtos) paridad bit-a-bit.
    let dir = std::env::temp_dir().join("fitz-e2e-f13-a-map");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    let src = "let cfg = {\"name\": \"fitz\", \"count\": 7, \"on\": true}\nprint(cfg)\n";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(out_run.status.success());
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(out_build.status.success());
    let bin = dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    let exec = Command::new(&bin).output().expect("run prog");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
    );
    assert!(run_stdout.contains("\"name\": \"fitz\""));
    assert!(run_stdout.contains("\"count\": 7"));
    assert!(run_stdout.contains("\"on\": true"));
}

#[test]
fn f13_lista_con_tipo_complejo_aborta_con_msg_claro() {
    // F13.E follow-up cerrado (post-9.w.2) — el caso `[1, [2, 3]]`
    // (List anidada como item de heterogéneo) ahora SÍ compila con
    // `fitz build` y produce output bit-a-bit con `fitz run`. La
    // fix fue extender la heurística sintáctica
    // `program_uses_fitz_value` para detectar mezcla
    // primitivo+compuesto (List/Map/StructLit/Bytes) — sin eso, el
    // walk detectaba la heterogeneidad pero el preludio FitzValue
    // ya se había decidido no emitir.
    //
    // El test mantiene el nombre histórico para que git log siga
    // siendo navegable, pero ahora valida la paridad en lugar del
    // abort.
    let src = "let xs = [1, [2, 3]]\nprint(xs)\n";
    let (stdout, exit) = build_and_run("f13-list-anidada-en-heterogeneo", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["[1, [2, 3]]"]);
}

#[test]
fn mw_wrap_codegen_rechaza_con_msg_que_cita_fitz_run() {
    // Mini-tanda Mw-Wrap — el codegen rechaza wrap-style mws con
    // un mensaje claro citando `fitz run` como workaround.
    // El intérprete sí los soporta (deuda residual del codegen).
    let dir = std::env::temp_dir().join("fitz-e2e-mw-wrap-codegen");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    let src = "\
@server(43500)
fn main() => 0

fn timing(req: Request, next: Fn() -> Response) -> Response {
    let r = next()
    return 200 {\"ok\": true}
}

@middleware(timing)
@get(\"/wrapped\")
fn wrapped() -> Str => \"handler\"
";
    std::fs::write(&src_path, src).expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        !out.status.success(),
        "esperaba que `fitz build` rechace wrap-style mws"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wrap-style") && stderr.contains("fitz run"),
        "esperaba msg sobre wrap-style + workaround `fitz run`, fue: {}",
        stderr
    );
}

#[test]
fn bytes_paridad_bit_a_bit_run_vs_build() {
    // Mini-tanda Bytes — el output de `fitz run` y `fitz build`
    // deben coincidir bit-a-bit para todos los casos canónicos:
    // literal con escapes, len, is_empty, to_str Ok/Err.
    let dir = std::env::temp_dir().join("fitz-e2e-bytes-paridad");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    let src = "\
let a = b\"hola\"
let b = b\"\\x00\\xff\"
let empty = b\"\"
print(a)
print(b)
print(a.len())
print(empty.is_empty())
print(a.is_empty())
let r = a.to_str()
print(r)
let s = bytes(\"converted\")
print(s)
print(len(s))
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(
        out_run.status.success(),
        "fitz run falló: {}",
        String::from_utf8_lossy(&out_run.stderr)
    );
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out_build.status.success(),
        "fitz build falló: {}",
        String::from_utf8_lossy(&out_build.stderr)
    );
    let bin = dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    let exec = Command::new(&bin).output().expect("ejecutar binario");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "esperaba paridad bit-a-bit `run` ↔ `build`"
    );
    // Sanity sobre el contenido.
    assert!(run_stdout.contains("b\"hola\""));
    assert!(run_stdout.contains("b\"\\x00\\xff\""));
    assert!(run_stdout.contains("4"));
    assert!(run_stdout.contains("true"));
    assert!(run_stdout.contains("Ok(\"hola\")"));
}

#[test]
fn oapi_return_ident_a_const_top_level_compila_y_emite_schema() {
    // `return NOT_FOUND { ... }` con NOT_FOUND const top-level
    // ahora parsea, compila a binario, y entra al schema OpenAPI.
    let dir = std::env::temp_dir().join("fitz-e2e-oapi-ident-build");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join("prog.fitz");
    let src = "\
let NOT_FOUND = 404
@server(43250)
fn main() => 0
@get(\"/u/{id}\")
fn h(id: Int) -> Int {
    if (id == 0) {
        return NOT_FOUND {\"error\": \"x\"}
    }
    return id
}
";
    std::fs::write(&src_path, src).expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out.status.success(),
        "esperaba compile OK, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let openapi = Command::new(fitz_bin())
        .args(["openapi"])
        .arg(&src_path)
        .output()
        .expect("fitz openapi");
    assert!(openapi.status.success());
    let schema = String::from_utf8_lossy(&openapi.stdout);
    assert!(
        schema.contains("\"404\""),
        "esperaba 404 en el schema OpenAPI, fue: {}",
        schema
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
    "03d-identifiers-unicode.fitz",
    "04-operadores.fitz",
    "04b-operadores-bit.fitz",
    "04c-asignacion-compuesta-bit.fitz",
    "05-strings.fitz",
    "05b-format-specs.fitz",
    "05e-bytes.fitz",
    "05d-escapes-extendidos.fitz",
    "06-logica.fitz",
    "07-if.fitz",
    "08-loops.fitz",
    "08b-loops-avanzados.fitz",
    "09b-indexing-slicing.fitz",
    "09c-tuples.fitz",
    "09d-comprehensions.fitz",
    "09e-for-map.fitz",
    "09f-let-destructure-rico.fitz",
    "10-match.fitz",
    "10b-match-tuple-subpatterns.fitz",
    "11-funciones.fitz",
    "11b-default-params.fitz",
    "11c-varargs.fitz",
    "11d-named-args.fitz",
    "12-type.fitz",
    "13-metodos.fitz",
    "13b-metodos-custom.fitz",
    "13c-metodos-extras.fitz",
    "13d-iteradores.fitz",
    "13e-mini-bundle-metodos.fitz",
    "13f-range-iteradores.fitz",
    "13g-static-methods.fitz",
    "13h-predicados-list.fitz",
    "13i-campos-privados.fitz",
    "13j-extras-str-map.fitz",
    "13k-flat-map-first-last-merge.fitz",
    "13l-update-comp-tuple-paramnames.fitz",
    "13m-min-max-sum-pad-keys-step.fitz",
    "13n-reduce-product-chars-entries-to-map.fitz",
    "13o-higher-order-y-consts-globales.fitz",
    "13p-mb4-y-comprehensions-extendidas.fitz",
    "13q-mb5-y-async-closures.fitz",
    "13r-mb6-y-async-build.fitz",
    "13s-mb7-y-fmt-build.fitz",
    "13t-mb8-bits-y-fmt-g.fitz",
    "13u-math-mb9-y-int-float.fitz",
    "13v-return-en-match.fitz",
    "14-result.fitz",
    // 14b: usa `Err(Int)` y `Err(Instance)` — el codegen pinea Err
    // como String, así que `fitz build` falla. Documentado en el
    // ejemplo como deuda residual de Err+. Sí corre en `fitz run`.
    "14c-result-tipado.fitz",
    "14d-err-compuestos.fitz",
    "16-modulos.fitz",
    "16b-modulos-let-expr.fitz",
    "16c-modulos-transitivos.fitz",
    "16d-import-multilinea.fitz",
    "17-http.fitz",
    "17b-middleware.fitz",
    "17c-multipart.fitz",
    "18-docs.fitz",
    "19-async.fitz",
    "19b-paralelismo.fitz",
    "20-build.fitz",
    "23-fmt-ejemplo.fitz",
    "24-tests.fitz",
    "28-auth.fitz",
    "29-ws.fitz",
    "29b-ws-binary.fitz",
    "29c-ws-bidir.fitz",
    "30-cron-background.fitz",
    "31-env.fitz",
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
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
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
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
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
        body.contains("\"id\":1")
            && body.contains("\"name\":\"ana\"")
            && body.contains("\"id\":2")
            && body.contains("\"name\":\"luis\""),
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
        &[("DELETE", "/users/2", None), ("GET", "/users", None)],
    );
    assert_eq!(results[0].0, 200);
    assert_eq!(results[1].0, 200);
    assert!(
        results[1].1.contains("\"name\":\"ana\"") && !results[1].1.contains("\"name\":\"luis\""),
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
    let results = build_spawn_requests("http-state-unused", src, 43324, &[("GET", "/", None)]);
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
    let (status, body) =
        build_spawn_request("http-status-401", src, 43400, "GET", "/protected", None);
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
    let (status_ok, body_ok) =
        build_spawn_request("http-status-mix-ok", src, 43401, "GET", "/u/1", None);
    assert_eq!(status_ok, 200);
    assert_eq!(body_ok.trim(), "\"alice\"");

    let (status_404, body_404) =
        build_spawn_request("http-status-mix-404", src, 43401, "GET", "/u/2", None);
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
    let src = "let r: List<Int> = [n for n in 0..10 if n % 2 == 0]\nprint(r)\n";
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
    // Re+ post-tightened: `Result<Int>` con `Err(Int)` ahora requiere
    // declarar el E explícito (`Result<Int, Int>`).
    let src = "fn fail() -> Result<Int, Int> {\n\
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
    // — paridad bit-a-bit con el Display del intérprete. Post-Re+ se
    // requiere declarar el E explícito.
    let src = "type ApiError { status: Int, msg: Str }\n\
               fn fetch() -> Result<Int, ApiError> {\n\
                 return Err(ApiError { status: 503, msg: \"unavailable\" })\n\
               }\n\
               match fetch() {\n\
                 Ok(v) => print(\"ok\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_err_plus_instance", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "err: ApiError { status: 503, msg: \"unavailable\" }"
    );
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

// ---- Mini-tanda Up — Map.update + list comprehension tuple destructure ----

#[test]
fn up_map_update_compila() {
    let src = "let scores: Map<Str, Int> = {\"ada\": 80, \"bob\": 45}\n\
               let bumped: Map<Str, Int> = scores.update(\"ada\", fn(v: Int) => v + 10)\n\
               print(bumped[\"ada\"])\n\
               print(bumped[\"bob\"])\n\
               let nochange: Map<Str, Int> = scores.update(\"missing\", fn(v: Int) => v + 999)\n\
               print(nochange.len())\n\
               print(nochange[\"ada\"])\n";
    let (stdout, exit) = build_and_run("up_map_update", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "90\n45\n2\n80");
}

#[test]
fn up_comprehension_tuple_destructure_compila() {
    let src = "let pairs: List<(Int, Int)> = [(1, 10), (2, 20), (3, 30)]\n\
               let sums: List<Int> = [a + b for (a, b) in pairs]\n\
               print(sums)\n\
               let firsts: List<Int> = [a for (a, _) in pairs]\n\
               print(firsts)\n";
    let (stdout, exit) = build_and_run("up_comp_tuple", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[11, 22, 33]\n[1, 2, 3]");
}

// ---- Mini-tanda Ex2 — List.flat_map/first/last + Map.merge ----

#[test]
fn ex2_list_flat_map_compila() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let r: List<Int> = xs.flat_map(fn(n: Int) => [n, n * 10])\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("ex2_flat_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 10, 2, 20, 3, 30]");
}

#[test]
fn ex2_list_first_y_last_devuelven_result() {
    let src = "let xs: List<Int> = [42, 7, 100]\n\
               match xs.first() {\n\
                 Ok(v) => print(\"first: {v}\"),\n\
                 Err(_) => print(\"empty\")\n\
               }\n\
               match xs.last() {\n\
                 Ok(v) => print(\"last: {v}\"),\n\
                 Err(_) => print(\"empty\")\n\
               }\n\
               let empty: List<Int> = []\n\
               match empty.first() {\n\
                 Ok(v) => print(\"first: {v}\"),\n\
                 Err(_) => print(\"empty list\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ex2_first_last", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "first: 42\nlast: 100\nempty list");
}

#[test]
fn ex2_map_merge_compila() {
    let src = "let m1: Map<Str, Int> = {\"a\": 1, \"b\": 2}\n\
               let m2: Map<Str, Int> = {\"b\": 20, \"c\": 3}\n\
               let r: Map<Str, Int> = m1.merge(m2)\n\
               print(r.len())\n\
               print(r[\"a\"])\n\
               print(r[\"b\"])\n\
               print(r[\"c\"])\n";
    let (stdout, exit) = build_and_run("ex2_map_merge", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3\n1\n20\n3");
}

// ---- Mini-tanda Ex — extras: Str search + Map filter/map_values ----

#[test]
fn ex_str_find_index_of_compilan() {
    let src = "let s: Str = \"hola mundo, hola fitz\"\n\
               match s.find(\"hola\") {\n\
                 Ok(i) => print(\"find: {i}\"),\n\
                 Err(_) => print(\"no\")\n\
               }\n\
               match s.last_index_of(\"hola\") {\n\
                 Ok(i) => print(\"last: {i}\"),\n\
                 Err(_) => print(\"no\")\n\
               }\n\
               match s.index_of(\"nope\") {\n\
                 Ok(i) => print(\"idx: {i}\"),\n\
                 Err(_) => print(\"not found\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ex_str_search", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "find: 0\nlast: 12\nnot found");
}

#[test]
fn ex_map_filter_y_map_values_compilan() {
    let src = "let scores: Map<Str, Int> = {\"ada\": 80, \"bob\": 45, \"cam\": 92}\n\
               let passing: Map<Str, Int> = scores.filter(fn(k: Str, v: Int) => v >= 60)\n\
               print(passing.len())\n\
               let doubled: Map<Str, Int> = scores.map_values(fn(v: Int) => v * 2)\n\
               print(doubled[\"ada\"])\n\
               print(doubled[\"bob\"])\n";
    let (stdout, exit) = build_and_run("ex_map_transforms", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "2\n160\n90");
}

// ---- Mini-tanda CM — cross-module method dispatch ----

#[test]
fn cm_metodos_custom_sobre_tipos_importados_compilan() {
    // Antes de CM: `from foo import User` + `u.greet()` fallaba en
    // `fitz build` con "el tipo `User` no tiene un método llamado
    // `greet`". Las methods del tipo no se copiaban al type_methods
    // del importer. Post-CM: dispatch funciona bit-a-bit.
    let main = "from utils import User\n\
                let u: User = User { id: 1, name: \"Ada\" }\n\
                print(u.greet())\n\
                let admin: User = User.admin()\n\
                print(admin.greet())\n";
    let utils = "type User {\n\
                     id: Int = 0\n\
                     name: Str = \"anon\"\n\
                     fn greet() -> Str { return \"hola, {name}\" }\n\
                     static fn admin() -> User { return User { id: 0, name: \"admin\" } }\n\
                 }\n";
    let (stdout, exit) =
        build_and_run_multi("cm-cross-module-methods", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Ada", "hola, admin"]);
}

// ---- Mini-tanda Vm — métodos privados (`_method`) en `type` ----

#[test]
fn vm_metodo_publico_compila() {
    // Sanity: un método público sigue funcionando con el filter de
    // private methods activo.
    let src = "type C {\n\
                   x: Int = 0\n\
                   fn show() -> Int { return x }\n\
               }\n\
               let c: C = C { x: 42 }\n\
               print(c.show())\n";
    let (stdout, exit) = build_and_run("vm_publico_ok", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

// ---- Mini-tanda Lx — List.any/all/count/find_index ----

#[test]
fn lx_any_y_all_bit_a_bit() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               print(xs.any(fn(x: Int) => x > 3))\n\
               print(xs.any(fn(x: Int) => x > 10))\n\
               print(xs.all(fn(x: Int) => x > 0))\n\
               print(xs.all(fn(x: Int) => x > 2))\n";
    let (stdout, exit) = build_and_run("lx_any_all", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse\ntrue\nfalse");
}

#[test]
fn lx_count_y_find_index_bit_a_bit() {
    let src = "let xs: List<Int> = [10, 20, 30, 40]\n\
               print(xs.count(fn(x: Int) => x > 15))\n\
               match xs.find_index(fn(x: Int) => x == 30) {\n\
                 Ok(i) => print(\"idx: {i}\"),\n\
                 Err(_) => print(\"missing\")\n\
               }\n\
               match xs.find_index(fn(x: Int) => x > 100) {\n\
                 Ok(i) => print(\"idx: {i}\"),\n\
                 Err(_) => print(\"missing\")\n\
               }\n";
    let (stdout, exit) = build_and_run("lx_count_find_index", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3\nidx: 2\nmissing");
}

#[test]
fn lx_lista_vacia_any_false_all_true() {
    let src = "let empty: List<Int> = []\n\
               print(empty.any(fn(x: Int) => true))\n\
               print(empty.all(fn(x: Int) => false))\n\
               print(empty.count(fn(x: Int) => true))\n";
    let (stdout, exit) = build_and_run("lx_empty", src);
    assert_eq!(exit, 0);
    // any vacía → false, all vacía → true (vacuous truth), count → 0.
    assert_eq!(stdout.trim(), "false\ntrue\n0");
}

// ---- Mini-tanda St — métodos estáticos en `type` ----

#[test]
fn st_static_methods_constructores_y_factories() {
    // `static fn zero()` y `static fn of(n)` como constructores.
    let src = "type C {\n\
                   value: Int = 0\n\n\
                   static fn zero() -> C { return C { value: 0 } }\n\
                   static fn of(n: Int) -> C { return C { value: n } }\n\
               }\n\
               let z: C = C.zero()\n\
               let c: C = C.of(42)\n\
               print(z)\n\
               print(c)\n";
    let (stdout, exit) = build_and_run("st_static_constructors", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "C { value: 0 }\nC { value: 42 }");
}

#[test]
fn st_static_e_instance_methods_coexisten() {
    // El mismo `type` puede tener ambos tipos de método. Test que
    // static + instance no se pisan.
    let src = "type C {\n\
                   value: Int = 0\n\n\
                   static fn make(n: Int) -> C { return C { value: n } }\n\n\
                   fn double() -> C { return C { value: value * 2 } }\n\
               }\n\
               let c: C = C.make(7)\n\
               let d: C = c.double()\n\
               print(c)\n\
               print(d)\n";
    let (stdout, exit) = build_and_run("st_static_y_instance", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "C { value: 7 }\nC { value: 14 }");
}

// ---- Mini-tanda F8 — identificadores no-ASCII (Unicode) ----

#[test]
fn f8_identifiers_unicode_compilan_y_corren() {
    // Letras griegas, acentos, ñ, CJK, cirílico. is_alphabetic /
    // is_alphanumeric del lexer ya los acepta; Rust permite Unicode
    // identifiers desde edition 2021 — paso transparente.
    let src = "let π: Float = 3.14159\n\
               let función: Str = \"hola\"\n\
               let café: Int = 42\n\
               let 名前: Str = \"Fitz\"\n\
               let имя: Str = \"Roy\"\n\
               print(π)\n\
               print(función)\n\
               print(café)\n\
               print(名前)\n\
               print(имя)\n";
    let (stdout, exit) = build_and_run("f8_idents_unicode", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3.14159\nhola\n42\nFitz\nRoy");
}

#[test]
fn f8_fn_y_params_con_identifiers_unicode() {
    // `fn` con nombre Unicode + params con nombre Unicode.
    let src = "fn niño(edad: Int) -> Str => \"niño de {edad}\"\n\
               print(niño(5))\n";
    let (stdout, exit) = build_and_run("f8_fn_unicode", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "niño de 5");
}

// ---- Mini-tanda Xor — operador `xor` lógico sobre Bool ----

#[test]
fn xor_tabla_de_verdad_bit_a_bit() {
    let src = "print(true xor true)\n\
               print(true xor false)\n\
               print(false xor true)\n\
               print(false xor false)\n";
    let (stdout, exit) = build_and_run("xor_tabla", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "false\ntrue\ntrue\nfalse");
}

#[test]
fn xor_chain_misma_precedencia_que_or() {
    // `true xor true xor true` left-assoc → ((T xor T) xor T) = (F xor T) = T.
    let src = "print(true xor true xor true)\n\
               print(true xor false xor true)\n";
    let (stdout, exit) = build_and_run("xor_chain", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

#[test]
fn xor_combina_con_and_y_or() {
    // `a and b xor c` → `(a and b) xor c`. And precedencia mayor.
    let src = "let a: Bool = true\n\
               let b: Bool = false\n\
               let c: Bool = true\n\
               print(a and b xor c)\n";
    let (stdout, exit) = build_and_run("xor_and_or", src);
    assert_eq!(exit, 0);
    // (true and false) xor true = false xor true = true.
    assert_eq!(stdout.trim(), "true");
}

// ---- Mini-tanda Mln — from import multi-línea con paréntesis ----

#[test]
fn mln_from_import_parens_multi_linea_compila_y_corre() {
    // Caso canónico: importar varios items en forma multi-línea con
    // paréntesis. Cada item en su línea, trailing comma opcional.
    let main = "from utils import (\n\
                    greet,\n\
                    shout,\n\
                    User,\n\
                )\n\
                print(greet(\"Mln\"))\n\
                print(shout(\"Mln\"))\n\
                print(User { id: 1, name: \"Fitz\" })\n";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"\n\
                 fn shout(s: Str) -> Str => s.upper()\n\
                 type User { id: Int = 0, name: Str = \"anon\" }\n";
    let (stdout, exit) =
        build_and_run_multi("mln-parens-multilinea", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["hola, Mln", "MLN", "User { id: 1, name: \"Fitz\" }"],
    );
}

#[test]
fn mln_from_import_parens_con_aliases_mixtos_compila() {
    // Aliases dentro de los paréntesis funcionan igual que en
    // single-line; el binding local se hace bajo el alias.
    let main = "from utils import (\n\
                    greet,\n\
                    shout as scream,\n\
                    User as Persona,\n\
                )\n\
                print(greet(\"Mln\"))\n\
                print(scream(\"Mln\"))\n\
                print(Persona { id: 1, name: \"Fitz\" })\n";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"\n\
                 fn shout(s: Str) -> Str => s.upper()\n\
                 type User { id: Int = 0, name: Str = \"anon\" }\n";
    let (stdout, exit) = build_and_run_multi("mln-parens-aliases", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "hola, Mln",
            "MLN",
            // Aliases locales: pero el Display del struct usa el
            // type_name canónico ("User"), no el alias ("Persona") —
            // paridad bit-a-bit con el evaluator (PreF8.4).
            "User { id: 1, name: \"Fitz\" }",
        ],
    );
}

// ---- Mini-tanda El — Err(List<T>) / Err(Map<K,V>) en codegen ----

#[test]
fn el_err_list_compila_y_preserva_value() {
    // Err(List<Int>): el binding `Err(xs)` tipa con List<Int>, así
    // métodos como `.len()` funcionan sobre el value.
    let src = "fn fail() -> Result<Int, List<Int>> {\n\
                 return Err([1, 2, 3])\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(xs) => print(\"err con {xs.len()} items\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_list", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err con 3 items");
}

#[test]
fn el_err_list_print_directo_matches_interprete() {
    // Print directo del Err con List preserva el formato canónico
    // (matchea el evaluator).
    let src = "fn fail() -> Result<Int, List<Int>> {\n\
                 return Err([10, 20, 30])\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok\"),\n\
                 Err(xs) => print(\"{xs}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_list_print", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 30]");
}

#[test]
fn el_err_map_compila_y_preserva_value() {
    // Err(Map<Str, Int>): el binding `Err(m)` tipa con Map<Str, Int>,
    // así `.len()` y `.has(k)` funcionan sobre el value.
    let src = "fn fail() -> Result<Int, Map<Str, Int>> {\n\
                 return Err({\"a\": 1, \"b\": 2})\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(m) => print(\"err size: {m.len()}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err size: 2");
}

#[test]
fn el_err_propagation_con_list_via_try_operator() {
    // `?` propaga el Err(List<T>) intacto.
    let src = "fn inner() -> Result<Int, List<Int>> {\n\
                 return Err([1, 2])\n\
               }\n\
               fn outer() -> Result<Int, List<Int>> {\n\
                 let v = inner()?\n\
                 return Ok(v)\n\
               }\n\
               match outer() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(xs) => print(\"pipe err: {xs.len()}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_list_try", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "pipe err: 2");
}

// ---- Mini-tanda Ir — iteradores sobre Range ----

#[test]
fn ir_range_enumerate_compila_y_corre() {
    let src = "for (i, n) in (0..3).enumerate() {\n\
                 print(\"{i}-{n}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ir_range_enumerate", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0-0\n1-1\n2-2");
}

#[test]
fn ir_range_zip_con_list_str_trunca() {
    let src = "let nombres: List<Str> = [\"ada\", \"bea\"]\n\
               for (i, n) in (1..100).zip(nombres) {\n\
                 print(\"{i}-{n}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ir_range_zip", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1-ada\n2-bea");
}

#[test]
fn ir_range_chain_con_list_int_concatena() {
    let src = "let extra: List<Int> = [100, 200]\n\
               let combo: List<Int> = (0..3).chain(extra)\n\
               print(combo)\n";
    let (stdout, exit) = build_and_run("ir_range_chain", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[0, 1, 2, 100, 200]");
}

#[test]
fn ir_range_len_exclusivo_e_inclusivo_compila() {
    let src = "print((10..20).len())\n\
               print((10..=20).len())\n";
    let (stdout, exit) = build_and_run("ir_range_len", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "10\n11");
}

// ---- Mini-tanda Mb — trim_start/trim_end + flatten + sort_by ----

#[test]
fn mb_str_trim_start_y_end_compilan() {
    let src = "print(\"  hola  \".trim_start())\n\
               print(\"  hola  \".trim_end())\n\
               print(\"\\n\\tlinea\\n\\t\".trim_start())\n\
               print(\"\\n\\tlinea\\n\\t\".trim_end())\n";
    let (stdout, exit) = build_and_run("mb_trim_start_end", src);
    assert_eq!(exit, 0);
    // trim_start: recorta inicio, deja el sufijo intacto.
    // trim_end: recorta final, deja el prefijo intacto.
    assert_eq!(stdout, "hola  \n  hola\nlinea\n\t\n\n\tlinea\n");
}

#[test]
fn mb_list_flatten_concatena_sublistas() {
    let src = "let xss: List<List<Int>> = [[1, 2], [3], [4, 5, 6]]\n\
               let flat: List<Int> = xss.flatten()\n\
               print(flat)\n\
               print(flat.len())\n";
    let (stdout, exit) = build_and_run("mb_list_flatten", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3, 4, 5, 6]\n6");
}

#[test]
fn mb_list_flatten_de_listas_vacias_es_vacio() {
    let src = "let xss: List<List<Int>> = [[], [], []]\n\
               let flat: List<Int> = xss.flatten()\n\
               print(flat)\n\
               print(flat.len())\n";
    let (stdout, exit) = build_and_run("mb_list_flatten_empty", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[]\n0");
}

#[test]
fn mb_list_sort_by_ascendente_y_descendente() {
    let src = "let xs: List<Int> = [3, 1, 4, 1, 5, 9, 2, 6]\n\
               xs.sort_by(fn(a: Int, b: Int) => a - b)\n\
               print(xs)\n\
               let ys: List<Int> = [3, 1, 4]\n\
               ys.sort_by(fn(a: Int, b: Int) => b - a)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("mb_list_sort_by", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 1, 2, 3, 4, 5, 6, 9]\n[4, 3, 1]");
}

#[test]
fn mb_list_sort_by_comparator_compuesto() {
    // Ordenar por valor absoluto (caso típico).
    let src = "fn abs_diff(a: Int, b: Int) -> Int {\n\
                   let aa: Int = if a < 0 { 0 - a } else { a }\n\
                   let bb: Int = if b < 0 { 0 - b } else { b }\n\
                   return aa - bb\n\
               }\n\
               let xs: List<Int> = [-3, 1, -4, 1, 5, -9, 2, 6]\n\
               xs.sort_by(fn(a: Int, b: Int) => abs_diff(a, b))\n\
               print(xs)\n";
    let (stdout, exit) = build_and_run("mb_list_sort_by_abs", src);
    assert_eq!(exit, 0);
    // Por valor absoluto ascendente: |1|=|1|=1, |2|=2, |-3|=3, |-4|=4,
    // |5|=5, |6|=6, |-9|=9 → orden estable: [1, 1, 2, -3, -4, 5, 6, -9]
    assert_eq!(stdout.trim(), "[1, 1, 2, -3, -4, 5, 6, -9]");
}

// ---- Mini-tanda Lt — let-destructure con sub-patterns ricos ----

#[test]
fn lt_let_literal_int_subpattern_compila_y_corre() {
    // `let (1, x) = (1, 42)` — literal Int como guard del primer slot.
    let src = "let (1, x) = (1, 42)\nprint(x)\n";
    let (stdout, exit) = build_and_run("lt_let_int_lit", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn lt_let_str_literal_subpattern_compila_y_corre() {
    // `let ("ada", n) = ("ada", 7)` — Str literal genera guard
    // `__s_X.as_str() == "ada"`.
    let src = "let (\"ada\", n) = (\"ada\", 7)\nprint(n)\n";
    let (stdout, exit) = build_and_run("lt_let_str_lit", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "7");
}

#[test]
fn lt_let_range_subpattern_compila_y_corre() {
    // `let (0..100, y) = (50, "yes")` — Range emite guard
    // `(0..100).contains(&__n_X)`.
    let src = "let (0..100, y) = (50, \"yes\")\nprint(y)\n";
    let (stdout, exit) = build_and_run("lt_let_range", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "yes");
}

#[test]
fn lt_let_ok_binding_extrae_resultado() {
    // `let (Ok(v), tag) = (Ok(99), "result")` — desempaca Result
    // dentro de tuple. Bindings: v=99, tag="result".
    let src = "let (Ok(v), tag) = (Ok(99), \"result\")\nprint(v)\nprint(tag)\n";
    let (stdout, exit) = build_and_run("lt_let_ok_binding", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "99\nresult");
}

#[test]
fn lt_let_panic_si_no_matchea() {
    // `let (1, x) = (2, 42)`: el 2 NO matchea el 1 → panic en runtime.
    // El binario debe terminar con exit code != 0.
    let src = "let (1, x) = (2, 42)\nprint(x)\n";
    let (_stdout, exit) = build_and_run("lt_let_panic", src);
    assert_ne!(exit, 0, "esperaba exit code != 0 por panic, fue: 0");
}

// ---- Mini-tanda F9 — escapes extendidos en strings ----

#[test]
fn f9_escapes_extendidos_paridad_bit_a_bit() {
    // `\u{...}` Unicode (BMP + suplementario), `\x..` ASCII hex, `\0`,
    // `\b`. El lexer produce un Token::Str con los chars resueltos,
    // así que el codegen no necesita lógica extra — `rust_str_literal`
    // emite el literal Rust correcto vía `format!("{:?}", s)`.
    let src = "let cafe: Str = \"caf\\u{00E9}\"\n\
               let snow: Str = \"\\u{2603}\"\n\
               let a: Str = \"\\x41-\\x7F\"\n\
               let nul: Str = \"x\\0y\"\n\
               print(cafe)\n\
               print(snow)\n\
               print(a)\n\
               print(nul.len())\n";
    let (stdout, exit) = build_and_run("f9_escapes_extendidos", src);
    assert_eq!(exit, 0);
    // café, ☃, A-<DEL>, nul.len() = 3 chars (x + NUL + y).
    assert_eq!(stdout.trim(), "café\n☃\nA-\u{007F}\n3");
}

// ---- Mini-tanda Mb2 + Rg ---------------------------------------
// Métodos chicos: List.min/max/sum, Str.pad_start/pad_end,
// Map.keys_sorted, Range.step_by. Cada test valida que `fitz build`
// produce un binario standalone con output bit-a-bit idéntico al
// intérprete (`fitz run`).

#[test]
fn mb2_list_min_max_sum_int_compila() {
    let src = "let xs: List<Int> = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5]\n\
               match xs.min() {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(_) => print(\"vacía\")\n\
               }\n\
               match xs.max() {\n\
                 Ok(v) => print(\"max: {v}\"),\n\
                 Err(_) => print(\"vacía\")\n\
               }\n\
               let total: Int = xs.sum()\n\
               print(\"sum: {total}\")\n";
    let (stdout, exit) = build_and_run("mb2_list_min_max_sum_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "min: 1\nmax: 9\nsum: 44");
}

#[test]
fn mb2_list_min_max_sum_float_compila() {
    let src = "let xs: List<Float> = [1.5, 0.5, 2.25, 1.0]\n\
               match xs.min() {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(_) => print(\"vacía\")\n\
               }\n\
               let total: Float = xs.sum()\n\
               print(\"sum: {total}\")\n";
    let (stdout, exit) = build_and_run("mb2_list_min_max_sum_float", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "min: 0.5\nsum: 5.25");
}

#[test]
fn mb2_list_min_vacia_devuelve_err_paridad() {
    let src = "let xs: List<Int> = []\n\
               match xs.min() {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n\
               let total: Int = xs.sum()\n\
               print(\"sum: {total}\")\n";
    let (stdout, exit) = build_and_run("mb2_list_min_vacia_err", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: lista vacía\nsum: 0");
}

#[test]
fn mb2_str_pad_start_end_compilan() {
    let src = "let s: Str = \"42\"\n\
               let a: Str = s.pad_start(5, \"0\")\n\
               let b: Str = s.pad_end(5, \".\")\n\
               let c: Str = \"hello\".pad_start(3, \"*\")\n\
               print(a)\n\
               print(b)\n\
               print(c)\n";
    let (stdout, exit) = build_and_run("mb2_str_pad", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "00042\n42...\nhello");
}

#[test]
fn mb2_map_keys_sorted_str_compila() {
    let src = "let m: Map<Str, Int> = {\"banana\": 2, \"apple\": 1, \"cherry\": 3}\n\
               let ks: List<Str> = m.keys_sorted()\n\
               print(ks)\n";
    let (stdout, exit) = build_and_run("mb2_map_keys_sorted_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[\"apple\", \"banana\", \"cherry\"]");
}

#[test]
fn mb2_map_keys_sorted_int_compila() {
    let src = "let m: Map<Int, Str> = {30: \"c\", 10: \"a\", 20: \"b\"}\n\
               let ks: List<Int> = m.keys_sorted()\n\
               print(ks)\n";
    let (stdout, exit) = build_and_run("mb2_map_keys_sorted_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 30]");
}

#[test]
fn rg_range_step_by_exclusivo_compila() {
    let src = "let xs: List<Int> = (0..10).step_by(2)\n\
               print(xs)\n\
               let total: Int = xs.sum()\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("rg_range_step_by_excl", src);
    assert_eq!(exit, 0);
    // 0..10 step 2 → [0, 2, 4, 6, 8], sum = 20.
    assert_eq!(stdout.trim(), "[0, 2, 4, 6, 8]\n20");
}

#[test]
fn rg_range_step_by_inclusivo_compila() {
    let src = "let xs: List<Int> = (0..=10).step_by(3)\n\
               print(xs)\n";
    let (stdout, exit) = build_and_run("rg_range_step_by_incl", src);
    assert_eq!(exit, 0);
    // 0..=10 step 3 → [0, 3, 6, 9].
    assert_eq!(stdout.trim(), "[0, 3, 6, 9]");
}

// ---- Mini-tanda Mb3 — fold + product + chars + entries + to_map ----

#[test]
fn mb3_list_reduce_sum_int_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let total: Int = xs.reduce(0, fn(acc: Int, x: Int) => acc + x)\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("mb3_list_reduce_sum", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "15");
}

#[test]
fn mb3_list_reduce_acc_distinto_de_t_compila() {
    // Acc puede ser de un tipo distinto al de los elementos.
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let s: Str = xs.reduce(\"\", fn(acc: Str, x: Int) => \"{acc}{x}-\")\n\
               print(s)\n";
    let (stdout, exit) = build_and_run("mb3_list_reduce_acc_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1-2-3-");
}

#[test]
fn mb3_list_product_compila() {
    let src = "let xs: List<Int> = [2, 3, 4]\n\
               let p: Int = xs.product()\n\
               print(p)\n\
               let empty: List<Int> = []\n\
               print(empty.product())\n";
    let (stdout, exit) = build_and_run("mb3_list_product", src);
    assert_eq!(exit, 0);
    // 2*3*4=24, vacío → 1 (sentinel).
    assert_eq!(stdout.trim(), "24\n1");
}

#[test]
fn mb3_str_chars_compila() {
    let src = "let cs: List<Str> = \"abc\".chars()\n\
               print(cs)\n\
               print(cs.len())\n";
    let (stdout, exit) = build_and_run("mb3_str_chars", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[\"a\", \"b\", \"c\"]\n3");
}

#[test]
fn mb3_map_entries_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}\n\
               let es: List<(Str, Int)> = m.entries()\n\
               print(es)\n\
               print(es.len())\n";
    let (stdout, exit) = build_and_run("mb3_map_entries", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[(\"a\", 1), (\"b\", 2)]\n2");
}

#[test]
fn mb3_list_to_map_compila() {
    let src = "let pairs: List<(Str, Int)> = [(\"a\", 1), (\"b\", 2)]\n\
               let m: Map<Str, Int> = pairs.to_map()\n\
               print(m[\"a\"])\n\
               print(m[\"b\"])\n\
               print(m.len())\n";
    let (stdout, exit) = build_and_run("mb3_list_to_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2\n2");
}

// ---- Mini-tanda Cd — codegen polish: higher-order + F12 ----

#[test]
fn cd_ho_map_con_fn_nombrada_compila() {
    let src = "fn double(n: Int) -> Int { return n * 2 }\n\
               let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = xs.map(double)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("cd_ho_map_named", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]");
}

#[test]
fn cd_ho_filter_con_fn_nombrada_compila() {
    let src = "fn is_even(n: Int) -> Bool { return n % 2 == 0 }\n\
               let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let ys: List<Int> = xs.filter(is_even)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("cd_ho_filter_named", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4]");
}

#[test]
fn cd_ho_reduce_binary_con_fn_nombrada_compila() {
    let src = "fn sumar(acc: Int, x: Int) -> Int { return acc + x }\n\
               let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let total: Int = xs.reduce(0, sumar)\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("cd_ho_reduce_named", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "15");
}

#[test]
fn cd_f12_let_int_const_referenciado_por_fn_compila() {
    let src = "let MAX = 100\n\
               fn cap(n: Int) -> Int {\n\
                   if (n > MAX) { return MAX }\n\
                   return n\n\
               }\n\
               print(cap(50))\n\
               print(cap(200))\n";
    let (stdout, exit) = build_and_run("cd_f12_let_int_const", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "50\n100");
}

#[test]
fn cd_f12_let_str_compila() {
    let src = "let GREETING = \"hola\"\n\
               fn greet(name: Str) -> Str { return \"{GREETING}, {name}\" }\n\
               print(greet(\"Ada\"))\n";
    let (stdout, exit) = build_and_run("cd_f12_let_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola, Ada");
}

#[test]
fn cd_f12_let_float_compila() {
    let src = "let PI = 3.14\n\
               fn area(r: Float) -> Float { return PI * r * r }\n\
               print(area(2.0))\n";
    let (stdout, exit) = build_and_run("cd_f12_let_float", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "12.56");
}

#[test]
fn cd_f12_const_eval_con_binop_compila() {
    let src = "let LIMIT = 10 * 2 + 5\n\
               fn check(n: Int) -> Bool { return n < LIMIT }\n\
               print(check(20))\n\
               print(check(30))\n";
    let (stdout, exit) = build_and_run("cd_f12_const_eval_binop", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

// ---- Mini-tanda Mb4 + Cmp+ ----------------------------------

#[test]
fn mb4_list_unique_compila() {
    let src = "let xs: List<Int> = [1, 2, 2, 3, 1, 4, 3]\n\
               let r: List<Int> = xs.unique()\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("mb4_list_unique", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3, 4]");
}

#[test]
fn mb4_list_partition_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5, 6]\n\
               let split: (List<Int>, List<Int>) = xs.partition(fn(n: Int) => n % 2 == 0)\n\
               print(split.0)\n\
               print(split.1)\n";
    let (stdout, exit) = build_and_run("mb4_list_partition", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]\n[1, 3, 5]");
}

#[test]
fn mb4_map_invert_compila() {
    let src = "let m: Map<Int, Str> = {1: \"a\", 2: \"b\"}\n\
               let inv: Map<Str, Int> = m.invert()\n\
               print(inv[\"a\"])\n\
               print(inv[\"b\"])\n";
    let (stdout, exit) = build_and_run("mb4_map_invert", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2");
}

#[test]
fn mb4_str_split_at_compila() {
    let src = "let p: (Str, Str) = \"hola mundo\".split_at(4)\n\
               print(p.0)\n\
               print(p.1)\n";
    let (stdout, exit) = build_and_run("mb4_str_split_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola\n mundo");
}

#[test]
fn cmp_multi_for_clauses_compila() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20]\n\
               let r: List<Int> = [x + y for x in xs for y in ys]\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("cmp_multi_for", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[11, 21, 12, 22, 13, 23]");
}

#[test]
fn cmp_multi_for_con_filter_compila() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20]\n\
               let r: List<Int> = [x * y for x in xs for y in ys if x % 2 == 1]\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("cmp_multi_for_filter", src);
    assert_eq!(exit, 0);
    // x impar (1, 3): (1*10, 1*20, 3*10, 3*20) = [10, 20, 30, 60].
    assert_eq!(stdout.trim(), "[10, 20, 30, 60]");
}

#[test]
fn cmp_map_comp_basico_compila() {
    let src = "let squares: Map<Int, Int> = {n: n * n for n in 1..=4}\n\
               print(squares[1])\n\
               print(squares[2])\n\
               print(squares[4])\n";
    let (stdout, exit) = build_and_run("cmp_map_comp", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n4\n16");
}

#[test]
fn cmp_map_comp_con_filter_compila() {
    let src = "let big: Map<Int, Int> = {n: n * 10 for n in 0..10 if n > 5}\n\
               print(big[6])\n\
               print(big[9])\n";
    let (stdout, exit) = build_and_run("cmp_map_comp_filter", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "60\n90");
}

// ---- Mini-tanda Mb5 + Async-cl --------------------------------

#[test]
fn mb5_list_group_by_compila() {
    let src = "let nums: List<Int> = [1, 2, 3, 4, 5, 6]\n\
               let g: Map<Str, List<Int>> = nums.group_by(fn(n: Int) => if (n % 2 == 0) { \"par\" } else { \"impar\" })\n\
               print(g[\"par\"])\n\
               print(g[\"impar\"])\n";
    let (stdout, exit) = build_and_run("mb5_list_group_by", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]\n[1, 3, 5]");
}

#[test]
fn mb5_list_zip_with_compila() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20, 30]\n\
               let r: List<Int> = xs.zip_with(ys, fn(a: Int, b: Int) => a + b)\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("mb5_list_zip_with", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[11, 22, 33]");
}

#[test]
fn mb5_list_max_by_compila() {
    let src = "type P { age: Int = 0 name: Str = \"\" }\n\
               let ps: List<P> = [P { age: 28, name: \"Bob\" }, P { age: 42, name: \"Cam\" }, P { age: 35, name: \"Ada\" }]\n\
               match ps.max_by(fn(p: P) => p.age) {\n\
                 Ok(p) => print(\"mayor: {p.name}\"),\n\
                 Err(_) => print(\"vacío\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mb5_list_max_by", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "mayor: Cam");
}

#[test]
fn mb5_list_min_by_lista_vacia_devuelve_err_compila() {
    let src = "let xs: List<Int> = []\n\
               match xs.min_by(fn(n: Int) => n) {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mb5_list_min_by_vacia", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: lista vacía");
}

#[test]
fn mb5_str_lines_compila() {
    let src = "let s = \"uno\\ndos\\ntres\"\n\
               let ls: List<Str> = s.lines()\n\
               print(ls)\n\
               print(ls.len())\n";
    let (stdout, exit) = build_and_run("mb5_str_lines", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[\"uno\", \"dos\", \"tres\"]\n3");
}

#[test]
fn mb5_str_is_empty_compila() {
    let src = "print(\"\".is_empty())\n\
               print(\"hola\".is_empty())\n";
    let (stdout, exit) = build_and_run("mb5_str_is_empty", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

// ---- Mini-tanda Mb8 + Bits-extras + Fmt-g ------------------

#[test]
fn mb8_list_starts_ends_with_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               print(xs.starts_with([1, 2]))\n\
               print(xs.ends_with([4, 5]))\n\
               print(xs.starts_with([1, 3]))\n";
    let (stdout, exit) = build_and_run("mb8_starts_ends", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\ntrue\nfalse");
}

#[test]
fn mb8_list_insert_at_compila() {
    let src = "let xs: List<Int> = [1, 2, 4, 5]\n\
               print(xs.insert_at(2, 3))\n";
    let (stdout, exit) = build_and_run("mb8_insert_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3, 4, 5]");
}

#[test]
fn mb8_list_remove_at_compila() {
    let src = "let xs: List<Int> = [10, 20, 30, 40]\n\
               print(xs.remove_at(2))\n";
    let (stdout, exit) = build_and_run("mb8_remove_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 40]");
}

#[test]
fn mb8_list_zip_to_map_compila() {
    let src = "let ks: List<Str> = [\"a\", \"b\"]\n\
               let vs: List<Int> = [1, 2]\n\
               let m: Map<Str, Int> = ks.zip_to_map(vs)\n\
               print(m[\"a\"])\n\
               print(m[\"b\"])\n";
    let (stdout, exit) = build_and_run("mb8_zip_to_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2");
}

#[test]
fn mb8_str_left_right_center_compila() {
    let src = "let s = \"hola mundo\"\n\
               print(s.left(4))\n\
               print(s.right(5))\n\
               print(s.center(20, \"-\"))\n";
    let (stdout, exit) = build_and_run("mb8_str_left_right_center", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola\nmundo\n-----hola mundo-----");
}

#[test]
fn bits_extras_popcount_y_leading_zeros_compila() {
    let src = "print(popcount(7))\n\
               print(popcount(255))\n\
               print(leading_zeros(1))\n\
               print(trailing_zeros(8))\n";
    let (stdout, exit) = build_and_run("bits_extras_popcount_lz", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3\n8\n63\n3");
}

#[test]
fn bits_extras_rotate_compila() {
    let src = "print(rotate_left(1, 4))\n\
               print(rotate_right(16, 4))\n";
    let (stdout, exit) = build_and_run("bits_extras_rotate", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "16\n1");
}

#[test]
fn fmt_g_general_compila() {
    let src = "let x = 1234.5\n\
               print(\"{x:g}\")\n\
               let y = 0.00001\n\
               print(\"{y:g}\")\n\
               let z = 3.140000\n\
               print(\"{z:g}\")\n";
    let (stdout, exit) = build_and_run("fmt_g_general", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1234.5\n1.00000e-5\n3.14");
}

#[test]
fn fmt_g_uppercase_compila() {
    let src = "let w = 1234567890.0\n\
               print(\"{w:G}\")\n";
    let (stdout, exit) = build_and_run("fmt_G_uppercase", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1.23457E9");
}

// ---- Mini-tanda Mb7 — take/drop/init/tail/intersperse/cycle +
//                      repeat_with + with ----------

#[test]
fn mb7_list_take_drop_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               print(xs.take(3))\n\
               print(xs.drop(2))\n\
               print(xs.take(99))\n\
               print(xs.drop(99))\n";
    let (stdout, exit) = build_and_run("mb7_take_drop", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3]\n[3, 4, 5]\n[1, 2, 3, 4, 5]\n[]");
}

#[test]
fn mb7_list_init_tail_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4]\n\
               print(xs.init())\n\
               print(xs.tail())\n";
    let (stdout, exit) = build_and_run("mb7_init_tail", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3]\n[2, 3, 4]");
}

#[test]
fn mb7_list_intersperse_compila() {
    let src = "let xs: List<Int> = [10, 20, 30]\n\
               print(xs.intersperse(0))\n";
    let (stdout, exit) = build_and_run("mb7_intersperse", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 0, 20, 0, 30]");
}

#[test]
fn mb7_list_cycle_compila() {
    let src = "let xs: List<Int> = [1, 2]\n\
               print(xs.cycle(3))\n";
    let (stdout, exit) = build_and_run("mb7_cycle", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 1, 2, 1, 2]");
}

#[test]
fn mb7_str_repeat_with_compila() {
    let src = "print(\"hi\".repeat_with(3, \"-\"))\n\
               print(\"x\".repeat_with(0, \",\"))\n";
    let (stdout, exit) = build_and_run("mb7_repeat_with", src);
    assert_eq!(exit, 0);
    // Segunda línea es string vacío (n=0).
    assert_eq!(stdout, "hi-hi-hi\n\n");
}

#[test]
fn mb7_map_with_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1}\n\
               let m2: Map<Str, Int> = m.with(\"b\", 2)\n\
               print(m2[\"a\"])\n\
               print(m2[\"b\"])\n\
               let m3: Map<Str, Int> = m.with(\"a\", 99)\n\
               print(m3[\"a\"])\n";
    let (stdout, exit) = build_and_run("mb7_with", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2\n99");
}

// ---- Mini-tanda Fmt-build — format specs faltantes ----

#[test]
fn fmt_build_grouping_coma_compila() {
    let src = "let n = 1234567\n\
               print(\"{n:,d}\")\n";
    let (stdout, exit) = build_and_run("fmt_grouping_coma", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1,234,567");
}

#[test]
fn fmt_build_grouping_underscore_compila() {
    let src = "let n = 1000000\n\
               print(\"{n:_d}\")\n";
    let (stdout, exit) = build_and_run("fmt_grouping_underscore", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1_000_000");
}

#[test]
fn fmt_build_percent_compila() {
    let src = "let r = 0.857\n\
               print(\"{r:.2%}\")\n";
    let (stdout, exit) = build_and_run("fmt_percent", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "85.70%");
}

#[test]
fn fmt_build_char_codepoint_compila() {
    let src = "let cp = 65\n\
               print(\"{cp:c}\")\n";
    let (stdout, exit) = build_and_run("fmt_char_codepoint", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "A");
}

#[test]
fn fmt_build_grouping_negativo_compila() {
    let src = "let n = -1234\n\
               print(\"{n:,d}\")\n";
    let (stdout, exit) = build_and_run("fmt_grouping_neg", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "-1,234");
}

// ---- Mini-tanda Mb6 — scan + windows + merge_with --------

#[test]
fn mb6_list_scan_acumula_outputs_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4]\n\
               let r: List<Int> = xs.scan(0, fn(acc: Int, x: Int) => acc + x)\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("mb6_scan", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 3, 6, 10]");
}

#[test]
fn mb6_list_windows_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let r: List<List<Int>> = xs.windows(3)\n\
               print(r)\n\
               print(r.len())\n";
    let (stdout, exit) = build_and_run("mb6_windows", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[[1, 2, 3], [2, 3, 4], [3, 4, 5]]\n3");
}

#[test]
fn mb6_map_merge_with_callback_resuelve_conflicts_compila() {
    let src = "let a: Map<Str, Int> = {\"x\": 1, \"y\": 2}\n\
               let b: Map<Str, Int> = {\"y\": 10, \"z\": 3}\n\
               let r: Map<Str, Int> = a.merge_with(b, fn(va: Int, vb: Int) => va + vb)\n\
               print(r[\"x\"])\n\
               print(r[\"y\"])\n\
               print(r[\"z\"])\n";
    let (stdout, exit) = build_and_run("mb6_merge_with", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n12\n3");
}

// ---- Mini-tanda HTTP-Cors — echo Origin sin filtro --------

#[test]
fn http_cors_echo_origin_hace_echo_sin_filtro() {
    // allow_origin: "echo" (Str literal) → echo del Origin recibido
    // sin filtro. Cualquier Origin que llegue se eco-emite en la
    // response. Si no llega Origin header, no se emite el header.
    let src = "\
@server(43392)
fn main() => 0

@middleware(cors({\"allow_origin\": \"echo\"}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    // Request CON Origin → echo en response.
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "http-cors-echo-with-origin",
        src,
        43392,
        "GET",
        "/api",
        &[("Origin", "https://anything.example.com")],
    );
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        lower.contains("access-control-allow-origin: https://anything.example.com"),
        "headers no contienen echo del Origin: {}",
        raw_headers
    );
}

#[test]
fn http_cors_echo_sin_origin_no_emite_header() {
    let src = "\
@server(43393)
fn main() => 0

@middleware(cors({\"allow_origin\": \"echo\"}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    // Request SIN Origin → no se emite header.
    let (status, raw_headers) =
        build_spawn_request_raw("http-cors-echo-no-origin", src, 43393, "GET", "/api");
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        !lower.contains("access-control-allow-origin:"),
        "Echo sin Origin no debería emitir el header: {}",
        raw_headers
    );
}

// ---- Mini-tanda HTTP-Err — status codes específicos por Err ----

#[test]
fn http_err_instance_con_status_field_devuelve_ese_status() {
    // Convención: Err con Instance con field `status: Int` → ese
    // status code se usa en la response. Body = Instance serializada.
    let src = "\
type ApiErr {
    status: Int = 500
    message: Str = \"\"
}

@server(43390)
fn main() => 0

@get(\"/users/{id}\")
fn get_user(id: Int) -> Result<Str, ApiErr> {
    if (id == 0) { return Err(ApiErr { status: 404, message: \"no encontrado\" }) }
    return Ok(\"Ada\")
}
";
    let (status, body) = build_spawn_request("http-err-404", src, 43390, "GET", "/users/0", None);
    assert_eq!(status, 404);
    assert!(body.contains("no encontrado"), "body fue: {}", body);
    // El body es el Instance serializado (no envuelto en `{"error": ...}`).
    assert!(body.contains("\"status\":404"), "body fue: {}", body);
}

#[test]
fn http_err_instance_con_status_field_400_y_ok_path() {
    let src = "\
type ApiErr {
    status: Int = 500
    message: Str = \"\"
}

@server(43391)
fn main() => 0

@get(\"/users/{id}\")
fn get_user(id: Int) -> Result<Str, ApiErr> {
    if (id < 0) { return Err(ApiErr { status: 400, message: \"id inválido\" }) }
    return Ok(\"hola\")
}
";
    // Caso OK: status 200.
    let (status_ok, body_ok) =
        build_spawn_request("http-err-ok", src, 43391, "GET", "/users/5", None);
    assert_eq!(status_ok, 200);
    assert!(body_ok.contains("hola"));
}

#[test]
fn async_cl_inline_dentro_de_fn_async_compila() {
    // Async-cl build: async closures inline compilan adentro de
    // fns async. Emitidas como `move |...| -> Pin<Box<dyn Future +
    // Send>> { Box::pin(async move { ... }) }`.
    let src = "async fn run() -> Int {\n\
                   let f = async fn(n: Int) -> Int {\n\
                       sleep(1).await\n\
                       return n * 2\n\
                   }\n\
                   let r = f(21).await\n\
                   return r\n\
               }\n\
               print(run().await)\n";
    let (stdout, exit) = build_and_run("async_cl_inline_dentro_de_fn", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn cd_combinado_ho_y_f12_compila() {
    // Combina ambos features: fn nombrada como callback Y const hoisteado
    // referenciado adentro de la fn.
    let src = "let MULTIPLIER = 10\n\
               fn boost(n: Int) -> Int { return n * MULTIPLIER }\n\
               let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = xs.map(boost)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("cd_combinado", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 30]");
}

#[test]
fn mb3_round_trip_entries_to_map_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
               let back: Map<Str, Int> = m.entries().to_map()\n\
               print(back[\"a\"])\n\
               print(back[\"b\"])\n\
               print(back[\"c\"])\n";
    let (stdout, exit) = build_and_run("mb3_round_trip", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2\n3");
}

// ---- Mini-tanda Math + Mb9 + Int/Float methods ------------------

#[test]
fn math_builtins_basicos_compila() {
    let src = "print(abs(-5))\n\
               print(abs(-3.14))\n\
               print(min(3, 5))\n\
               print(max(1.5, 2.5))\n\
               print(pow(2, 10))\n\
               print(sqrt(16))\n\
               print(ceil(3.2))\n\
               print(floor(3.8))\n\
               print(round(3.5))\n\
               print(clamp(5, 0, 10))\n\
               print(clamp(-5, 0, 10))\n\
               print(clamp(15, 0, 10))\n";
    let (stdout, exit) = build_and_run("math_builtins", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "5\n3.14\n3\n2.5\n1024.0\n4.0\n4\n3\n4\n5\n0\n10"
    );
}

#[test]
fn mb9_str_swap_case_title_compila() {
    let src = "print(\"Hola Mundo\".swap_case())\n\
               print(\"hola mundo de fitz\".title())\n";
    let (stdout, exit) = build_and_run("mb9_str_swap_title", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hOLA mUNDO\nHola Mundo De Fitz");
}

#[test]
fn mb9_str_is_alpha_digit_numeric_compila() {
    let src = "print(\"hola\".is_alpha())\n\
               print(\"hola123\".is_alpha())\n\
               print(\"12345\".is_digit())\n\
               print(\"3.14\".is_numeric())\n\
               print(\"-42\".is_numeric())\n\
               print(\"3.14.5\".is_numeric())\n";
    let (stdout, exit) = build_and_run("mb9_str_is_predicates", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse\ntrue\ntrue\ntrue\nfalse");
}

#[test]
fn mb9_list_split_at_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let parts = xs.split_at(2)\n\
               print(parts)\n";
    let (stdout, exit) = build_and_run("mb9_list_split_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "([1, 2], [3, 4, 5])");
}

#[test]
fn mb9_map_has_value_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
               print(m.has_value(2))\n\
               print(m.has_value(99))\n";
    let (stdout, exit) = build_and_run("mb9_map_has_value", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

#[test]
fn int_methods_abs_to_str_to_str_base_compila() {
    let src = "let n: Int = -5\n\
               print(n.abs())\n\
               print((42).to_str())\n\
               print((255).to_str_base(16))\n\
               print((10).to_str_base(2))\n\
               print((8).to_str_base(8))\n";
    let (stdout, exit) = build_and_run("int_methods", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "5\n42\nff\n1010\n10");
}

#[test]
fn float_methods_abs_to_str_is_nan_is_finite_compila() {
    let src = "let x: Float = -3.14\n\
               print(x.abs())\n\
               print((3.14).to_str())\n\
               print((1.0).is_nan())\n\
               print((1.0).is_finite())\n";
    let (stdout, exit) = build_and_run("float_methods", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3.14\n3.14\nfalse\ntrue");
}

// ---- Mini-tanda Fp — default params ------------------

#[test]
fn fp_default_param_str_compila() {
    let src = "fn greet(name: Str = \"amigo\") -> Str {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(greet())\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("fp_default_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "Hola, amigo\nHola, Fitz");
}

#[test]
fn fp_mezcla_required_y_default_compila() {
    let src = "fn add(a: Int, b: Int = 10) -> Int {\n\
                   return a + b\n\
               }\n\
               print(add(5))\n\
               print(add(5, 2))\n";
    let (stdout, exit) = build_and_run("fp_mezcla", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "15\n7");
}

// ---- Mini-tanda Fp.2 — varargs ------------------

#[test]
fn fp2_varargs_basico_compila() {
    let src = "fn sum(...xs: Int) -> Int {\n\
                   let total: Int = 0\n\
                   for x in xs { total = total + x }\n\
                   return total\n\
               }\n\
               print(sum())\n\
               print(sum(1, 2, 3))\n\
               print(sum(10, 20))\n";
    let (stdout, exit) = build_and_run("fp2_varargs_basico", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0\n6\n30");
}

#[test]
fn fp2_varargs_con_required_compila() {
    let src = "fn join(prefix: Str, ...xs: Str) -> Int {\n\
                   return xs.len()\n\
               }\n\
               print(join(\"_\"))\n\
               print(join(\"_\", \"a\", \"b\"))\n\
               print(join(\"_\", \"x\", \"y\", \"z\"))\n";
    let (stdout, exit) = build_and_run("fp2_varargs_required", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0\n2\n3");
}

// ---- Mini-tanda Fp.3 — named args ------------------

#[test]
fn fp3_named_args_basico_compila() {
    let src = "fn greet(name: Str = \"amigo\", greeting: Str = \"Hola\") -> Str {\n\
                   return \"{greeting}, {name}\"\n\
               }\n\
               print(greet())\n\
               print(greet(name: \"Fitz\"))\n\
               print(greet(greeting: \"Hi\"))\n\
               print(greet(greeting: \"Hey\", name: \"Roy\"))\n";
    let (stdout, exit) = build_and_run("fp3_named_basico", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "Hola, amigo\nHola, Fitz\nHi, amigo\nHey, Roy"
    );
}

#[test]
fn fp3_mezcla_posicional_y_named_compila() {
    let src =
        "fn config(host: Str = \"127.0.0.1\", port: Int = 3000, debug: Bool = false) -> Str {\n\
                   return \"{host}:{port}/{debug}\"\n\
               }\n\
               print(config())\n\
               print(config(\"0.0.0.0\"))\n\
               print(config(\"0.0.0.0\", port: 8080))\n\
               print(config(port: 9000, debug: true))\n";
    let (stdout, exit) = build_and_run("fp3_mezcla", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "127.0.0.1:3000/false\n0.0.0.0:3000/false\n0.0.0.0:8080/false\n127.0.0.1:9000/true"
    );
}

// ---- Mini-tanda Sp.2 — return en match arm ------------------

#[test]
fn sp2_return_en_match_arm_compila() {
    let src = "fn classify(n: Int) -> Str {\n\
                   match n {\n\
                       0 => return \"cero\"\n\
                       1..10 => return \"chico\"\n\
                       _ => return \"grande\"\n\
                   }\n\
                   return \"unreachable\"\n\
               }\n\
               print(classify(0))\n\
               print(classify(5))\n\
               print(classify(100))\n";
    let (stdout, exit) = build_and_run("sp2_return_match", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "cero\nchico\ngrande");
}

// ---- Mini-tanda 5b.1 — param type inference desde call sites ----

#[test]
fn fp_5b1_param_sin_anotar_se_infiere_desde_call_site() {
    let src = "fn greet(name) {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("fp_5b1_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "Hola, Fitz");
}

#[test]
fn p2_param_y_return_ambos_inferidos_via_re_check() {
    // Mini-tanda P2 — chained fix 5b.1 + Hpx.2. Sin anotar param
    // NI return type, el codegen re-corre el checker tras inferir
    // el param desde el call site para refinar el return type.
    let src = "fn double(n) {\n\
                   return n * 2\n\
               }\n\
               fn greet(name) {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(double(21))\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("p2_both_inferred", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42\nHola, Fitz");
}

#[test]
fn fp_5b1_param_int_se_infiere_con_return_anotado() {
    // Cuando el return type está anotado pero el param no,
    // 5b.1 infiere el param sin colidir con Hpx.2.
    let src = "fn double(n) -> Int {\n\
                   return n * 2\n\
               }\n\
               print(double(21))\n";
    let (stdout, exit) = build_and_run("fp_5b1_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

// ---- Mini-tanda Hpx.2 — return type inference en handlers ------

#[test]
fn hpx2_fn_sin_anotacion_de_return_infiere_del_body_compila() {
    let src = "fn greet(name: Str) {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("hpx2_no_ret_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "Hola, Fitz");
}

#[test]
fn hpx2_fn_sin_anotacion_infiere_int_compila() {
    let src = "fn double(n: Int) {\n\
                   return n * 2\n\
               }\n\
               print(double(21))\n";
    let (stdout, exit) = build_and_run("hpx2_no_ret_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn hpx2_fn_con_if_else_infiere_lub() {
    let src = "fn maybe(b: Bool) {\n\
                   if b {\n\
                       return 42\n\
                   }\n\
                   return 0\n\
               }\n\
               print(maybe(true))\n\
               print(maybe(false))\n";
    let (stdout, exit) = build_and_run("hpx2_if_else", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42\n0");
}

#[test]
fn sp2_match_arm_con_block_compila() {
    let src = "fn f(n: Int) -> Int {\n\
                   return match n {\n\
                       0 => {\n\
                           let x: Int = 10\n\
                           return x * 2\n\
                       }\n\
                       _ => 99\n\
                   }\n\
               }\n\
               print(f(0))\n\
               print(f(1))\n";
    let (stdout, exit) = build_and_run("sp2_match_block", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "20\n99");
}

#[test]
fn fp_multiples_defaults_compila() {
    let src = "fn make(prefix: Str = \"x\", n: Int = 3, sep: Str = \"-\") -> Str {\n\
                   return \"{prefix}{sep}{n}\"\n\
               }\n\
               print(make())\n\
               print(make(\"y\"))\n\
               print(make(\"z\", 5))\n\
               print(make(\"w\", 7, \":\"))\n";
    let (stdout, exit) = build_and_run("fp_multiples", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "x-3\ny-3\nz-5\nw:7");
}

// ---------------------------------------------------------------------------
// Fase 9.w.1.d — Auth nativa en `fitz build`.
//
// Tests E2E que validan que un programa con `@auth_provider` +
// `@authenticated`/`@admin` compila a binario nativo y responde con
// los códigos esperados (200/401/403). Paridad bit-a-bit con los
// tests del intérprete en `src/http.rs::tests::auth_*`.
// ---------------------------------------------------------------------------

/// Spawn del server + barrido de requests con distintos Authorization
/// headers. Una sola compilación + un solo spawn para ahorrar tiempo;
/// los 5 requests se hacen sobre el mismo server vivo.
fn build_spawn_auth_requests(
    test_name: &str,
    src: &str,
    port: u16,
    requests: &[(&str, &str, Option<&str>)], // (method, path, optional bearer token)
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
    let mut results: Vec<(u16, String)> = Vec::with_capacity(requests.len());
    for (method, path, token) in requests {
        let auth_header = match token {
            Some(t) => format!("Authorization: Bearer {}\r\n", t),
            None => String::new(),
        };
        let request = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
            method, path, addr, auth_header,
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
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
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        results.push((status, body));
    }

    let _ = child.kill();
    let _ = child.wait();
    results
}

#[test]
fn auth_codegen_flujo_completo_end_to_end() {
    // Programa de referencia: provider que matchea Authorization contra
    // dos tokens hardcoded (admin, user); ruta pública sin auth, una con
    // `@authenticated` y otra con `@admin`. El programa entero compila a
    // binario nativo con `fitz build`, se spawn-ea el server, y se le
    // hacen 6 requests cubriendo los casos del wrapper auth.
    let src = "\
@server(43901)\n\
fn main() => 0\n\
\n\
type User { id: Int, name: Str, role: Str }\n\
\n\
@auth_provider\n\
fn check(headers: Map<Str, Str>) -> Result<User> {\n\
    match headers.get(\"authorization\") {\n\
        Ok(token) => {\n\
            if (token == \"Bearer admin-token\") {\n\
                return Ok(User { id: 1, name: \"Admin\", role: \"admin\" })\n\
            }\n\
            if (token == \"Bearer user-token\") {\n\
                return Ok(User { id: 2, name: \"Alice\", role: \"user\" })\n\
            }\n\
            return Err(\"token inválido\")\n\
        }\n\
        Err(_) => return Err(\"falta Authorization\")\n\
    }\n\
}\n\
\n\
@get(\"/public\")\n\
fn public_route() -> Str => \"sin auth\"\n\
\n\
@authenticated\n\
@get(\"/me\")\n\
fn me(user: User) -> Str => user.name\n\
\n\
@admin\n\
@get(\"/admin\")\n\
fn admin_route(user: User) -> Str => \"hola admin\"\n\
";
    let results = build_spawn_auth_requests(
        "auth_codegen_flujo",
        src,
        43901,
        &[
            ("GET", "/public", None),
            ("GET", "/me", None),
            ("GET", "/me", Some("wrong-token")),
            ("GET", "/me", Some("user-token")),
            ("GET", "/admin", Some("user-token")),
            ("GET", "/admin", Some("admin-token")),
        ],
    );
    // /public sin auth → 200 con body "sin auth"
    assert_eq!(results[0].0, 200, "public route 200, fue {:?}", results[0]);
    assert!(
        results[0].1.contains("sin auth"),
        "body /public: {:?}",
        results[0].1
    );

    // /me sin Authorization → 401 con "falta Authorization"
    assert_eq!(results[1].0, 401, "/me sin auth 401, fue {:?}", results[1]);
    assert!(
        results[1].1.contains("falta Authorization"),
        "/me sin header body: {:?}",
        results[1].1
    );

    // /me con token inválido → 401 con "token inválido"
    assert_eq!(
        results[2].0, 401,
        "/me token inválido 401, fue {:?}",
        results[2]
    );
    assert!(
        results[2].1.contains("token inválido"),
        "/me con token wrong body: {:?}",
        results[2].1
    );

    // /me con user válido → 200 con "Alice"
    assert_eq!(
        results[3].0, 200,
        "/me user válido 200, fue {:?}",
        results[3]
    );
    assert!(
        results[3].1.contains("Alice"),
        "/me user body: {:?}",
        results[3].1
    );

    // /admin con rol user → 403
    assert_eq!(results[4].0, 403, "/admin user → 403, fue {:?}", results[4]);
    assert!(
        results[4].1.contains("admin"),
        "/admin con rol user body: {:?}",
        results[4].1
    );

    // /admin con rol admin → 200 con "hola admin"
    assert_eq!(
        results[5].0, 200,
        "/admin admin → 200, fue {:?}",
        results[5]
    );
    assert!(
        results[5].1.contains("hola admin"),
        "/admin admin body: {:?}",
        results[5].1
    );
}

#[test]
fn auth_codegen_jwt_y_hash_builtins_cli() {
    // CLI puro: usa `jwt.encode`/`jwt.decode` + `hash.password`/
    // `hash.verify` sin handlers HTTP. Verifica que las deps se sumen
    // condicional al Cargo.toml generado y que los helpers estén en el
    // preludio. El binario imprime el shape esperado de cada salida.
    let src = "\
let secret = \"secret-32-bytes-long-test-aaaaaa\"\n\
let claims: Map<Str, Str> = {\"sub\": \"u42\", \"role\": \"admin\"}\n\
let token = jwt.encode(claims, secret)\n\
print(\"jwt-len-gt-20: {len(token) > 20}\")\n\
let pw = hash.password(\"supersecret\")\n\
print(\"argon2id-prefix: {pw[0..10]}\")\n\
let ok = hash.verify(\"supersecret\", pw)\n\
let bad = hash.verify(\"wrong\", pw)\n\
print(\"verify-ok: {ok}\")\n\
print(\"verify-bad: {bad}\")\n\
";
    let (stdout, exit) = build_and_run("auth_jwt_hash_cli", src);
    assert_eq!(exit, 0, "exit code: {} stdout: {}", exit, stdout);
    assert!(stdout.contains("jwt-len-gt-20: true"), "stdout: {}", stdout);
    assert!(
        stdout.contains("argon2id-prefix: $argon2id$"),
        "stdout: {}",
        stdout
    );
    assert!(stdout.contains("verify-ok: true"), "stdout: {}", stdout);
    assert!(stdout.contains("verify-bad: false"), "stdout: {}", stdout);
}

// ---------------------------------------------------------------------------
// Fase 9.w.2.c — Tests E2E del codegen WebSocket.
//
// Compilan un programa con `@ws("/path")` a binario nativo con `fitz
// build`, spawnean el binario, conectan un cliente WS via
// tokio-tungstenite (dev-dep), envían/reciben frames, y verifican que
// el flujo matchea bit-a-bit el comportamiento del intérprete (9.w.2.b).
// ---------------------------------------------------------------------------

/// Spawn del binario WS + cliente que envía/recibe un mensaje texto.
/// Devuelve la respuesta del server.
///
/// SERIAL es Mutex<()> sync intencional para serializar tests
/// (cargo test corre tests en paralelo por default; SERIAL hace
/// mutex global por test E2E que invoca rustc). Mantenerlo durante
/// el await es deliberado — soltarlo defeats the purpose.
#[allow(clippy::await_holding_lock)]
async fn ws_build_send_recv(
    test_name: &str,
    src: &str,
    port: u16,
    path: &str,
    payload: &str,
) -> String {
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
        "fitz build WS falló:\nstdout: {}\nstderr: {}",
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
        panic!("server WS no abrió el puerto {} en 3s", port);
    }

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let url = format!("ws://{}{}", addr, path);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect");
    ws.send(Message::text(payload.to_string()))
        .await
        .expect("send");
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .expect("frame")
        .expect("ok");
    let txt = match frame {
        Message::Text(t) => t.to_string(),
        other => panic!("esperaba text, fue {:?}", other),
    };
    let _ = child.kill();
    let _ = child.wait();
    txt
}

#[test]
fn ws_codegen_echo_simple_compila_y_responde() {
    // Programa mínimo `@ws` con WsConn<Str>. Verifica que el codegen
    // emite el preludio + wrapper + dispatch de los métodos, y que el
    // binario nativo responde idéntico al intérprete.
    let src = "@server(43971)\n\
               fn main() => 0\n\
               @ws(\"/echo\")\n\
               async fn echo(conn: WsConn<Str>) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(msg) => {\n\
                           let _ = conn.send(\"eco-{msg}\")\n\
                           return null\n\
                       }\n\
                       Err(_) => return null\n\
                   }\n\
               }";
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let resp = rt.block_on(async {
        ws_build_send_recv("ws_codegen_echo", src, 43971, "/echo", "\"hola\"").await
    });
    assert_eq!(resp, "\"eco-hola\"");
}

#[test]
fn ws_codegen_tipo_custom_marshaling_json() {
    // `WsConn<ChatMsg>` con tipo custom. El codegen debe emitir el
    // marshaling JSON via __ToFitzJson/__FromFitzJson del tipo,
    // espejo del runtime (9.w.2.b).
    let src = "@server(43972)\n\
               fn main() => 0\n\
               type ChatMsg { user: Str, text: Str }\n\
               @ws(\"/chat\")\n\
               async fn chat(conn: WsConn<ChatMsg>) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(msg) => {\n\
                           let reply = ChatMsg { user: msg.user, text: \"re:{msg.text}\" }\n\
                           let _ = conn.send(reply)\n\
                           return null\n\
                       }\n\
                       Err(_) => return null\n\
                   }\n\
               }";
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let resp = rt.block_on(async {
        ws_build_send_recv(
            "ws_codegen_custom",
            src,
            43972,
            "/chat",
            "{\"user\":\"ada\",\"text\":\"hi\"}",
        )
        .await
    });
    // serde_json preserve_order: el orden de los fields del struct se
    // mantiene en la serialización.
    let v: serde_json::Value = serde_json::from_str(&resp).expect("JSON válido");
    assert_eq!(v["user"], serde_json::json!("ada"));
    assert_eq!(v["text"], serde_json::json!("re:hi"));
}

// ---------------------------------------------------------------------------
// 9.w.2-binary-frames — `WsConn<Bytes>` end-to-end con codegen
// ---------------------------------------------------------------------------

/// Variante binaria de `ws_build_send_recv`. Conecta vía WS, envía un
/// frame Binary, recibe el response también como Binary. Garantía:
/// bytes pasan bit-a-bit por el wire.
#[allow(clippy::await_holding_lock)]
async fn ws_build_send_recv_binary(
    test_name: &str,
    src: &str,
    port: u16,
    path: &str,
    payload: Vec<u8>,
) -> Vec<u8> {
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
        "fitz build WS<Bytes> falló:\nstdout: {}\nstderr: {}",
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
        panic!("server WS<Bytes> no abrió el puerto {} en 3s", port);
    }

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let url = format!("ws://{}{}", addr, path);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect");
    ws.send(Message::binary(payload))
        .await
        .expect("send binary");
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .expect("frame")
        .expect("ok");
    let bs = match frame {
        Message::Binary(b) => b.to_vec(),
        other => panic!("esperaba binary, fue {:?}", other),
    };
    let _ = child.kill();
    let _ = child.wait();
    bs
}

#[test]
fn ws_codegen_binary_echo_compila_y_responde() {
    // `WsConn<Bytes>` round-trip end-to-end. El binario nativo debería
    // aceptar frames binarios raw (no JSON-marshalled), procesarlos como
    // `Value::Bytes` y devolverlos sin tocar — paridad bit-a-bit con el
    // intérprete (`ws_echo_binary_round_trip` en src/http.rs).
    let src = "@server(43973)\n\
               fn main() => 0\n\
               @ws(\"/raw\")\n\
               async fn raw(conn: WsConn<Bytes>) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(buf) => match conn.send(buf) {\n\
                           Ok(_) => return null,\n\
                           Err(_) => return null,\n\
                       },\n\
                       Err(_) => return null,\n\
                   }\n\
               }";
    let payload: Vec<u8> = vec![0x00, 0x01, 0x10, 0x80, 0xff, 0x7e];
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let resp = rt.block_on(async {
        ws_build_send_recv_binary(
            "ws_codegen_binary",
            src,
            43973,
            "/raw",
            payload.clone(),
        )
        .await
    });
    assert_eq!(resp, payload);
}

// ---------------------------------------------------------------------------
// 9.w.2-ws-auth-browser — auth WS via subprotocol `bearer.<token>`
// ---------------------------------------------------------------------------

/// E2E del codegen `fitz build` con auth via subprotocol. El cliente
/// envía `bearer.<token>` como subprotocol; el server lo extrae,
/// valida con el @auth_provider, hace echo del subprotocol en el
/// handshake, y el handler corre con el `user` inyectado.
#[allow(clippy::await_holding_lock)]
#[test]
fn ws_codegen_auth_via_subprotocol_acepta_token() {
    let src = "@server(43990)\n\
               fn main() => 0\n\
               type User { id: Int, name: Str, role: Str }\n\
               @auth_provider\n\
               fn check(h: Map<Str, Str>) -> Result<User> {\n\
                   let v: Str = match h.get(\"authorization\") {\n\
                       Ok(s) => s,\n\
                       Err(_) => return Err(\"falta authorization\")\n\
                   }\n\
                   if (v == \"Bearer secret-tok\") {\n\
                       return Ok(User { id: 1, name: \"Ada\", role: \"user\" })\n\
                   }\n\
                   return Err(\"token inválido\")\n\
               }\n\
               @authenticated\n\
               @ws(\"/chat\")\n\
               async fn chat(conn: WsConn<Str>, user: User) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(_) => {\n\
                           let _ = conn.send(\"hola {user.name}\")\n\
                           return null\n\
                       }\n\
                       Err(_) => return null\n\
                   }\n\
               }";
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (status_proto_echoed, msg) = rt.block_on(async move {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join("fitz-e2e-ws-auth-subproto");
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
        use std::process::{Child, Stdio};
        let mut child: Child = Command::new(&bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let addr = "127.0.0.1:43990";
        let start = std::time::Instant::now();
        let mut connected = false;
        while start.elapsed() < std::time::Duration::from_secs(3) {
            if std::net::TcpStream::connect(addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !connected {
            let _ = child.kill();
            panic!("server WS no abrió el puerto en 3s");
        }
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let url = format!("ws://{}/chat", addr);
        let mut req = url.as_str().into_client_request().unwrap();
        req.headers_mut().insert(
            "sec-websocket-protocol",
            "bearer.secret-tok".parse().unwrap(),
        );
        let (mut ws, resp) = tokio_tungstenite::connect_async(req)
            .await
            .expect("handshake debería pasar con bearer.secret-tok");
        let echoed = resp
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        ws.send(tokio_tungstenite::tungstenite::Message::text("\"hola\""))
            .await
            .expect("send");
        let frame = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        let msg = match frame {
            tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
            other => panic!("esperaba text, fue {:?}", other),
        };
        let _ = child.kill();
        let _ = child.wait();
        (echoed, msg)
    });
    assert_eq!(status_proto_echoed, "bearer.secret-tok");
    assert_eq!(msg, "\"hola Ada\"");
}

// ---------------------------------------------------------------------------
// R.bug-deadlock — regression test (2026-05-21)
// ---------------------------------------------------------------------------

/// El codegen pre-fix emitía un `format!(fmt, arg1, arg2, ...)` donde
/// los temporales (MutexGuards de `.lock().unwrap()`) vivían hasta el
/// final de la statement. Si dos args lockeaban el mismo Arc<Mutex<>>
/// (caso típico: `print("{xs.len()} - {total(xs)}")`), el segundo
/// `.lock()` desde el mismo thread quedaba bloqueado esperando que el
/// primero libere → deadlock silencioso del binario.
///
/// Fix: `gen_str_interp` ahora emite cada arg como `let __aN = <code>;`
/// en un bloque ANTES del `format!`. Cada `let` cierra una statement,
/// dropea el MutexGuard temporal antes de evaluar el siguiente arg.
///
/// Este test corre el binario con timeout — si el deadlock vuelve, el
/// test falla por exit code (timeout). El stdout valida que ambas
/// líneas se imprimen.
#[test]
fn r_bug_deadlock_str_interp_re_lock_mismo_arc_no_cuelga() {
    let src = "type Sale { product: Str, amount: Float }\n\
               let xs: List<Sale> = [\n\
                   Sale { product: \"a\", amount: 1.0 },\n\
                   Sale { product: \"b\", amount: 2.0 },\n\
               ]\n\
               fn total(sales: List<Sale>) -> Float {\n\
                   return sales.reduce(0.0, fn(acc: Float, s: Sale) => acc + s.amount)\n\
               }\n\
               print(\"len={xs.len()} total={total(xs)}\")\n";
    let (stdout, exit) = build_and_run("r-bug-deadlock-str-interp", src);
    assert_eq!(
        exit, 0,
        "binario debió terminar limpio (timeout = deadlock)"
    );
    assert!(
        stdout.contains("len=2"),
        "esperaba `len=2` en stdout, fue: {}",
        stdout
    );
    assert!(
        stdout.contains("total=3"),
        "esperaba `total=3` en stdout, fue: {}",
        stdout
    );
}

// ----------------------------------------------------------------------
// Mini-fase 8.7.bis — Coerción PyAny → List<T> / Nominal / List<Nominal>
// en codegen, paridad con la coerción runtime cerrada en Paso 1 del
// plan post-boilerplates.
// ----------------------------------------------------------------------

#[cfg(feature = "python")]
#[test]
fn fase_8_7_bis_build_pyany_a_list_int_via_anotacion() {
    // `json.loads("[1, 2, 3]")` devuelve un PyList opaco. La anotación
    // `List<Int>` dispara `__fitz_py_to_list_i64` (ya emitido en el
    // preludio) — antes de 8.7.bis el coerce caía al passthrough y el
    // binario no compilaba con error de tipo Rust.
    //
    // Patrón: workaround para que match arms con varios stmts compilen:
    // envolver el cuerpo en una fn que retorna `Result<Null>` y luego
    // pattern-match al top level.
    let src = "from python import json\n\
               fn work() -> Result<Null> {\n\
                   let raw = json.loads(\"[1, 2, 3]\")?\n\
                   let xs: List<Int> = raw\n\
                   let sum = xs.reduce(0, fn(a: Int, b: Int) => a + b)\n\
                   print(\"len={xs.len()} sum={sum}\")\n\
                   return Ok(null)\n\
               }\n\
               match work() {\n\
                 Ok(_) => print(\"done\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_bis_list_int", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert!(
        stdout.contains("len=3 sum=6"),
        "esperaba `len=3 sum=6` en stdout, fue: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_bis_build_pyany_a_instance_via_anotacion() {
    // `json.loads("{...}")` devuelve PyDict opaco. La anotación `User`
    // (nominal) dispara `__fitz_py_to_instance_User` emitido por
    // `gen_type_def` cuando uses_python=true.
    //
    // El `{` y `}` del JSON literal se escapan con `{{` y `}}` (regla
    // estándar de interpolación de strings Fitz — sin escape Fitz los
    // interpreta como start/end de interpolación).
    // `\{` y `\}` escapan literal `{`/`}` adentro del string (sin escape
    // se interpretan como inicio/fin de interpolación).
    let src = "type User { id: Int, name: Str, email: Str = \"\" }\n\
               from python import json\n\
               fn work() -> Result<Null> {\n\
                   let raw = json.loads(\"\\{\\\"id\\\": 7, \\\"name\\\": \\\"ada\\\"\\}\")?\n\
                   let u: User = raw\n\
                   print(\"id={u.id} name={u.name} email='{u.email}'\")\n\
                   return Ok(null)\n\
               }\n\
               match work() {\n\
                 Ok(_) => print(\"done\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_bis_instance", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    // El default `email = ""` se aplica porque el dict no trae la key.
    assert!(
        stdout.contains("id=7 name=ada email=''"),
        "esperaba `id=7 name=ada email=''` en stdout, fue: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_bis_build_pyany_a_list_de_instances() {
    // Patrón canónico del boilerplate `api-postgres-python` /
    // `api-fullstack-postgres`: `let users: List<User> = json.loads(s)?`.
    // El codegen emite `__fitz_py_to_list_User` que itera el PyList y
    // llama a `__fitz_py_to_instance_User` por item. Antes de 8.7.bis
    // este código no compilaba con error de tipo Rust.
    let src = "type User { id: Int, name: Str }\n\
               from python import json\n\
               fn work() -> Result<Null> {\n\
                   let raw = json.loads(\"[\\{\\\"id\\\": 1, \\\"name\\\": \\\"ada\\\"\\}, \\{\\\"id\\\": 2, \\\"name\\\": \\\"luis\\\"\\}]\")?\n\
                   let users: List<User> = raw\n\
                   print(\"n={users.len()} first={users[0].name}\")\n\
                   return Ok(null)\n\
               }\n\
               match work() {\n\
                 Ok(_) => print(\"done\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_bis_list_instance", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert!(
        stdout.contains("n=2 first=ada"),
        "esperaba `n=2 first=ada` en stdout, fue: {}",
        stdout
    );
}

// ----------------------------------------------------------------------
// Mini-fase env builtin (2026-05-22, Paso 3 post-boilerplates) —
// `env(key)`, `env_or(key, default)`, `load_env(path)`. Tests E2E del
// codegen: build a binario nativo + spawn con env var inyectada via
// `Command::env(...)` → validar que el binario lee la var correctamente.
// ----------------------------------------------------------------------

#[test]
fn env_builtin_lee_var_existente_y_propaga_con_try() {
    let src = "\
        fn read() -> Result<Str> {\n\
            let s = env(\"FITZ_E2E_GREETING\")?\n\
            return Ok(\"hola, {s}!\")\n\
        }\n\
        match read() {\n\
          Ok(v) => print(v),\n\
          Err(e) => print(\"err: {e}\")\n\
        }\n";
    let (stdout, exit) =
        build_and_run_with_env("env-builtin-exists", src, &[("FITZ_E2E_GREETING", "mundo")]);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola, mundo!");
}

#[test]
fn env_builtin_var_missing_propaga_err() {
    // No seteamos la var → env() devuelve Err, el `?` propaga, el
    // match top-level imprime el msg de error.
    let src = "\
        fn read() -> Result<Str> {\n\
            let s = env(\"FITZ_E2E_NUNCA_EXISTE_XYZ\")?\n\
            return Ok(s)\n\
        }\n\
        match read() {\n\
          Ok(v) => print(\"got: {v}\"),\n\
          Err(e) => print(\"caught: {e}\")\n\
        }\n";
    let (stdout, exit) = build_and_run_with_env("env-builtin-missing", src, &[]);
    assert_eq!(exit, 0);
    assert!(
        stdout.contains("caught:") && stdout.contains("FITZ_E2E_NUNCA_EXISTE_XYZ"),
        "esperaba caught con key, fue: {}",
        stdout
    );
}

#[test]
fn env_or_builtin_devuelve_default_si_missing() {
    let src = "\
        let port = env_or(\"FITZ_E2E_NO_SET_PORT\", \"3000\")\n\
        print(\"port={port}\")\n";
    let (stdout, exit) = build_and_run_with_env("env-or-default", src, &[]);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "port=3000");
}

#[test]
fn env_or_builtin_var_existente_ignora_default() {
    let src = "\
        let port = env_or(\"FITZ_E2E_PORT_REAL\", \"3000\")\n\
        print(\"port={port}\")\n";
    let (stdout, exit) =
        build_and_run_with_env("env-or-real", src, &[("FITZ_E2E_PORT_REAL", "8080")]);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "port=8080");
}

#[test]
fn load_env_builtin_carga_archivo_y_lee_vars() {
    // Escribimos un .env en un tempdir SEPARADO al del build (el build
    // hace `remove_dir_all` adentro de `fitz-e2e-<test_name>`, no queremos
    // que pise el env file). La fitz src lee `FITZ_E2E_LOAD_PATH` (env
    // var del runtime) para saber dónde está el .env.
    let envdir = std::env::temp_dir().join("fitz-e2e-load-env-FILES");
    let _ = std::fs::remove_dir_all(&envdir);
    std::fs::create_dir_all(&envdir).unwrap();
    let env_file = envdir.join("config.env");
    std::fs::write(
        &env_file,
        "# config para el test\nFITZ_E2E_LOAD_K1=valor1\nFITZ_E2E_LOAD_K2=\"con espacios\"\n",
    )
    .unwrap();

    // Wrap todo el laburo en una fn que retorna Result<Null> para
    // poder usar el patrón `?` y dejar el match top-level con un solo
    // print por arm (limitación: print no es expresión en codegen).
    let src = "\
        fn boot_and_read() -> Result<Null> {\n\
            let path = env(\"FITZ_E2E_LOAD_PATH\")?\n\
            let _ = load_env(path)?\n\
            let k1 = env_or(\"FITZ_E2E_LOAD_K1\", \"\")\n\
            let k2 = env_or(\"FITZ_E2E_LOAD_K2\", \"\")\n\
            print(\"k1={k1} k2={k2}\")\n\
            return Ok(null)\n\
        }\n\
        match boot_and_read() {\n\
          Ok(_) => print(\"done\"),\n\
          Err(e) => print(\"err: {e}\")\n\
        }\n";
    let env_path = env_file.to_str().unwrap().to_string();
    let (stdout, exit) = build_and_run_with_env(
        "load-env-codegen",
        src,
        &[("FITZ_E2E_LOAD_PATH", env_path.as_str())],
    );
    let _ = std::fs::remove_file(&env_file);
    assert_eq!(exit, 0, "stdout fue: {}", stdout);
    assert!(
        stdout.contains("k1=valor1 k2=con espacios"),
        "esperaba k1+k2 cargados del archivo, fue: {}",
        stdout
    );
}

// ----------------------------------------------------------------------
// Mini-tanda Cleanup-Residual (2026-05-22) — R.bug-result-status:
// handler con return type Result<T> + return <status> mezclados.
// Pre-fix el codegen serializaba el Result wrapper como
// `{"Ok":{...}}` en lugar de desempacar el inner. Caso del
// boilerplate api-simple que tenía workaround inline.
// ----------------------------------------------------------------------

#[test]
fn r_bug_result_status_handler_unwrap_ok_serializa_t_directo() {
    let src = "\
@server(43777)
fn main() => 0

type Item { id: Int, name: Str }

@get(\"/items/{id}\")
fn get_item(id: Int) -> Result<Item> {
    if (id == 1) {
        return Ok(Item { id: 1, name: \"alpha\" })
    }
    return 404 { \"error\": \"no encontrado\" }
}
";
    // GET /items/1 — return Ok(Item) debe serializar Item directo,
    // no `{"Ok":{...}}`.
    let (status, body) = build_spawn_request(
        "r-bug-result-status-ok",
        src,
        43777,
        "GET",
        "/items/1",
        None,
    );
    assert_eq!(status, 200, "body: {}", body);
    let lower = body.to_lowercase();
    assert!(
        !lower.contains("\"ok\":") && !lower.contains("\"err\":"),
        "el body NO debe tener wrapper Ok/Err: {}",
        body
    );
    assert!(
        body.contains("\"id\":1") && body.contains("\"name\":\"alpha\""),
        "el body debe ser el Item directo: {}",
        body
    );
}

#[test]
fn r_bug_result_status_handler_path_404_funciona() {
    let src = "\
@server(43778)
fn main() => 0

type Item { id: Int, name: Str }

@get(\"/items/{id}\")
fn get_item(id: Int) -> Result<Item> {
    if (id == 1) {
        return Ok(Item { id: 1, name: \"alpha\" })
    }
    return 404 { \"error\": \"no encontrado\" }
}
";
    // GET /items/99 — return <status> path sigue funcionando OK.
    let (status, body) = build_spawn_request(
        "r-bug-result-status-404",
        src,
        43778,
        "GET",
        "/items/99",
        None,
    );
    assert_eq!(status, 404, "body: {}", body);
    assert!(
        body.contains("\"error\":\"no encontrado\""),
        "404 debe traer mensaje: {}",
        body
    );
}

// ----------------------------------------------------------------------
// Mini-fase loader-absoluto (2026-05-22, Paso 4 post-boilerplates) —
// `from sub.foo` desde un módulo en subcarpeta debe resolver al
// import_root (parent del entry file) cuando la resolución relativa
// falla. Caso canónico del boilerplate api-postgres-python.
// ----------------------------------------------------------------------

#[test]
fn loader_absoluto_data_sibling_import_compila_en_fitz_build() {
    // Setup multi-archivo:
    //   src/main.fitz       — usa data/users (que importa types/user)
    //   src/types/user.fitz — define type User
    //   src/data/users.fitz — `from types.user import User` (resuelve via import_root)
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("fitz-e2e-loader-absoluto");
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("types")).unwrap();
    std::fs::create_dir_all(src.join("data")).unwrap();

    std::fs::write(
        src.join("types").join("user.fitz"),
        "type User { id: Int, name: Str }\n",
    )
    .unwrap();
    // data/users.fitz importa `User` desde el módulo hermano types/.
    // El test clave: ese `from types.user import User` resuelve via
    // import_root (parent del main.fitz), no via la búsqueda relativa
    // que daría `src/data/types/user.fitz` (mal).
    //
    // Sólo testeamos que el import RESUELVA (build success); cómo
    // usamos el tipo después está fuera del scope de este test.
    std::fs::write(
        src.join("data").join("users.fitz"),
        "from types.user import User\n\
         fn make_name(u: User) -> Str => u.name\n",
    )
    .unwrap();
    std::fs::write(
        src.join("main.fitz"),
        "from data.users import make_name\n\
         from types.user import User\n\
         let u = User { id: 7, name: \"ada\" }\n\
         print(\"hello {make_name(u)}\")\n",
    )
    .unwrap();

    // Build el main.fitz directamente.
    let main_fitz = src.join("main.fitz");
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_fitz)
        .output()
        .expect("invocar fitz build");
    assert!(
        output.status.success(),
        "fitz build falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) { "main.exe" } else { "main" };
    let bin = src.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);
    assert_eq!(stdout.trim(), "hello ada");

    let _ = std::fs::remove_dir_all(&dir);
}
