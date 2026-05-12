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
    assert_lines(&stdout, &["6"]);
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

#[test]
fn build_aborta_si_codegen_no_soporta_feature() {
    // Listas no se soportan en 5b.1.
    let stderr = build_expect_fail(
        "unsupported-list",
        "let xs = [1, 2, 3]\nprint(xs)\n",
    );
    assert!(
        stderr.contains("5b.3") || stderr.contains("listas"),
        "esperaba mensaje sobre listas / 5b.3, fue: {}",
        stderr
    );
}
