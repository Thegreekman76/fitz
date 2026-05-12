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
    // 5b.4 abre Result/match. La feature bloqueada que apuntamos acá
    // pasa a ser `import` (5b.5) — el codegen aborta con mensaje claro.
    let stderr = build_expect_fail(
        "unsupported-import",
        "from foo import bar\nprint(bar)\n",
    );
    assert!(
        stderr.contains("5b.5") || stderr.contains("import"),
        "esperaba mensaje sobre import / 5b.5, fue: {}",
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
