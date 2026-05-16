// cli_e2e.rs — Tests integration de Fase 9.y.1 (`fitz new` / `fitz init`).
//
// Invocan el bin `fitz` real (no la lib) con `current_dir(tempdir)`
// para que el scaffolding se materialice adentro del temp y no
// contamine el repo. Validan archivos creados, contenido del
// manifest, exit codes, y casos de error.

use std::path::Path;
use std::process::Command;

fn fitz_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fitz")
}

/// Helper: corre el bin `fitz` con args dados en el `current_dir`
/// indicado. Devuelve (stdout, stderr, exit_code).
fn run_fitz(args: &[&str], cwd: &Path) -> (String, String, i32) {
    let output = Command::new(fitz_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("invocar fitz");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn new_crea_estructura_completa_default_cli() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) =
        run_fitz(&["new", "mi-app", "--no-git"], tmp.path());
    assert_eq!(code, 0, "stderr: {stderr}");

    let project = tmp.path().join("mi-app");
    assert!(project.is_dir(), "carpeta del proyecto no creada");
    assert!(project.join("fitz.toml").is_file(), "fitz.toml falta");
    assert!(
        project.join("src").join("main.fitz").is_file(),
        "src/main.fitz falta"
    );
    assert!(project.join(".gitignore").is_file(), ".gitignore falta");

    // Manifest tiene los campos esperados.
    let manifest_text = std::fs::read_to_string(project.join("fitz.toml")).unwrap();
    assert!(manifest_text.contains("name = \"mi-app\""));
    assert!(manifest_text.contains("version = \"0.1.0\""));
    assert!(manifest_text.contains("edition = \"2026\""));
    assert!(manifest_text.contains("main = \"src/main.fitz\""));

    // Template CLI usa print top-level (sin @get/@server).
    let main_text = std::fs::read_to_string(project.join("src").join("main.fitz")).unwrap();
    assert!(main_text.contains("print("));
    assert!(main_text.contains("mi-app"));
    assert!(!main_text.contains("@get"));
    assert!(!main_text.contains("@server"));
}

#[test]
fn new_con_http_usa_template_http() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) =
        run_fitz(&["new", "mi-http", "--http", "--no-git"], tmp.path());
    assert_eq!(code, 0, "stderr: {stderr}");

    let main_text = std::fs::read_to_string(
        tmp.path().join("mi-http").join("src").join("main.fitz"),
    )
    .unwrap();
    assert!(main_text.contains("@get(\"/\")"));
    assert!(main_text.contains("@server(3000)"));
    assert!(main_text.contains("mi-http"));
}

#[test]
fn new_sin_no_git_inicializa_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, _stderr, code) = run_fitz(&["new", "mi-git"], tmp.path());
    assert_eq!(code, 0);
    // `.git/` solo existe si `git init` corrió. En CI sin git el comando
    // emite warning pero no aborta; en este test asumimos git instalado
    // (los devs de Fitz tienen git).
    assert!(
        tmp.path().join("mi-git").join(".git").is_dir(),
        ".git/ no se creó — ¿git está instalado en el PATH?"
    );
}

#[test]
fn new_con_no_git_no_inicializa_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, _stderr, code) = run_fitz(&["new", "mi-nogit", "--no-git"], tmp.path());
    assert_eq!(code, 0);
    assert!(!tmp.path().join("mi-nogit").join(".git").exists());
}

#[test]
fn new_aborta_si_carpeta_ya_existe() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("existente")).unwrap();
    let (_stdout, stderr, code) =
        run_fitz(&["new", "existente", "--no-git"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("ya existe"), "stderr: {stderr}");
}

#[test]
fn new_aborta_con_nombre_invalido() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["new", "Foo", "--no-git"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("nombre inválido"), "stderr: {stderr}");
    assert!(stderr.contains("Foo"), "stderr no menciona el nombre: {stderr}");
}

#[test]
fn init_usa_nombre_del_directorio_actual() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("mi-init");
    std::fs::create_dir_all(&project).unwrap();
    let (_stdout, stderr, code) = run_fitz(&["init", "--no-git"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let manifest_text = std::fs::read_to_string(project.join("fitz.toml")).unwrap();
    assert!(manifest_text.contains("name = \"mi-init\""));
}

#[test]
fn init_con_name_override_ignora_directorio() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("Dir-Invalido");
    std::fs::create_dir_all(&project).unwrap();
    let (_stdout, stderr, code) =
        run_fitz(&["init", "--name", "renamed-app", "--no-git"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let manifest_text = std::fs::read_to_string(project.join("fitz.toml")).unwrap();
    assert!(manifest_text.contains("name = \"renamed-app\""));
}

#[test]
fn init_aborta_si_manifest_ya_existe() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("mi-app");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("fitz.toml"), "[package]\nname = \"x\"\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["init", "--no-git"], &project);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("ya existe"),
        "stderr: {stderr}"
    );
}

#[test]
fn init_aborta_si_directorio_tiene_nombre_invalido_sin_override() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("Dir-Con-Mayusculas");
    std::fs::create_dir_all(&project).unwrap();
    let (_stdout, stderr, code) = run_fitz(&["init", "--no-git"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("nombre inválido"), "stderr: {stderr}");
    assert!(stderr.contains("--name"), "stderr no sugiere --name: {stderr}");
}

#[test]
fn programa_generado_por_new_corre_con_fitz_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, _stderr, code) =
        run_fitz(&["new", "demo-app", "--no-git"], tmp.path());
    assert_eq!(code, 0);

    let main_fitz = tmp.path().join("demo-app").join("src").join("main.fitz");
    let output = Command::new(fitz_bin())
        .args(["run"])
        .arg(&main_fitz)
        .output()
        .expect("fitz run");
    assert!(
        output.status.success(),
        "fitz run sobre el template falló:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Hola desde demo-app"),
        "stdout inesperado: {stdout}"
    );
}
