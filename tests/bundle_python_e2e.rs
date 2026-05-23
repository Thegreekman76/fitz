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
