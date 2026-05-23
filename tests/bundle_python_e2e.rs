// bundle_python_e2e.rs — Tests integration de Fase 8.b (bundling
// CPython embebido vía `fitz build --bundle-python`).
//
// El happy path (build + run del binario standalone) NO se testea
// acá porque requiere:
//  - Tarball PBS de ~21 MB descargado (~10s primera vez en CI)
//  - Python 3.14.x disponible en PATH al build time (deuda
//    R.bug-pyo3-abi3-portable-link)
//  - ~3-5s de cold start sobre tar -xzf en cada corrida
//
// El smoke manual cubre el happy path. Acá testeamos:
//  - Validación temprana: `--bundle-python` sin `from python import`
//    aborta con mensaje claro.
//  - Validación temprana: programa con errores de sintaxis aborta con
//    error de lex/parse, no de bundling.

use std::process::Command;

/// Path al binario `fitz` que cargo construye para integration tests.
fn fitz_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fitz")
}

#[test]
fn bundle_python_sin_from_python_import_aborta_con_mensaje_claro() {
    let dir = std::env::temp_dir().join("fitz-bundle-validation-no-python");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let fitz_src = dir.join("prog.fitz");
    std::fs::write(
        &fitz_src,
        r#"print("hola mundo sin python")
let x = 42
print(x)
"#,
    )
    .expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build", "--bundle-python"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build --bundle-python");

    // Debe abortar — el programa NO usa interop Python.
    assert!(
        !output.status.success(),
        "esperaba que abortara: el programa no usa `from python import`"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Mensaje específico debe mencionar el flag y la condición.
    assert!(
        stderr.contains("--bundle-python")
            && stderr.contains("from python import"),
        "mensaje debe explicar el constraint del flag. Got stderr:\n{}",
        stderr
    );
    // Sugerencia de fix también.
    assert!(
        stderr.contains("sin el flag"),
        "mensaje debería sugerir omitir el flag. Got stderr:\n{}",
        stderr
    );
}

#[test]
fn bundle_pip_implica_bundle_python_y_aborta_sin_from_python_import() {
    // Fase 8.c: `--bundle-pip <pkg>` implica `--bundle-python`. Sin
    // `from python import` en el código, debe abortar con el mismo
    // mensaje que `--bundle-python` directo.
    let dir = std::env::temp_dir().join("fitz-bundle-pip-no-python");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, "print(\"hola sin python\")\n").expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build", "--bundle-pip", "requests"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build --bundle-pip requests");

    assert!(
        !output.status.success(),
        "esperaba que abortara: el programa no usa `from python import`"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--bundle-python") && stderr.contains("from python import"),
        "mensaje debe explicar el constraint del flag. Got stderr:\n{}",
        stderr
    );
}

#[test]
fn bundle_pip_repetible_acepta_varios_paquetes() {
    // Validación del CLI parsing: --bundle-pip debe ser repetible.
    // No corremos el pipeline real (eso requiere red + Python), solo
    // validamos que el parser acepta el flag varias veces sin error.
    // Sigue abortando por falta de `from python import`, pero el
    // error es el de validación tardía, no el de parsing.
    let dir = std::env::temp_dir().join("fitz-bundle-pip-multiple");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let fitz_src = dir.join("prog.fitz");
    std::fs::write(&fitz_src, "print(\"sin python\")\n").expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args([
            "build",
            "--bundle-pip",
            "requests",
            "--bundle-pip",
            "sqlalchemy==2.0.0",
            "--bundle-pip",
            "psycopg2-binary",
        ])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build con varios --bundle-pip");

    // Debe abortar por validation (sin from python import), pero
    // NO por error de CLI parsing.
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Sin error de clap (error de parsing del CLI).
    assert!(
        !stderr.contains("error: invalid value")
            && !stderr.contains("error: the argument"),
        "clap NO debería rechazar el flag repetido. Got stderr:\n{}",
        stderr
    );
    // SÍ debería tener el error de validation tardía.
    assert!(
        stderr.contains("from python import"),
        "debe llegar a la validación tardía. Got stderr:\n{}",
        stderr
    );
}

#[test]
fn bundle_python_con_error_de_sintaxis_aborta_antes_de_bundling() {
    let dir = std::env::temp_dir().join("fitz-bundle-validation-syntax-err");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let fitz_src = dir.join("prog.fitz");
    // Programa con error sintáctico: `let` sin valor.
    std::fs::write(&fitz_src, "from python import math\nlet x =\n").expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build", "--bundle-python"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz build --bundle-python");

    assert!(
        !output.status.success(),
        "esperaba que abortara por error de sintaxis"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // El error debe venir del parser (no del bundling) — sin
    // "→ compilando real binary…" ni "→ asegurando PBS tarball…",
    // que aparecen solo si pasa lex/parse/check.
    assert!(
        !stderr.contains("→ compilando real binary"),
        "el pipeline de bundling NO debe arrancar si el parse falla. Got stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("→ asegurando PBS tarball"),
        "el download del tarball NO debe dispararse si el parse falla. Got stderr:\n{}",
        stderr
    );
}
