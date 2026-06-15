// launcher_template_e2e.rs — Test integration de Fase 8.b.3.
//
// Valida que el template del launcher, una vez sustituido con paths
// reales (dummies), compila como un binario Rust válido. Esto cubre:
//  - Sintaxis del template (no hay bugs tipográficos en el código
//    embebido como string).
//  - Escapado correcto de paths Windows (backslashes y espacios).
//  - cfg! conditionals (Unix vs Windows) bien planteados.
//  - include_bytes! resuelve los paths sustituidos.
//
// **NO** valida que el launcher EJECUTE correctamente (eso es 8.b.5
// con un real binary + tarball PBS). Solo que compila.

use fitz::launcher_template;
use std::process::Command;
use std::sync::Mutex;

/// Mutex global: los tests de este archivo serializan invocaciones de
/// `cargo check` para evitar contención en el target dir compartido.
static SERIAL: Mutex<()> = Mutex::new(());

#[test]
fn template_launcher_compiles_with_dummy_paths() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    let dir = std::env::temp_dir().join("fitz-launcher-template-smoke");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("crear src dir");

    // Archivos dummy para include_bytes! (cualquier contenido sirve;
    // el smoke valida que compila, no que ejecuta bien).
    let tarball_path = dir.join("tarball.tar.gz");
    let real_binary_path = dir.join("fitz-real");
    std::fs::write(&tarball_path, b"dummy tarball content").expect("write tarball");
    std::fs::write(&real_binary_path, b"dummy real binary content").expect("write real binary");

    // Generar main.rs y Cargo.toml con paths reales sustituidos.
    let main_rs = launcher_template::gen_launcher_main_rs(
        &tarball_path.to_string_lossy(),
        &real_binary_path.to_string_lossy(),
        "abc123def4567890",
        None,
    );
    let cargo_toml = launcher_template::gen_launcher_cargo_toml("smoke_launcher");

    std::fs::write(dir.join("src").join("main.rs"), main_rs).expect("write main.rs");
    std::fs::write(dir.join("Cargo.toml"), cargo_toml).expect("write Cargo.toml");

    // cargo check: parsea + chequea tipos, sin emit del binario.
    // ~5x más rápido que cargo build full.
    let output = Command::new("cargo")
        .args(["check", "--release", "--quiet"])
        .current_dir(&dir)
        .output()
        .expect("invocar cargo check");

    assert!(
        output.status.success(),
        "el template del launcher NO compila como Rust válido:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn template_launcher_compiles_with_windows_path_and_spaces() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    let dir = std::env::temp_dir().join("fitz-launcher-windows-paths-smoke");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("crear src dir");

    // Sub-dir con espacio (caso típico Windows: "Program Files").
    let sub_with_space = dir.join("with space");
    std::fs::create_dir_all(&sub_with_space).expect("crear subdir con espacio");

    let tarball_path = sub_with_space.join("tarball.tar.gz");
    let real_binary_path = sub_with_space.join("fitz-real.exe");
    std::fs::write(&tarball_path, b"dummy").expect("write tarball");
    std::fs::write(&real_binary_path, b"dummy").expect("write real");

    let main_rs = launcher_template::gen_launcher_main_rs(
        &tarball_path.to_string_lossy(),
        &real_binary_path.to_string_lossy(),
        "0123456789abcdef",
        None,
    );
    let cargo_toml = launcher_template::gen_launcher_cargo_toml("smoke_win");

    std::fs::write(dir.join("src").join("main.rs"), main_rs).expect("write main.rs");
    std::fs::write(dir.join("Cargo.toml"), cargo_toml).expect("write Cargo.toml");

    let output = Command::new("cargo")
        .args(["check", "--release", "--quiet"])
        .current_dir(&dir)
        .output()
        .expect("invocar cargo check");

    assert!(
        output.status.success(),
        "el template del launcher con path con espacios NO compila:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
