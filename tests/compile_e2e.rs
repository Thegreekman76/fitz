// compile_e2e.rs — Tests integration de Fase 5b.1.
//
// Toma un programa Fitz, llama a `fitz build`, ejecuta el binario y
// chequea stdout / exit code. Los tests usan un directorio temporal
// único por test (concatenando el nombre del test) para no pisar
// builds entre corridas.
//
// Importante: estos tests **invocan rustc** internamente vía `fitz
// build`. Son más lentos que los unitarios; cada uno toma ~2s.

use std::path::PathBuf;
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
fn build_aborta_si_codegen_no_soporta_feature() {
    // 5b.6 abre @get/@post/etc. La feature bloqueada que apuntamos acá
    // pasa a ser **state compartido HTTP** (`let X = ...` top-level
    // junto a decoradores HTTP) — el codegen aborta con mensaje claro
    // citando 5b.6 como deuda residual.
    let stderr = build_expect_fail(
        "unsupported-http-state",
        "let users = [1, 2]\n@get(\"/users\") fn list() -> Str => \"x\"\n",
    );
    assert!(
        stderr.contains("state compartido") || stderr.contains("5b.6"),
        "esperaba mensaje sobre state compartido / 5b.6, fue: {}",
        stderr
    );
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
    "04-operadores.fitz",
    "05-strings.fitz",
    "06-logica.fitz",
    "07-if.fitz",
    "08-loops.fitz",
    "10-match.fitz",
    "12-type.fitz",
    "13-metodos.fitz",
    "14-result.fitz",
    "16-modulos.fitz",
    "18-build.fitz",
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

#[test]
fn http_state_compartido_aborta_build() {
    let stderr = build_expect_fail(
        "http-state-aborts",
        "let users = [1, 2]\n@get(\"/users\") fn list() -> Str => \"x\"\n",
    );
    assert!(
        stderr.contains("state compartido"),
        "esperaba mensaje sobre state compartido, fue: {}",
        stderr
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
