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
fn new_creates_complete_default_cli_structure() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["new", "mi-app", "--no-git"], tmp.path());
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
fn new_with_http_uses_http_template() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["new", "mi-http", "--http", "--no-git"], tmp.path());
    assert_eq!(code, 0, "stderr: {stderr}");

    let main_text =
        std::fs::read_to_string(tmp.path().join("mi-http").join("src").join("main.fitz")).unwrap();
    assert!(main_text.contains("@get(\"/\")"));
    assert!(main_text.contains("@server(3000)"));
    assert!(main_text.contains("mi-http"));
}

#[test]
fn new_without_no_git_initializes_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, _stderr, code) = run_fitz(&["new", "mi-git"], tmp.path());
    assert_eq!(code, 0);
    // `.git/` solo existe si `git init` corrió. En CI sin git el comando
    // emite warning pero no aborta; en este test asumimos git instalado
    // (los devs de Fitz tienen git).
    assert!(
        tmp.path().join("mi-git").join(".git").is_dir(),
        ".git/ was not created — is git installed in the PATH?"
    );
}

#[test]
fn new_with_no_git_does_not_initialize_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, _stderr, code) = run_fitz(&["new", "mi-nogit", "--no-git"], tmp.path());
    assert_eq!(code, 0);
    assert!(!tmp.path().join("mi-nogit").join(".git").exists());
}

#[test]
fn new_aborts_if_folder_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("existente")).unwrap();
    let (_stdout, stderr, code) = run_fitz(&["new", "existente", "--no-git"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("already exists"), "stderr: {stderr}");
}

#[test]
fn new_aborts_with_invalid_name() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["new", "Foo", "--no-git"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("invalid name"), "stderr: {stderr}");
    assert!(
        stderr.contains("Foo"),
        "stderr no menciona el nombre: {stderr}"
    );
}

#[test]
fn init_uses_current_directory_name() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("mi-init");
    std::fs::create_dir_all(&project).unwrap();
    let (_stdout, stderr, code) = run_fitz(&["init", "--no-git"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let manifest_text = std::fs::read_to_string(project.join("fitz.toml")).unwrap();
    assert!(manifest_text.contains("name = \"mi-init\""));
}

#[test]
fn init_with_name_override_ignores_directory() {
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
fn init_aborts_if_manifest_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("mi-app");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("fitz.toml"), "[package]\nname = \"x\"\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["init", "--no-git"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("already exists"), "stderr: {stderr}");
}

#[test]
fn init_aborts_if_directory_has_invalid_name_without_override() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("Dir-Con-Mayusculas");
    std::fs::create_dir_all(&project).unwrap();
    let (_stdout, stderr, code) = run_fitz(&["init", "--no-git"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("invalid name"), "stderr: {stderr}");
    assert!(
        stderr.contains("--name"),
        "stderr no sugiere --name: {stderr}"
    );
}

#[test]
fn program_generated_by_new_runs_with_fitz_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, _stderr, code) = run_fitz(&["new", "demo-app", "--no-git"], tmp.path());
    assert_eq!(code, 0);

    let main_fitz = tmp.path().join("demo-app").join("src").join("main.fitz");
    let output = Command::new(fitz_bin())
        .args(["run"])
        .arg(&main_fitz)
        .output()
        .expect("fitz run");
    assert!(
        output.status.success(),
        "fitz run over the template failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Hello from demo-app"),
        "unexpected stdout: {stdout}"
    );
}

// ---- Fase 9.y.2 — `fitz run`/`build`/`check` integrados con manifest ----

/// Helper: crea un proyecto con `fitz new <name> --no-git` adentro de
/// `tmp` y devuelve el path del proyecto.
fn create_project(tmp_root: &Path, name: &str) -> std::path::PathBuf {
    let (_stdout, stderr, code) = run_fitz(&["new", name, "--no-git"], tmp_root);
    assert_eq!(code, 0, "fitz new failed: {stderr}");
    tmp_root.join(name)
}

#[test]
fn run_without_args_inside_project_executes_bin_main() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "run-test");
    let (stdout, stderr, code) = run_fitz(&["run"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("Hello from run-test"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn check_without_args_inside_project_checks_bin_main() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "check-test");
    let (stdout, _stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("no type errors"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn run_camina_hacia_arriba_buscando_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "walk-test");
    let nested = project.join("subdir").join("more");
    std::fs::create_dir_all(&nested).unwrap();
    let (stdout, stderr, code) = run_fitz(&["run"], &nested);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("Hello from walk-test"), "stdout: {stdout}");
}

#[test]
fn run_with_explicit_file_ignores_manifest_and_runs_in_single_file_mode() {
    // El archivo explícito es de un proyecto ajeno: el cwd está adentro
    // de `walk-test` pero pasamos un .fitz que vive en otro lado.
    let tmp = tempfile::tempdir().unwrap();
    let _project = create_project(tmp.path(), "walk-test");
    let other_dir = tmp.path().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other_fitz = other_dir.join("script.fitz");
    std::fs::write(&other_fitz, "print(\"single-file mode OK\")\n").unwrap();

    let output = Command::new(fitz_bin())
        .args(["run"])
        .arg(&other_fitz)
        .current_dir(tmp.path().join("walk-test"))
        .output()
        .expect("fitz run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("single-file mode OK"), "stdout: {stdout}");
}

#[test]
fn run_without_manifest_and_file_aborts_with_clear_message() {
    let tmp = tempfile::tempdir().unwrap();
    // No creamos proyecto: el tempdir está vacío.
    let (_stdout, stderr, code) = run_fitz(&["run"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("fitz.toml"), "stderr: {stderr}");
    assert!(stderr.contains("fitz new"), "stderr: {stderr}");
}

#[test]
fn check_without_manifest_and_file_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["check"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("fitz.toml"), "stderr: {stderr}");
}

#[test]
fn manifest_without_bin_section_aborts_with_clear_message() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("fitz.toml"),
        "[package]\nname = \"sin-bin\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let (_stdout, stderr, code) = run_fitz(&["run"], tmp.path());
    assert_eq!(code, 1);
    assert!(
        stderr.contains("[bin]"),
        "stderr no menciona [bin]: {stderr}"
    );
}

#[test]
fn corrupt_manifest_aborts_with_clear_message() {
    let tmp = tempfile::tempdir().unwrap();
    // TOML malformado: cierra una tabla sin abrir.
    std::fs::write(tmp.path().join("fitz.toml"), "this is = = not toml\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["run"], tmp.path());
    assert_eq!(code, 1);
    // El mensaje viene de ManifestError::Parse (toml::de::Error).
    assert!(
        stderr.contains("parseando manifest") || stderr.contains("fitz.toml"),
        "stderr no menciona parsing: {stderr}"
    );
}

// ---- Fase 9.y.3.a — path deps + lockfile ----

/// Helper: convierte un proyecto `path/to/proj` (creado con `fitz new`)
/// de binary a library reescribiendo `fitz.toml` con `[lib]` en vez
/// de `[bin]`. También escribe un `src/lib.fitz` mínimo.
fn convert_to_lib(project_dir: &Path, name: &str, version: &str) {
    let lib_manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2026\"\n\n[lib]\nentry = \"src/lib.fitz\"\n"
    );
    std::fs::write(project_dir.join("fitz.toml"), lib_manifest).unwrap();
    std::fs::create_dir_all(project_dir.join("src")).unwrap();
    std::fs::write(
        project_dir.join("src").join("lib.fitz"),
        "// minimal lib for tests\nfn helper(x: Int) -> Int => x + 1\n",
    )
    .unwrap();
}

/// Helper: reescribe el `fitz.toml` de un proyecto agregando una sección
/// `[dependencies]` con un path dep.
fn add_path_dep(project_dir: &Path, app_name: &str, dep_name: &str, rel_path: &str) {
    let manifest = format!(
        "[package]\nname = \"{app_name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n\n[dependencies]\n{dep_name} = {{ path = \"{rel_path}\" }}\n"
    );
    std::fs::write(project_dir.join("fitz.toml"), manifest).unwrap();
}

#[test]
fn check_with_path_dep_emits_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = tmp.path().join("utils-lib");
    let _ = run_fitz(&["new", "utils-lib", "--no-git"], tmp.path());
    convert_to_lib(&lib_dir, "utils-lib", "0.2.0");

    let app_dir = tmp.path().join("app");
    let _ = run_fitz(&["new", "app", "--no-git"], tmp.path());
    add_path_dep(&app_dir, "app", "utils-lib", "../utils-lib");

    let (stdout, stderr, code) = run_fitz(&["check"], &app_dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("actualizado"), "stdout: {stdout}");

    let lock_path = app_dir.join("fitz.lock");
    assert!(lock_path.is_file(), "lockfile no existe");
    let lock_text = std::fs::read_to_string(&lock_path).unwrap();
    assert!(lock_text.contains("version = 1"));
    assert!(lock_text.contains("name = \"utils-lib\""));
    assert!(lock_text.contains("version = \"0.2.0\""));
    // Path deps NO tienen source en el lockfile (Cargo convention).
    assert!(!lock_text.contains("source"), "lockfile: {lock_text}");
}

#[test]
fn check_is_idempotent_without_rewriting_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = tmp.path().join("utils-lib");
    let _ = run_fitz(&["new", "utils-lib", "--no-git"], tmp.path());
    convert_to_lib(&lib_dir, "utils-lib", "0.2.0");

    let app_dir = tmp.path().join("app");
    let _ = run_fitz(&["new", "app", "--no-git"], tmp.path());
    add_path_dep(&app_dir, "app", "utils-lib", "../utils-lib");

    // Primera corrida escribe el lockfile.
    let (stdout1, _, _) = run_fitz(&["check"], &app_dir);
    assert!(stdout1.contains("actualizado"));

    // Segunda corrida no re-escribe (mismo contenido).
    let (stdout2, _, _) = run_fitz(&["check"], &app_dir);
    assert!(
        !stdout2.contains("actualizado"),
        "second run should not have notified update: {stdout2}"
    );
}

#[test]
fn lockfile_regenerates_when_dep_changes_version() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = tmp.path().join("utils-lib");
    let _ = run_fitz(&["new", "utils-lib", "--no-git"], tmp.path());
    convert_to_lib(&lib_dir, "utils-lib", "0.2.0");

    let app_dir = tmp.path().join("app");
    let _ = run_fitz(&["new", "app", "--no-git"], tmp.path());
    add_path_dep(&app_dir, "app", "utils-lib", "../utils-lib");

    let _ = run_fitz(&["check"], &app_dir);

    // Cambiar la versión de la dep.
    convert_to_lib(&lib_dir, "utils-lib", "0.5.1");

    let (stdout, _, code) = run_fitz(&["check"], &app_dir);
    assert_eq!(code, 0);
    assert!(stdout.contains("actualizado"), "stdout: {stdout}");
    let lock_text = std::fs::read_to_string(app_dir.join("fitz.lock")).unwrap();
    assert!(
        lock_text.contains("version = \"0.5.1\""),
        "lock: {lock_text}"
    );
}

#[test]
fn project_without_deps_does_not_emit_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "no-deps");
    let (_stdout, _stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 0);
    assert!(
        !project.join("fitz.lock").exists(),
        "fitz.lock should not have been created for project without deps"
    );
}

#[test]
fn short_version_aborts_citing_9y5() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "ver-app");
    std::fs::write(
        project.join("fitz.toml"),
        "[package]\nname = \"ver-app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n\n[dependencies]\nfoo = \"1.0.0\"\n",
    )
    .unwrap();
    let (_stdout, stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("9.y.5"), "stderr: {stderr}");
}

#[test]
fn git_dep_without_tag_or_rev_aborts_requesting_one() {
    // Pre-9.y.3.c este test verificaba que git deps eran rechazadas
    // wholesale citando 9.y.3.c. Post-cierre: git deps SÍ se aceptan,
    // pero requieren `tag` o `rev` explícito por reproducibilidad
    // (no `branch`). Esta versión del test asegura el mensaje
    // accionable.
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "git-app");
    std::fs::write(
        project.join("fitz.toml"),
        "[package]\nname = \"git-app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n\n[dependencies]\nhelpers = { git = \"https://github.com/foo/bar\" }\n",
    )
    .unwrap();
    let (_stdout, stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("tag") && stderr.contains("rev"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("reproducibility"), "stderr: {stderr}");
}

#[test]
fn nonexistent_path_dep_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "missing-app");
    add_path_dep(&project, "missing-app", "ghost", "../no-existe");
    let (_stdout, stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("does not exist"), "stderr: {stderr}");
}

#[test]
fn path_dep_without_lib_aborts_with_suggestion() {
    let tmp = tempfile::tempdir().unwrap();
    // Dep es un proyecto solo-bin (no library).
    let _ = run_fitz(&["new", "solo-bin", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("app");
    let _ = run_fitz(&["new", "app", "--no-git"], tmp.path());
    add_path_dep(&app_dir, "app", "solo-bin", "../solo-bin");

    let (_stdout, stderr, code) = run_fitz(&["check"], &app_dir);
    assert_eq!(code, 1);
    assert!(stderr.contains("[lib]"), "stderr: {stderr}");
    assert!(stderr.contains("entry"), "stderr: {stderr}");
}

// ---- Fase 9.y.3.b — Loader integration (deps usables desde código) ----

/// Helper: setup mínimo de proyecto importer + lib. Crea
/// `<tmp>/<lib>/` con `[lib] entry = "src/lib.fitz"` exponiendo las
/// funciones del template; crea `<tmp>/<app>/` con `[dependencies]`
/// apuntando al lib + `src/main.fitz` con `from <lib> import ...`.
/// Devuelve el path del importer.
fn setup_dep_project(
    tmp: &Path,
    lib_name: &str,
    lib_version: &str,
    lib_body: &str,
    app_name: &str,
    app_body: &str,
) -> std::path::PathBuf {
    // Lib.
    let _ = run_fitz(&["new", lib_name, "--no-git"], tmp);
    let lib_dir = tmp.join(lib_name);
    convert_to_lib(&lib_dir, lib_name, lib_version);
    std::fs::write(lib_dir.join("src").join("lib.fitz"), lib_body).unwrap();

    // App.
    let _ = run_fitz(&["new", app_name, "--no-git"], tmp);
    let app_dir = tmp.join(app_name);
    add_path_dep(&app_dir, app_name, lib_name, &format!("../{lib_name}"));
    std::fs::write(app_dir.join("src").join("main.fitz"), app_body).unwrap();
    app_dir
}

#[test]
fn run_resolves_from_dep_import_via_dep_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = setup_dep_project(
        tmp.path(),
        "myutils",
        "0.1.0",
        "fn double(x: Int) -> Int => x * 2\nfn greet(name: Str) -> Str => \"hola {name}\"\n",
        "myapp",
        "from myutils import double, greet\nprint(\"d={double(21)}\")\nprint(greet(\"Patagonia\"))\n",
    );

    let (stdout, stderr, code) = run_fitz(&["run"], &app_dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("d=42"), "stdout: {stdout}");
    assert!(stdout.contains("hola Patagonia"), "stdout: {stdout}");
}

#[test]
fn unreferenced_run_dep_does_not_fail_if_not_imported() {
    // Setup proyecto con dep en manifest pero main.fitz no la importa.
    // El lockfile se emite igual, el programa corre sin tocar la dep.
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = setup_dep_project(
        tmp.path(),
        "unused-lib",
        "0.1.0",
        "fn helper() -> Int => 1\n",
        "myapp",
        "print(\"sin imports\")\n",
    );
    let (stdout, _stderr, code) = run_fitz(&["run"], &app_dir);
    assert_eq!(code, 0);
    assert!(stdout.contains("sin imports"));
    assert!(
        app_dir.join("fitz.lock").is_file(),
        "lockfile should have been emitted"
    );
}

#[test]
fn run_local_fitz_not_shadowed_by_nonexistent_file() {
    // Confirma que el fallback path-relativo sigue funcionando cuando
    // el segment NO matchea ninguna dep. Importer importa un módulo
    // local `utils.fitz` que vive en `<app>/src/`. Sin deps en el
    // manifest, sin shadowing — comportamiento single-file mode
    // dentro de un proyecto.
    let tmp = tempfile::tempdir().unwrap();
    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    // Módulo local utils.fitz adyacente al main.
    std::fs::write(
        app_dir.join("src").join("utils.fitz"),
        "fn triple(x: Int) -> Int => x * 3\n",
    )
    .unwrap();
    std::fs::write(
        app_dir.join("src").join("main.fitz"),
        "from utils import triple\nprint(\"t={triple(7)}\")\n",
    )
    .unwrap();
    let (stdout, stderr, code) = run_fitz(&["run"], &app_dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("t=21"), "stdout: {stdout}");
}

#[test]
fn run_dep_shadows_local_file_with_same_name() {
    // Decisión documentada: si hay `[dependencies] foo = { path = ... }`
    // Y un `src/foo.fitz` local, la dep gana. Verificamos el behavior.
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = setup_dep_project(
        tmp.path(),
        "shared",
        "0.1.0",
        "fn ping() -> Str => \"DEP\"\n",
        "myapp",
        "from shared import ping\nprint(ping())\n",
    );
    // Crear un `src/shared.fitz` local con OTRO `ping()`.
    std::fs::write(
        app_dir.join("src").join("shared.fitz"),
        "fn ping() -> Str => \"LOCAL\"\n",
    )
    .unwrap();

    let (stdout, stderr, code) = run_fitz(&["run"], &app_dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("DEP"),
        "la dep debe ganar sobre el archivo local: {stdout}"
    );
    assert!(
        !stdout.contains("LOCAL"),
        "el archivo local no debe haberse cargado: {stdout}"
    );
}

// ---- Fase 9.y.3.c — Git deps + cache local ----

/// Helper: convierte un directorio `<dir>` ya armado como library
/// Fitz (con `fitz.toml` y `src/lib.fitz`) en un git repo con un commit
/// inicial y un tag. Devuelve el commit hash.
fn init_git_repo_with_tag(dir: &Path, tag: &str) -> String {
    let run = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("invocar git");
        assert!(
            output.status.success(),
            "git {} failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@fitz.dev"]);
    run(&["config", "user.name", "test"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "initial"]);
    run(&["tag", tag]);
    run(&["rev-parse", "HEAD"]).trim().to_string()
}

/// Helper: setup minimal git dep — crea `<tmp>/<lib>/` con `[lib]` +
/// código, lo convierte en git repo con tag; crea `<tmp>/<app>/` con
/// `[dependencies] <lib> = { git = "file://<lib-path>", tag = "<tag>" }`
/// + `src/main.fitz`. Devuelve `(app_dir, cache_dir, commit_hash)`.
///
/// El cache_dir es un tempdir aislado vía `FITZ_CACHE_DIR`.
fn setup_git_dep_project(
    tmp_root: &Path,
    lib_name: &str,
    lib_version: &str,
    lib_body: &str,
    tag: &str,
    app_name: &str,
    app_body: &str,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    // 1. Crear lib + convertir a git repo.
    let _ = run_fitz(&["new", lib_name, "--no-git"], tmp_root);
    let lib_dir = tmp_root.join(lib_name);
    convert_to_lib(&lib_dir, lib_name, lib_version);
    std::fs::write(lib_dir.join("src").join("lib.fitz"), lib_body).unwrap();
    let commit = init_git_repo_with_tag(&lib_dir, tag);

    // 2. Crear app con dep git al file:// URL del repo local.
    let _ = run_fitz(&["new", app_name, "--no-git"], tmp_root);
    let app_dir = tmp_root.join(app_name);

    // file:// URL — git acepta paths absolutos directos en todas las
    // plataformas pero el formato canónico con file:/// trabaja
    // uniforme en Linux/Mac/Windows. Convertimos backslashes a forward
    // para que la URL sea legal.
    let lib_path_str = lib_dir.to_string_lossy().replace('\\', "/");
    let git_url = format!("file:///{}", lib_path_str.trim_start_matches('/'));

    let manifest = format!(
        "[package]\nname = \"{app_name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [bin]\nmain = \"src/main.fitz\"\n\n\
         [dependencies]\n{lib_name} = {{ git = \"{git_url}\", tag = \"{tag}\" }}\n"
    );
    std::fs::write(app_dir.join("fitz.toml"), manifest).unwrap();
    std::fs::write(app_dir.join("src").join("main.fitz"), app_body).unwrap();

    // 3. Cache dir aislado en otro subdir del tmp.
    let cache_dir = tmp_root.join(".fitz-cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    (app_dir, cache_dir, commit)
}

/// Helper: corre fitz en `cwd` con `FITZ_CACHE_DIR` apuntando a
/// `cache_dir`. Necesario para que git deps no toquen el cache global
/// del usuario.
fn run_fitz_with_cache(args: &[&str], cwd: &Path, cache_dir: &Path) -> (String, String, i32) {
    let output = Command::new(fitz_bin())
        .args(args)
        .current_dir(cwd)
        .env("FITZ_CACHE_DIR", cache_dir)
        .output()
        .expect("invocar fitz");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn git_dep_clones_to_cache_and_emits_lockfile_with_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let (app_dir, cache_dir, commit) = setup_git_dep_project(
        tmp.path(),
        "myutils",
        "0.1.0",
        "fn double(x: Int) -> Int => x * 2\n",
        "v0.1.0",
        "myapp",
        "from myutils import double\nprint(\"d={double(21)}\")\n",
    );

    let (stdout, stderr, code) = run_fitz_with_cache(&["run"], &app_dir, &cache_dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("d=42"), "stdout: {stdout}");

    // Cache directory creado con el naming esperado.
    let git_cache = cache_dir.join("git");
    assert!(git_cache.is_dir(), "cache/git no existe");
    let entries: Vec<_> = std::fs::read_dir(&git_cache).unwrap().collect();
    assert_eq!(
        entries.len(),
        1,
        "se esperaba un dir clonado, hay: {entries:?}"
    );

    // Lockfile incluye source con el commit hash exacto.
    let lockfile = std::fs::read_to_string(app_dir.join("fitz.lock")).unwrap();
    assert!(lockfile.contains("name = \"myutils\""));
    assert!(
        lockfile.contains(&format!("#{commit}")),
        "lockfile no incluye el commit hash {commit}: {lockfile}"
    );
    assert!(lockfile.contains("source = \"git+"), "lockfile: {lockfile}");
}

#[test]
fn git_dep_reuses_cache_without_re_cloning() {
    let tmp = tempfile::tempdir().unwrap();
    let (app_dir, cache_dir, _commit) = setup_git_dep_project(
        tmp.path(),
        "reused",
        "0.1.0",
        "fn helper() -> Int => 7\n",
        "v0.1.0",
        "myapp",
        "from reused import helper\nprint(\"h={helper()}\")\n",
    );

    // Primera corrida: clona.
    let (_, _, code1) = run_fitz_with_cache(&["run"], &app_dir, &cache_dir);
    assert_eq!(code1, 0);

    // Marcar el cache dir con un mtime de referencia: tocamos un
    // archivo dentro para luego verificar que NO se sobrescribió.
    let git_cache = cache_dir.join("git");
    let clone_dir = std::fs::read_dir(&git_cache)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let marker = clone_dir.join("FITZ_TEST_MARKER");
    std::fs::write(&marker, "no me toques").unwrap();

    // Segunda corrida: debe REUSAR cache (el marker debe persistir).
    let (_, _, code2) = run_fitz_with_cache(&["run"], &app_dir, &cache_dir);
    assert_eq!(code2, 0);

    assert!(
        marker.is_file(),
        "the marker was deleted — the cache was re-cloned instead of reused"
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "no me toques",
        "el marker fue sobrescrito — re-clone destructivo"
    );
}

#[test]
fn git_dep_lockfile_idempotent_if_commit_does_not_change() {
    let tmp = tempfile::tempdir().unwrap();
    let (app_dir, cache_dir, _commit) = setup_git_dep_project(
        tmp.path(),
        "idem",
        "0.1.0",
        "fn helper() -> Int => 1\n",
        "v0.1.0",
        "myapp",
        "print(\"ok\")\n",
    );

    // Primera corrida: emite lockfile.
    let (stdout1, _, _) = run_fitz_with_cache(&["check"], &app_dir, &cache_dir);
    assert!(stdout1.contains("actualizado"), "stdout1: {stdout1}");

    // Segunda corrida: lockfile ya está sync, no debe notificar
    // "actualizado".
    let (stdout2, _, _) = run_fitz_with_cache(&["check"], &app_dir, &cache_dir);
    assert!(
        !stdout2.contains("actualizado"),
        "lockfile should not have been rewritten: {stdout2}"
    );
}

#[test]
fn git_dep_nonexistent_tag_aborts_with_git_message() {
    let tmp = tempfile::tempdir().unwrap();
    // Crear repo con tag v0.1.0 pero pedir tag v9.9.9 que no existe.
    let lib_dir = tmp.path().join("realib");
    std::fs::create_dir_all(lib_dir.join("src")).unwrap();
    std::fs::write(
        lib_dir.join("fitz.toml"),
        "[package]\nname = \"realib\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[lib]\nentry = \"src/lib.fitz\"\n",
    )
    .unwrap();
    std::fs::write(lib_dir.join("src").join("lib.fitz"), "fn x() -> Int => 1\n").unwrap();
    init_git_repo_with_tag(&lib_dir, "v0.1.0");

    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    let lib_path_str = lib_dir.to_string_lossy().replace('\\', "/");
    let git_url = format!("file:///{}", lib_path_str.trim_start_matches('/'));
    let manifest = format!(
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [bin]\nmain = \"src/main.fitz\"\n\n\
         [dependencies]\nrealib = {{ git = \"{git_url}\", tag = \"v9.9.9\" }}\n"
    );
    std::fs::write(app_dir.join("fitz.toml"), manifest).unwrap();
    let cache_dir = tmp.path().join(".fitz-cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let (_, stderr, code) = run_fitz_with_cache(&["check"], &app_dir, &cache_dir);
    assert_eq!(code, 1);
    // El mensaje debería citar `git clone` failing.
    assert!(
        stderr.to_lowercase().contains("git"),
        "stderr no menciona git: {stderr}"
    );
}

/// Build sin args en manifest mode: el codegen carga la dep del
/// dep_registry y compila ambos módulos en un Cargo project unificado.
/// Test pesado (~3-5s) porque invoca rustc real para 2 archivos `.fitz`.
#[test]
fn build_resolves_from_dep_import_via_dep_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = setup_dep_project(
        tmp.path(),
        "myutils",
        "0.1.0",
        "fn double(x: Int) -> Int => x * 2\n",
        "myapp",
        "from myutils import double\nprint(\"d={double(21)}\")\n",
    );

    let (_stdout, stderr, code) = run_fitz(&["build"], &app_dir);
    assert_eq!(code, 0, "stderr: {stderr}");

    let bin_name = if cfg!(windows) { "myapp.exe" } else { "myapp" };
    let bin_path = app_dir.join("target").join("release").join(bin_name);
    assert!(
        bin_path.is_file(),
        "binary does not exist at {}",
        bin_path.display()
    );

    let output = Command::new(&bin_path).output().expect("execute binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("d=42"), "stdout del binario: {stdout}");
}

// ---- Fase 9.y.4 — `fitz add` / `fitz remove` / `fitz update` ----

#[test]
fn add_path_dep_modifies_manifest_and_emits_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = tmp.path().join("utils-lib");
    let _ = run_fitz(&["new", "utils-lib", "--no-git"], tmp.path());
    convert_to_lib(&lib_dir, "utils-lib", "0.1.0");

    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");

    let (stdout, stderr, code) =
        run_fitz(&["add", "utils-lib", "--path", "../utils-lib"], &app_dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("agregado"), "stdout: {stdout}");

    let manifest = std::fs::read_to_string(app_dir.join("fitz.toml")).unwrap();
    assert!(
        manifest.contains("utils-lib = { path = \"../utils-lib\" }"),
        "manifest:\n{manifest}"
    );
    let lockfile = std::fs::read_to_string(app_dir.join("fitz.lock")).unwrap();
    assert!(
        lockfile.contains("name = \"utils-lib\""),
        "lockfile:\n{lockfile}"
    );
}

#[test]
fn add_without_flags_aborts_requesting_path_or_git() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "myapp");
    let (_stdout, stderr, code) = run_fitz(&["add", "foo"], &project);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("--path") && stderr.contains("--git"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("9.y.5"),
        "stderr should mention the registry: {stderr}"
    );
}

#[test]
fn add_git_without_tag_or_rev_aborts_requesting_one() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "myapp");
    let (_stdout, stderr, code) = run_fitz(&["add", "foo", "--git", "https://x.com/r"], &project);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("tag") && stderr.contains("rev"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("reproducibility"), "stderr: {stderr}");
}

#[test]
fn add_path_and_git_together_aborts_with_clap_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "myapp");
    let (_stdout, stderr, code) = run_fitz(
        &["add", "foo", "--path", "../x", "--git", "https://y.com/z"],
        &project,
    );
    assert_eq!(code, 2); // clap exit code 2 para argument conflicts.
    assert!(stderr.contains("cannot be used"), "stderr: {stderr}");
}

#[test]
fn add_outside_project_aborts_with_clear_message() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["add", "foo", "--path", "../x"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("fitz.toml"), "stderr: {stderr}");
    assert!(stderr.contains("fitz new"), "stderr: {stderr}");
}

#[test]
fn add_overwrites_existing_dep() {
    let tmp = tempfile::tempdir().unwrap();
    let _ = run_fitz(&["new", "lib-uno", "--no-git"], tmp.path());
    convert_to_lib(&tmp.path().join("lib-uno"), "lib-uno", "0.1.0");
    let _ = run_fitz(&["new", "lib-dos", "--no-git"], tmp.path());
    convert_to_lib(&tmp.path().join("lib-dos"), "lib-dos", "0.2.0");

    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");

    // Add primero, después overwrite con otro path.
    let _ = run_fitz(&["add", "compartido", "--path", "../lib-uno"], &app_dir);
    let (_stdout, _stderr, code) =
        run_fitz(&["add", "compartido", "--path", "../lib-dos"], &app_dir);
    assert_eq!(code, 0);

    let manifest = std::fs::read_to_string(app_dir.join("fitz.toml")).unwrap();
    assert!(
        manifest.contains("compartido = { path = \"../lib-dos\" }"),
        "manifest:\n{manifest}"
    );
    assert!(
        !manifest.contains("../lib-uno"),
        "the old path should not have persisted:\n{manifest}"
    );
}

#[test]
fn remove_existing_dep_removes_and_updates_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let _ = run_fitz(&["new", "u", "--no-git"], tmp.path());
    convert_to_lib(&tmp.path().join("u"), "u", "0.1.0");
    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    let _ = run_fitz(&["add", "u", "--path", "../u"], &app_dir);
    assert!(app_dir.join("fitz.lock").is_file());

    let (stdout, stderr, code) = run_fitz(&["remove", "u"], &app_dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("quitada"), "stdout: {stdout}");

    let manifest = std::fs::read_to_string(app_dir.join("fitz.toml")).unwrap();
    assert!(!manifest.contains("u = "), "manifest:\n{manifest}");
    // Como era la única dep, [dependencies] se borró y el lockfile
    // también (deps vacías = sin lockfile).
    assert!(!manifest.contains("[dependencies]"));
    assert!(
        !app_dir.join("fitz.lock").exists(),
        "fitz.lock should have been deleted when no deps remained"
    );
}

#[test]
fn remove_nonexistent_dep_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "myapp");
    let (_stdout, stderr, code) = run_fitz(&["remove", "no-existe"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("no estaba"), "stderr: {stderr}");
}

#[test]
fn update_without_git_deps_reports_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let _ = run_fitz(&["new", "u", "--no-git"], tmp.path());
    convert_to_lib(&tmp.path().join("u"), "u", "0.1.0");
    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    let _ = run_fitz(&["add", "u", "--path", "../u"], &app_dir);

    let (stdout, _, code) = run_fitz(&["update"], &app_dir);
    assert_eq!(code, 0);
    assert!(stdout.contains("no git deps"), "stdout: {stdout}");
}

#[test]
fn update_invalidates_git_dep_cache_and_re_clones() {
    let tmp = tempfile::tempdir().unwrap();
    // Setup repo git con tag.
    let lib_dir = tmp.path().join("u");
    std::fs::create_dir_all(lib_dir.join("src")).unwrap();
    std::fs::write(
        lib_dir.join("fitz.toml"),
        "[package]\nname = \"u\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[lib]\nentry = \"src/lib.fitz\"\n",
    )
    .unwrap();
    std::fs::write(lib_dir.join("src/lib.fitz"), "fn x() -> Int => 1\n").unwrap();
    init_git_repo_with_tag(&lib_dir, "v0.1.0");

    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    let cache_dir = tmp.path().join(".cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let lib_path_str = lib_dir.to_string_lossy().replace('\\', "/");
    let git_url = format!("file:///{}", lib_path_str.trim_start_matches('/'));

    // Add la git dep — clona al cache.
    let (_, stderr, code) = run_fitz_with_cache(
        &["add", "u", "--git", &git_url, "--tag", "v0.1.0"],
        &app_dir,
        &cache_dir,
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    let git_cache = cache_dir.join("git");
    let cache_entries: Vec<_> = std::fs::read_dir(&git_cache).unwrap().collect();
    assert_eq!(cache_entries.len(), 1);
    let clone_dir = cache_entries[0].as_ref().unwrap().path();
    // Marker para verificar que update borra el dir.
    std::fs::write(clone_dir.join("FITZ_TEST_MARKER"), "se va a borrar").unwrap();

    let (stdout, _, code) = run_fitz_with_cache(&["update"], &app_dir, &cache_dir);
    assert_eq!(code, 0);
    assert!(stdout.contains("cache invalidated"), "stdout: {stdout}");

    // El cache fue re-clonado: el marker NO debería existir más.
    let cache_entries_after: Vec<_> = std::fs::read_dir(&git_cache).unwrap().collect();
    assert_eq!(
        cache_entries_after.len(),
        1,
        "exactly one clone should remain"
    );
    let new_clone = cache_entries_after[0].as_ref().unwrap().path();
    assert!(
        !new_clone.join("FITZ_TEST_MARKER").exists(),
        "the marker should have disappeared after re-clone"
    );
}

#[test]
fn update_nonexistent_dep_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let _ = run_fitz(&["new", "u", "--no-git"], tmp.path());
    convert_to_lib(&tmp.path().join("u"), "u", "0.1.0");
    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    let _ = run_fitz(&["add", "u", "--path", "../u"], &app_dir);

    let (_, stderr, code) = run_fitz(&["update", "no-existe"], &app_dir);
    assert_eq!(code, 1);
    assert!(stderr.contains("is not in"), "stderr: {stderr}");
}

// ---- Fase 9.z.1.a — `fitz fmt` ----

#[test]
fn fmt_archivo_explicito_canonicaliza_indent_y_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("scratch.fitz");
    std::fs::write(
        &file,
        "fn double(n: Int) -> Int { return n * 2 }\nprint(double(21))\n",
    )
    .unwrap();

    let (_, stderr, code) = run_fitz(&["fmt"], file.parent().unwrap());
    // Sin args sin manifest debería fallar:
    assert_eq!(code, 1, "stderr: {stderr}");

    // Con archivo explícito:
    let output = Command::new(fitz_bin())
        .args(["fmt"])
        .arg(&file)
        .output()
        .expect("fitz fmt");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = std::fs::read_to_string(&file).unwrap();
    assert!(
        after.contains("fn double(n: Int) -> Int {\n    return n * 2\n}"),
        "fn did not stay multi-line with indent:\n{after}"
    );
    assert!(after.ends_with('\n'), "should end with newline");
}

#[test]
fn fmt_check_idempotent_returns_0() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("ok.fitz");
    // Texto ya en forma canónica (blank line obligatoria entre
    // fn def y stmt simple subsiguiente — ver
    // `fmt::needs_blank_line_before`).
    let canonical = "fn double(n: Int) -> Int {\n    return n * 2\n}\n\nprint(double(21))\n";
    std::fs::write(&file, canonical).unwrap();

    let output = Command::new(fitz_bin())
        .args(["fmt", "--check"])
        .arg(&file)
        .output()
        .expect("fitz fmt --check");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // El archivo NO debe haberse modificado.
    let after = std::fs::read_to_string(&file).unwrap();
    assert_eq!(after, canonical);
}

#[test]
fn fmt_check_non_canonical_returns_1_without_modifying() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("dirty.fitz");
    let dirty = "fn double(n: Int) -> Int { return n * 2 }\n";
    std::fs::write(&file, dirty).unwrap();

    let output = Command::new(fitz_bin())
        .args(["fmt", "--check"])
        .arg(&file)
        .output()
        .expect("fitz fmt --check");
    assert_eq!(output.status.code(), Some(1));

    // No debe modificar el archivo (--check es read-only).
    let after = std::fs::read_to_string(&file).unwrap();
    assert_eq!(after, dirty);
}

#[test]
fn fmt_does_not_emit_warning_post_9z1b() {
    // Fase 9.z.1.b cerró la deuda de comments — el warning loud
    // del modo write fue removido. Este test bloquea regresiones.
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("scratch.fitz");
    std::fs::write(&file, "let x = 1\n").unwrap();

    let output = Command::new(fitz_bin())
        .args(["fmt"])
        .arg(&file)
        .output()
        .expect("fitz fmt");
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("⚠"),
        "post-9.z.1.b there must be no loss warning: {stderr}"
    );
    assert!(
        !stderr.contains("9.z.1.b"),
        "post-9.z.1.b no debe mencionar la deuda como pendiente: {stderr}"
    );
}

#[test]
fn fmt_preserves_comments_and_blank_lines() {
    // Fase 9.z.1.b: el round-trip debe ser exacto incluyendo
    // comments y blank lines.
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("doc.fitz");
    let original = "// header\n\
                    \n\
                    let x = 1  // trailing\n\
                    \n\
                    // entre stmts\n\
                    let y = 2\n";
    std::fs::write(&file, original).unwrap();

    let output = Command::new(fitz_bin())
        .args(["fmt"])
        .arg(&file)
        .output()
        .expect("fitz fmt");
    assert!(output.status.success());

    let after = std::fs::read_to_string(&file).unwrap();
    assert!(
        after.contains("// header"),
        "comment header was deleted: {after}"
    );
    assert!(
        after.contains("// trailing"),
        "trailing was deleted: {after}"
    );
    assert!(
        after.contains("// entre stmts"),
        "middle comment was deleted: {after}"
    );
    // Blank line entre el header y `let x = 1` debe estar presente.
    assert!(
        after.contains("// header\n\nlet x"),
        "blank between header and let was deleted: {after}"
    );
}

#[test]
fn fmt_trailing_comment_seguido_de_bloque_no_inserta_blank_spurio() {
    // Regresión del bug fixed post-9.z.5: trailing comment al final
    // del body de un fn, seguido de OTRO bloque (for/while/if/match),
    // insertaba un blank line spurio adentro del body del segundo
    // bloque (ver `docs/deudas-post-5b.md` para el MRE).
    //
    // El fix: agregar guarda `prev_end_line > 0` al cálculo de
    // `had_blank_in_source` en `fmt_stmt_list`, paralelo a la de
    // `smart_blank`. Sin la guarda, `last_emitted_comment_line` de
    // scope outer disparaba el chequeo de blank sobre líneas FUERA
    // del bloque actual.
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("t.fitz");
    let original = "fn greet(name: Str) -> Str {\n\
                    \x20\x20\x20\x20return \"Hola, {name}!\" // inline\n\
                    }\n\
                    \n\
                    for n in [\"Ada\"] {\n\
                    \x20\x20\x20\x20print(greet(n))\n\
                    }\n";
    std::fs::write(&file, original).unwrap();

    let output = Command::new(fitz_bin())
        .args(["fmt"])
        .arg(&file)
        .output()
        .expect("fitz fmt");
    assert!(output.status.success());

    let after = std::fs::read_to_string(&file).unwrap();
    // No debe haber blank line spurio entre `for ... {` y `print(...)`.
    assert!(
        !after.contains("for n in [\"Ada\"] {\n\n"),
        "blank spurio dentro del body del for: {after}"
    );
    // Smoke: trailing comment preservado, ambos bloques presentes.
    assert!(after.contains("// inline"));
    assert!(after.contains("for n in [\"Ada\"]"));
    assert!(after.contains("print(greet(n))"));
}

#[test]
fn fmt_file_with_syntax_error_aborts_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("broken.fitz");
    let broken = "let x = (\n"; // paréntesis sin cerrar
    std::fs::write(&file, broken).unwrap();

    let output = Command::new(fitz_bin())
        .args(["fmt"])
        .arg(&file)
        .output()
        .expect("fitz fmt");
    assert_eq!(output.status.code(), Some(1));
    // El archivo no debe haberse modificado.
    let after = std::fs::read_to_string(&file).unwrap();
    assert_eq!(after, broken);
}

#[test]
fn fmt_without_args_inside_project_discovers_src_files() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "fmt-proj");

    // Tocar src/main.fitz con formato no canónico.
    let main = project.join("src").join("main.fitz");
    std::fs::write(&main, "fn x() -> Int { return 1 }\n").unwrap();

    let (_, _, code) = run_fitz(&["fmt"], &project);
    assert_eq!(code, 0);

    let after = std::fs::read_to_string(&main).unwrap();
    assert!(
        after.contains("fn x() -> Int {\n    return 1\n}"),
        "src/main.fitz was not reformatted:\n{after}"
    );
}

/// Build sin args en manifest mode: produce el binario en
/// `<manifest_dir>/target/release/<pkg-name>(.exe)` con el nombre del
/// paquete (NO el stem del fuente). Test pesado (~3s) porque invoca
/// rustc real adentro de cargo build.
#[test]
fn build_without_args_emits_to_target_release_with_pkg_name() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "build-target-test");
    let (_stdout, stderr, code) = run_fitz(&["build"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let bin_name = if cfg!(windows) {
        "build-target-test.exe"
    } else {
        "build-target-test"
    };
    let bin_path = project.join("target").join("release").join(bin_name);
    assert!(
        bin_path.exists(),
        "binary does not exist at {}",
        bin_path.display()
    );

    // Ejecutar el binario y verificar el output.
    let output = Command::new(&bin_path).output().expect("execute binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Hello from build-target-test"),
        "binary stdout: {stdout}"
    );
}

// =================================================================
// Fase 9.z.2.b — `fitz test` (testing built-in: runner + discovery)
// =================================================================

/// Helper: escribe `content` en `<root>/<rel>` (creando dirs si hace
/// falta). Útil para armar mini-proyectos en cada test.
fn write_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("crear parent");
    }
    std::fs::write(&path, content).expect("escribir archivo");
}

#[test]
fn test_single_file_runs_tests_and_reports_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "\
        @test fn suma_funciona() {\n\
            assert_eq(2 + 2, 4)\n\
        }\n\
        @test fn resta_funciona() {\n\
            assert_eq(10 - 3, 7)\n\
        }\n\
    ";
    write_file(tmp.path(), "tests.fitz", src);

    let (stdout, stderr, code) = run_fitz(&["test", "--file", "tests.fitz"], tmp.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("running 2 tests"), "stdout: {stdout}");
    assert!(stdout.contains("test suma_funciona ... ok"));
    assert!(stdout.contains("test resta_funciona ... ok"));
    assert!(stdout.contains("test result"));
    assert!(stdout.contains("2 passed"));
    assert!(stdout.contains("0 failed"));
}

#[test]
fn test_failure_returns_exit_1_with_left_right_detail() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "@test fn falla() { assert_eq(5, 10) }\n",
    );

    let (stdout, _stderr, code) = run_fitz(&["test", "--file", "t.fitz"], tmp.path());
    assert_eq!(code, 1);
    assert!(stdout.contains("test falla ... FAILED"));
    assert!(stdout.contains("left:"));
    assert!(stdout.contains("right:"));
    assert!(stdout.contains("test result"));
    assert!(stdout.contains("1 failed"));
}

#[test]
fn test_filter_substring_matches_only_what_it_contains() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "@test fn suma_basica() { assert_eq(1 + 1, 2) }\n\
         @test fn suma_negativa() { assert_eq(-1 + 1, 0) }\n\
         @test fn resta_basica() { assert_eq(3 - 1, 2) }\n",
    );

    let (stdout, _stderr, code) = run_fitz(&["test", "--file", "t.fitz", "suma"], tmp.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("running 2 tests (1 filtered out)"));
    assert!(stdout.contains("suma_basica"));
    assert!(stdout.contains("suma_negativa"));
    assert!(!stdout.contains("resta_basica"));
}

#[test]
fn test_filter_without_matches_returns_0_tests_but_exit_0() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "@test fn algo() { assert_eq(1, 1) }\n",
    );

    let (stdout, _stderr, code) =
        run_fitz(&["test", "--file", "t.fitz", "inexistente"], tmp.path());
    // 0 matches con un test descubierto = 0 failures, exit 0.
    assert_eq!(code, 0);
    assert!(stdout.contains("0 tests"));
    assert!(stdout.contains("1 filtered out"));
}

#[test]
fn test_async_fn_works() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "@test async fn pausa() {\n\
            let r = sleep(0).await\n\
            assert_eq(r, null)\n\
         }\n",
    );

    let (stdout, _stderr, code) = run_fitz(&["test", "--file", "t.fitz"], tmp.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("test pausa ... ok"));
}

#[test]
fn test_manifest_mode_descubre_tests_integration() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(
        root,
        "fitz.toml",
        "[package]\nname = \"libproj\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [lib]\nentry = \"src/lib.fitz\"\n",
    );
    write_file(
        root,
        "src/lib.fitz",
        "fn doble(n: Int) -> Int {\n    return n * 2\n}\n",
    );
    write_file(
        root,
        "tests/math.fitz",
        "from libproj import doble\n\n\
         @test fn doble_de_5_es_10() { assert_eq(doble(5), 10) }\n\
         @test fn doble_de_0_es_0() { assert_eq(doble(0), 0) }\n",
    );

    let (stdout, stderr, code) = run_fitz(&["test"], root);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout: {stdout}");
    assert!(stdout.contains("running 2 tests"));
    assert!(stdout.contains("tests/math.fitz::doble_de_5_es_10"));
    assert!(stdout.contains("tests/math.fitz::doble_de_0_es_0"));
}

#[test]
fn test_manifest_mode_lib_only_without_tests_loads_lib_inline() {
    // Proyecto solo-lib con `@test` inline, sin `tests/*.fitz`. El
    // runner debe cargar el lib directamente para descubrir esos tests.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(
        root,
        "fitz.toml",
        "[package]\nname = \"liblone\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [lib]\nentry = \"src/lib.fitz\"\n",
    );
    write_file(
        root,
        "src/lib.fitz",
        "fn doble(n: Int) -> Int {\n    return n * 2\n}\n\n\
         @test fn doble_de_21() { assert_eq(doble(21), 42) }\n",
    );

    let (stdout, stderr, code) = run_fitz(&["test"], root);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout: {stdout}");
    assert!(stdout.contains("test src/lib.fitz::doble_de_21 ... ok"));
    assert!(stdout.contains("1 passed"));
}

#[test]
fn test_manifest_no_duplica_tests_de_lib_importada_por_tests_integration() {
    // Regresión del bug fix: cuando `tests/*.fitz` importa el lib y el
    // lib tiene `@test` inline, esos tests se deben registrar UNA SOLA
    // VEZ (modo "tests integration": el runner solo carga `tests/*.fitz`
    // direct; el lib se carga via loader cache).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(
        root,
        "fitz.toml",
        "[package]\nname = \"dupproj\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [lib]\nentry = \"src/lib.fitz\"\n",
    );
    write_file(
        root,
        "src/lib.fitz",
        "fn doble(n: Int) -> Int { return n * 2 }\n\n\
         @test fn doble_inline() { assert_eq(doble(3), 6) }\n",
    );
    write_file(
        root,
        "tests/integration.fitz",
        "from dupproj import doble\n\n\
         @test fn doble_integracion() { assert_eq(doble(7), 14) }\n",
    );

    let (stdout, stderr, code) = run_fitz(&["test"], root);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout: {stdout}");
    // Solo 2 tests, no 3 (no duplicación):
    assert!(stdout.contains("running 2 tests"), "stdout: {stdout}");
    // El inline aparece con label del LIB (lib.fitz), no del importer:
    assert!(stdout.contains("lib.fitz::doble_inline"));
    assert!(stdout.contains("tests/integration.fitz::doble_integracion"));
}

#[test]
fn test_without_manifest_and_file_aborts_with_clear_message() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["test"], tmp.path());
    assert_eq!(code, 1);
    assert!(
        stderr.contains("not found") || stderr.contains("fitz.toml"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_assert_throws_passes_when_callback_throws() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "@test fn tira() { assert_throws(fn() => assert(false, \"intencional\")) }\n",
    );
    let (stdout, _stderr, code) = run_fitz(&["test", "--file", "t.fitz"], tmp.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("test tira ... ok"));
}

#[test]
fn test_file_with_type_errors_aborts_before_running() {
    // Strict checker: si el código no compila, no hay tests que correr.
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "@test fn t() { let x: Int = \"no es int\" }\n",
    );
    let (stdout, stderr, code) = run_fitz(&["test", "--file", "t.fitz"], tmp.path());
    assert_eq!(code, 1, "stdout: {stdout}, stderr: {stderr}");
    // Mensaje del checker llega via stderr (formato del runner).
    assert!(
        stderr.contains("error") || stderr.contains("tipo"),
        "stderr: {stderr}"
    );
}

// =================================================================
// Fase 9.z.5 — `fitz lint` (linter de patrones más allá de tipos)
// =================================================================

#[test]
fn lint_detecta_unused_variable_y_unused_import() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "import math\nlet x = 5\nprint(\"hola\")\n",
    );
    let (stdout, _stderr, code) = run_fitz(&["lint", "t.fitz"], tmp.path());
    assert_eq!(code, 0, "default lint exit 0 sobre warnings");
    assert!(stdout.contains("unused_import"));
    assert!(stdout.contains("unused_variable"));
    assert!(stdout.contains("`math`"));
    assert!(stdout.contains("`x`"));
}

#[test]
fn lint_deny_promueve_a_error_y_exit_1() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "t.fitz", "let x = 5\nprint(\"hola\")\n");
    let (stdout, _stderr, code) =
        run_fitz(&["lint", "t.fitz", "--deny", "unused_variable"], tmp.path());
    assert_eq!(code, 1);
    assert!(stdout.contains("error"));
    assert!(stdout.contains("unused_variable"));
    assert!(stdout.contains("1 denied"));
}

#[test]
fn lint_suppression_with_allow_silences() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "// @allow(unused_variable)\nlet x = 5\nprint(\"hola\")\n",
    );
    let (stdout, _stderr, code) = run_fitz(&["lint", "t.fitz"], tmp.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("sin findings"), "stdout: {stdout}");
}

#[test]
fn lint_nonexistent_file_returns_exit_1() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["lint", "no_existe.fitz"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("no se pudo"));
}

#[test]
fn lint_string_concat_detecta_literales() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "t.fitz", "let m = \"a\" + \"b\"\nprint(m)\n");
    let (stdout, _stderr, code) = run_fitz(&["lint", "t.fitz"], tmp.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("string_concat"));
}

#[test]
fn lint_clean_code_does_not_emit_findings() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "fn greet(name: Str) -> Str {\n    return \"Hola, {name}\"\n}\nprint(greet(\"Fitz\"))\n",
    );
    let (stdout, stderr, code) = run_fitz(&["lint", "t.fitz"], tmp.path());
    assert_eq!(code, 0, "stderr: {stderr}, stdout: {stdout}");
    assert!(stdout.contains("sin findings"));
}

#[test]
fn lint_useless_match_un_solo_arm_catchall() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "t.fitz",
        "let y = 5\nmatch y { _ => print(y) }\n",
    );
    let (stdout, _stderr, code) = run_fitz(&["lint", "t.fitz"], tmp.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("useless_match"));
}

// =================================================================
// Smoke del ejemplo del cap 16b: greeter (bin) importa greetings (lib)
// via [dependencies] path = "../greetings", fitz run produce el output
// esperado, fitz.lock se genera automático.
// =================================================================

#[test]
fn cap_16b_greeter_example_runs_and_generates_lockfile() {
    // Reproducimos `examples/guide/16b-pkg-manager/` en tempdir
    // para que el test no toque el repo. Lockfile generado vive en
    // el tempdir y se descarta al cerrar.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // --- greetings/ (lib) ---
    write_file(
        root,
        "greetings/fitz.toml",
        "[package]\nname = \"greetings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [lib]\nentry = \"src/lib.fitz\"\n",
    );
    write_file(
        root,
        "greetings/src/lib.fitz",
        "fn hola(nombre: Str) -> Str {\n    return \"Hola, {nombre}!\"\n}\n\n\
         fn formal(nombre: Str) -> Str {\n    return \"Buenas tardes, {nombre}.\"\n}\n",
    );

    // --- greeter/ (bin que importa greetings) ---
    write_file(
        root,
        "greeter/fitz.toml",
        "[package]\nname = \"greeter\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [bin]\nmain = \"src/main.fitz\"\n\n\
         [dependencies]\ngreetings = { path = \"../greetings\" }\n",
    );
    write_file(
        root,
        "greeter/src/main.fitz",
        "from greetings import hola, formal\n\n\
         print(hola(\"Fitz\"))\nprint(formal(\"Patagonia\"))\n",
    );

    // fitz run desde greeter/ — manifest mode + dep_registry resuelto
    let (stdout, stderr, code) = run_fitz(&["run"], &root.join("greeter"));
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("Hola, Fitz!"), "stdout: {stdout}");
    assert!(
        stdout.contains("Buenas tardes, Patagonia."),
        "stdout: {stdout}"
    );

    // El lockfile debe haberse generado en greeter/fitz.lock con la
    // dep `greetings` registrada.
    let lockfile = root.join("greeter").join("fitz.lock");
    assert!(lockfile.exists(), "fitz.lock was not generated");
    let lock_text = std::fs::read_to_string(&lockfile).expect("leer fitz.lock");
    assert!(
        lock_text.contains("name = \"greetings\""),
        "lock: {lock_text}"
    );
}

#[test]
fn cap_16b_fitz_build_compila_greeter_a_binario_nativo() {
    // Mismo setup que arriba pero compilamos a binario nativo y
    // verificamos que el ejecutable produce el output esperado.
    // Garantiza paridad bit-a-bit `fitz run` ↔ `fitz build` con
    // dep path involucrada.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    write_file(
        root,
        "greetings/fitz.toml",
        "[package]\nname = \"greetings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [lib]\nentry = \"src/lib.fitz\"\n",
    );
    write_file(
        root,
        "greetings/src/lib.fitz",
        "fn hola(nombre: Str) -> Str {\n    return \"Hola, {nombre}!\"\n}\n",
    );
    write_file(
        root,
        "greeter/fitz.toml",
        "[package]\nname = \"greeter\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [bin]\nmain = \"src/main.fitz\"\n\n\
         [dependencies]\ngreetings = { path = \"../greetings\" }\n",
    );
    write_file(
        root,
        "greeter/src/main.fitz",
        "from greetings import hola\n\nprint(hola(\"compilado\"))\n",
    );

    // Build (compila + emite a target/release/<pkg-name>{.exe}).
    let (stdout, stderr, code) = run_fitz(&["build"], &root.join("greeter"));
    assert_eq!(code, 0, "build stdout: {stdout}\nstderr: {stderr}");

    let bin_name = if cfg!(windows) {
        "greeter.exe"
    } else {
        "greeter"
    };
    let bin_path = root
        .join("greeter")
        .join("target")
        .join("release")
        .join(bin_name);
    assert!(
        bin_path.exists(),
        "binario no existe: {}",
        bin_path.display()
    );

    // Ejecutar y comparar output.
    let output = Command::new(&bin_path).output().expect("execute binary");
    assert!(output.status.success(), "exit: {:?}", output.status.code());
    let bin_stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        bin_stdout.contains("Hola, compilado!"),
        "stdout binario: {bin_stdout}"
    );
}

// ---------------------------------------------------------------------------
// pyi-stubs (v0.9.39) — `fitz py-stubs <archivo.pyi>`
// ---------------------------------------------------------------------------

#[test]
fn py_stubs_basic_class_emits_fitz_type() {
    let tmpdir = tempfile::tempdir().unwrap();
    let stub_path = tmpdir.path().join("user.pyi");
    std::fs::write(
        &stub_path,
        "class ApiUser:\n    id: int\n    name: str\n    active: bool\n",
    )
    .unwrap();

    let (stdout, _stderr, code) =
        run_fitz(&["py-stubs", stub_path.to_str().unwrap()], tmpdir.path());
    assert_eq!(code, 0, "exit: {}", code);
    assert!(stdout.contains("type ApiUser {"));
    assert!(stdout.contains("id: Int"));
    assert!(stdout.contains("name: Str"));
    assert!(stdout.contains("active: Bool"));
}

#[test]
fn py_stubs_tipos_compuestos_y_optional_se_mapean() {
    let tmpdir = tempfile::tempdir().unwrap();
    let stub_path = tmpdir.path().join("data.pyi");
    std::fs::write(
        &stub_path,
        "from typing import Optional\n\
         class ApiData:\n    \
             tags: list[str]\n    \
             counts: dict[str, int]\n    \
             nickname: Optional[str]\n    \
             flags: list[bool] | None\n",
    )
    .unwrap();

    let (stdout, _stderr, code) =
        run_fitz(&["py-stubs", stub_path.to_str().unwrap()], tmpdir.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("tags: List<Str>"));
    assert!(stdout.contains("counts: Map<Str, Int>"));
    assert!(stdout.contains("nickname: Str?"));
    assert!(stdout.contains("flags: List<Bool>?"));
}

#[test]
fn py_stubs_out_a_archivo() {
    let tmpdir = tempfile::tempdir().unwrap();
    let stub_path = tmpdir.path().join("models.pyi");
    let out_path = tmpdir.path().join("models.fitz");
    std::fs::write(&stub_path, "class Item:\n    sku: str\n    price: float\n").unwrap();

    let (_stdout, _stderr, code) = run_fitz(
        &[
            "py-stubs",
            stub_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
        ],
        tmpdir.path(),
    );
    assert_eq!(code, 0);
    let written = std::fs::read_to_string(&out_path).unwrap();
    assert!(written.contains("type Item {"));
    assert!(written.contains("sku: Str"));
    assert!(written.contains("price: Float"));
}

#[test]
fn py_stubs_nonexistent_file_is_error() {
    let tmpdir = tempfile::tempdir().unwrap();
    let bogus = tmpdir.path().join("nope.pyi");
    let (_stdout, stderr, code) = run_fitz(&["py-stubs", bogus.to_str().unwrap()], tmpdir.path());
    assert_ne!(code, 0);
    assert!(stderr.contains("py-stubs") || stderr.contains("no se pudo"));
}

#[test]
fn py_stubs_skip_fns_y_vars_solo_classes() {
    // Top-level def y var del stub se SKIPEAN (los conserva PyAny en
    // runtime). Solo las clases se materializan como `type` Fitz.
    let tmpdir = tempfile::tempdir().unwrap();
    let stub_path = tmpdir.path().join("mix.pyi");
    std::fs::write(
        &stub_path,
        "def helper(x: int) -> str: ...\n\
         VERSION: str\n\
         class Widget:\n    label: str\n",
    )
    .unwrap();

    let (stdout, _stderr, code) =
        run_fitz(&["py-stubs", stub_path.to_str().unwrap()], tmpdir.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("type Widget"));
    // helper/VERSION no se materializan (deuda menor documentada).
    assert!(!stdout.contains("fn helper"));
    assert!(!stdout.contains("VERSION"));
}

// =================================================================
// Fase 12.4 — `fitz docker init`
// =================================================================

/// Crea un mini-proyecto Fitz con `fitz.toml` + `src/main.fitz` que el
/// caller le pasa. Devuelve el path del directorio (raíz del proyecto).
fn make_docker_project(tmp: &Path, pkg_name: &str, main_fitz: &str) -> std::path::PathBuf {
    let project = tmp.join(pkg_name);
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("fitz.toml"),
        format!(
            "[package]\nname = \"{pkg}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n",
            pkg = pkg_name,
        ),
    )
    .unwrap();
    std::fs::write(project.join("src").join("main.fitz"), main_fitz).unwrap();
    project
}

#[test]
fn docker_init_pure_cli_writes_three_files_without_expose_or_db() {
    let tmp = tempfile::tempdir().unwrap();
    let project = make_docker_project(tmp.path(), "demo-cli", "print(\"hola\")\n");

    let (stdout, stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    let dockerignore = std::fs::read_to_string(project.join(".dockerignore")).unwrap();
    let compose = std::fs::read_to_string(project.join("docker-compose.yml")).unwrap();

    // Dockerfile: multi-stage, sin EXPOSE.
    assert!(dockerfile.contains("FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder"));
    assert!(dockerfile.contains("FROM gcr.io/distroless/cc-debian12"));
    assert!(
        dockerfile.contains("COPY --from=builder /app/target/release/demo-cli /usr/local/bin/app")
    );
    assert!(!dockerfile.contains("EXPOSE"));

    // .dockerignore: targets locales.
    assert!(dockerignore.contains("target/"));
    assert!(dockerignore.contains(".env"));

    // compose: solo `app` service, sin db, sin ports.
    assert!(compose.contains("  app:"));
    assert!(compose.contains("container_name: demo-cli"));
    assert!(!compose.contains("  db:"));
    assert!(!compose.contains("    ports:"));

    // stdout reporta CLI puro.
    assert!(stdout.contains("CLI program (no @server)"));
    assert!(stdout.contains("wrote: Dockerfile"));
    assert!(stdout.contains("wrote: .dockerignore"));
    assert!(stdout.contains("wrote: docker-compose.yml"));
}

#[test]
fn docker_init_http_with_server_emits_expose_and_ports() {
    let tmp = tempfile::tempdir().unwrap();
    let main_fitz = "@get(\"/\")\nfn root() => \"ok\"\n\n@server(8080)\nfn main() => 0\n";
    let project = make_docker_project(tmp.path(), "myhttp", main_fitz);

    let (stdout, stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    let compose = std::fs::read_to_string(project.join("docker-compose.yml")).unwrap();

    assert!(dockerfile.contains("EXPOSE 8080"));
    assert!(compose.contains("    ports:\n      - \"8080:8080\""));
    assert!(stdout.contains("@server(port = 8080)"));
}

#[test]
fn docker_init_with_db_emits_postgres_and_database_url() {
    let tmp = tempfile::tempdir().unwrap();
    let main_fitz = "\
@get(\"/users\")
async fn list_users() -> List<Str> {
    let conn = db.connect(\"postgres://x\")
    []
}

@server(3000)
fn main() => 0
";
    let project = make_docker_project(tmp.path(), "api-db", main_fitz);

    let (stdout, stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let compose = std::fs::read_to_string(project.join("docker-compose.yml")).unwrap();
    assert!(compose.contains("  db:"));
    assert!(compose.contains("image: postgres:16-alpine"));
    assert!(compose.contains("DATABASE_URL:"));
    assert!(compose.contains("depends_on:"));
    assert!(compose.contains("\nvolumes:\n  pgdata:"));
    assert!(stdout.contains("DB usage"));
}

#[test]
fn docker_init_skips_existing_files_and_suggests_force() {
    let tmp = tempfile::tempdir().unwrap();
    let project = make_docker_project(tmp.path(), "demo-skip", "print(\"hola\")\n");
    std::fs::write(project.join("Dockerfile"), "viejo").unwrap();

    let (stdout, stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    assert_eq!(dockerfile, "viejo", "skip preserved the old file");

    assert!(stdout.contains("skipped"));
    assert!(stdout.contains("--force"));
}

#[test]
fn docker_init_force_overwrites_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let project = make_docker_project(tmp.path(), "demo-force", "print(\"hola\")\n");
    std::fs::write(project.join("Dockerfile"), "viejo").unwrap();

    let (_stdout, stderr, code) = run_fitz(&["docker", "init", "--force"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    assert!(dockerfile.contains("FROM ghcr.io/thegreekman76/fitz"));
}

#[test]
fn docker_init_without_manifest_aborts_with_clear_message() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["docker", "init"], tmp.path());
    assert_ne!(code, 0);
    // Mensaje del helper resolve_entry: cita `fitz.toml` o el comando
    // para crear uno.
    assert!(
        stderr.contains("fitz.toml") || stderr.contains("fitz new"),
        "stderr inesperado: {stderr}",
    );
}

// ---- 12.4.b — smart detection rica + `fitz docker build` ----

#[test]
fn docker_init_uses_python_reporta_runtime_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let main_fitz = "from python import math\n\nprint(\"hola\")\n";
    let project = make_docker_project(tmp.path(), "py-cli", main_fitz);

    let (stdout, stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    assert!(
        dockerfile.contains("FROM python:3.12-slim-bookworm"),
        "Dockerfile expected with runtime python:3.12-slim-bookworm: {dockerfile}",
    );
    assert!(!dockerfile.contains("FROM gcr.io/distroless/cc-debian12"));
    assert!(stdout.contains("Python interop"));
    assert!(stdout.contains("python:3.12-slim-bookworm"));
}

#[test]
fn docker_init_uses_cron_emits_restart_unless_stopped() {
    let tmp = tempfile::tempdir().unwrap();
    let main_fitz = "@cron(\"0 * * * *\")\nfn limpiar() => 0\n";
    let project = make_docker_project(tmp.path(), "scheduler", main_fitz);

    let (stdout, stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let compose = std::fs::read_to_string(project.join("docker-compose.yml")).unwrap();
    assert!(compose.contains("    restart: unless-stopped"));
    assert!(stdout.contains("@cron"));
}

#[test]
fn docker_init_python_and_server_emits_http_healthcheck_in_compose() {
    let tmp = tempfile::tempdir().unwrap();
    let main_fitz = "\
from python import os

@get(\"/\")
fn root() => \"ok\"

@server(3000)
fn main() => 0
";
    let project = make_docker_project(tmp.path(), "py-api", main_fitz);

    let (_stdout, stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");

    let compose = std::fs::read_to_string(project.join("docker-compose.yml")).unwrap();
    assert!(compose.contains("    healthcheck:"));
    assert!(compose.contains("wget"));
    assert!(compose.contains("http://localhost:3000/healthz"));
}

#[test]
fn docker_init_distroless_and_server_does_not_emit_healthcheck_but_clear_comment() {
    let tmp = tempfile::tempdir().unwrap();
    let main_fitz = "@get(\"/\")\nfn root() => \"ok\"\n\n@server(3000)\nfn main() => 0\n";
    let project = make_docker_project(tmp.path(), "api", main_fitz);

    let (_stdout, _stderr, code) = run_fitz(&["docker", "init"], &project);
    assert_eq!(code, 0);

    let compose = std::fs::read_to_string(project.join("docker-compose.yml")).unwrap();
    // El comentario explicando por qué no emitimos healthcheck con
    // distroless está presente.
    assert!(compose.contains("HTTP healthcheck NOT emitted"));
    assert!(compose.contains("/healthz"));
}

#[test]
fn docker_build_without_dockerfile_aborts_with_suggestion() {
    let tmp = tempfile::tempdir().unwrap();
    let project = make_docker_project(tmp.path(), "no-dockerfile", "print(\"hola\")\n");
    // NO corremos `docker init` primero — no debería haber Dockerfile.

    let (_stdout, stderr, code) = run_fitz(&["docker", "build"], &project);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("Dockerfile") && stderr.contains("fitz docker init"),
        "stderr inesperado: {stderr}",
    );
}

#[test]
fn docker_build_without_manifest_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["docker", "build"], tmp.path());
    assert_ne!(code, 0);
    assert!(
        stderr.contains("fitz.toml") || stderr.contains("fitz new"),
        "stderr inesperado: {stderr}",
    );
}

// =================================================================
// Fase 12.6 — `fitz deploy <docker|compose>`
// =================================================================

#[test]
fn deploy_docker_without_dockerfile_aborts_with_suggestion() {
    let tmp = tempfile::tempdir().unwrap();
    let project = make_docker_project(tmp.path(), "no-dockerfile", "print(\"hola\")\n");

    let (_stdout, stderr, code) = run_fitz(&["deploy", "docker"], &project);
    assert_ne!(code, 0);
    // El error puede ser MissingDockerfile (si docker está instalado y
    // pasamos el pre-flight check) o DockerNotInstalled (sin docker en
    // PATH). Cualquiera de los dos es correcto.
    assert!(
        stderr.contains("Dockerfile") || stderr.contains("docker"),
        "stderr inesperado: {stderr}",
    );
}

#[test]
fn deploy_compose_without_compose_file_aborts_with_suggestion() {
    let tmp = tempfile::tempdir().unwrap();
    let project = make_docker_project(tmp.path(), "no-compose", "print(\"hola\")\n");

    let (_stdout, stderr, code) = run_fitz(&["deploy", "compose"], &project);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("docker-compose.yml")
            || stderr.contains("docker")
            || stderr.contains("compose"),
        "stderr inesperado: {stderr}",
    );
}

#[test]
fn deploy_docker_without_manifest_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["deploy", "docker"], tmp.path());
    assert_ne!(code, 0);
    assert!(
        stderr.contains("fitz.toml") || stderr.contains("fitz new"),
        "stderr inesperado: {stderr}",
    );
}

#[test]
fn deploy_compose_without_manifest_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["deploy", "compose"], tmp.path());
    assert_ne!(code, 0);
    assert!(
        stderr.contains("fitz.toml") || stderr.contains("fitz new"),
        "stderr inesperado: {stderr}",
    );
}

#[test]
fn deploy_help_lista_docker_y_compose() {
    let tmp = tempfile::tempdir().unwrap();
    let (stdout, _stderr, code) = run_fitz(&["deploy", "--help"], tmp.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("docker"));
    assert!(stdout.contains("compose"));
}

// -----------------------------------------------------------------
// Phase 4 Y-B, Session 1.c — `fitz new --template <name>`
// -----------------------------------------------------------------

/// Corre `fitz` seteando además los env vars indicados. Necesario para
/// los tests del template CLI, que apuntan las plantillas a repos git
/// locales via `FITZ_TEMPLATE_<NAME>_URL/SUBPATH/REF` sin tocar red.
fn run_fitz_with_env(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> (String, String, i32) {
    let mut cmd = Command::new(fitz_bin());
    cmd.args(args).current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("invocar fitz");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

/// Setup helper: crea un template repo mínimo en `<tmp>/tpl-src/` con
/// una carpeta `templates/basic/` que contiene `fitz.toml` +
/// `src/main.fitz` + `README.md`, usando `{{name}}` como placeholder.
/// Lo convierte en git repo con tag `main` y devuelve la ruta absoluta
/// del repo (para armar la URL `file:///`).
fn setup_liveviews_template_repo(tmp_root: &Path) -> std::path::PathBuf {
    let repo = tmp_root.join("tpl-src");
    let basic = repo.join("templates").join("basic");
    let basic_src = basic.join("src");
    std::fs::create_dir_all(&basic_src).unwrap();

    std::fs::write(
        basic.join("fitz.toml"),
        "[package]\nname = \"{{name}}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [bin]\nmain = \"src/main.fitz\"\n",
    )
    .unwrap();
    std::fs::write(
        basic_src.join("main.fitz"),
        "print(\"Hola desde {{name}}\")\n",
    )
    .unwrap();
    std::fs::write(
        basic.join("README.md"),
        "# {{name}}\n\nProject scaffolded from the `liveviews` template.\n",
    )
    .unwrap();
    // A binary-ish file to make sure the copy tolerates non-UTF-8
    // content untouched.
    std::fs::write(basic.join("blob.bin"), [0xFFu8, 0xFE, 0x00, 0x01, 0x02]).unwrap();

    // Convertimos en repo git con tag `main` (reusando el patrón de
    // `init_git_repo_with_tag` pero con el tag `main` que coincide con
    // el ref por default del template `liveviews`).
    let _ = init_git_repo_with_tag(&repo, "main");
    repo
}

#[test]
fn new_with_template_liveviews_scaffolds_project() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_liveviews_template_repo(tmp.path());

    // `file:///...` que git entiende igual en Linux/Mac/Windows.
    let repo_str = repo.to_string_lossy().replace('\\', "/");
    let git_url = format!("file:///{}", repo_str.trim_start_matches('/'));

    // Directorio de trabajo aislado para que el proyecto scaffoldeado
    // caiga adentro del tempdir.
    let workdir = tmp.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();

    let (stdout, stderr, code) = run_fitz_with_env(
        &["new", "my-app", "--template", "liveviews", "--no-git"],
        &workdir,
        &[
            ("FITZ_TEMPLATE_LIVEVIEWS_URL", git_url.as_str()),
            ("FITZ_TEMPLATE_LIVEVIEWS_SUBPATH", "templates/basic"),
            ("FITZ_TEMPLATE_LIVEVIEWS_REF", "main"),
        ],
    );
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");

    let project = workdir.join("my-app");
    assert!(project.is_dir(), "project directory was not created");
    assert!(project.join("fitz.toml").is_file(), "fitz.toml missing");
    assert!(
        project.join("src").join("main.fitz").is_file(),
        "src/main.fitz missing",
    );
    assert!(project.join("README.md").is_file(), "README.md missing");

    // `{{name}}` fue substituido en cada archivo de texto.
    let toml = std::fs::read_to_string(project.join("fitz.toml")).unwrap();
    assert!(
        toml.contains("name = \"my-app\""),
        "placeholder not substituted in fitz.toml: {toml}",
    );
    assert!(
        !toml.contains("{{name}}"),
        "leftover placeholder in fitz.toml"
    );

    let main = std::fs::read_to_string(project.join("src").join("main.fitz")).unwrap();
    // Normalizamos CRLF: en Windows `git clone` puede convertir LF a
    // CRLF (autocrlf); otros SOs mantienen LF. Nos importa el
    // contenido, no el line ending exacto.
    assert_eq!(main.replace("\r\n", "\n"), "print(\"Hola desde my-app\")\n",);

    let readme = std::fs::read_to_string(project.join("README.md")).unwrap();
    assert!(readme.contains("# my-app"), "README head: {readme}");

    // El archivo binario se copió byte-a-byte sin sustituir.
    let blob = std::fs::read(project.join("blob.bin")).unwrap();
    assert_eq!(blob, vec![0xFFu8, 0xFE, 0x00, 0x01, 0x02]);

    // `.git/` del template repo NO se propaga al proyecto scaffoldeado.
    assert!(
        !project.join(".git").exists(),
        "template's .git/ should not be copied into the new project",
    );
}

#[test]
fn new_with_unknown_template_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(
        &["new", "my-app", "--template", "does-not-exist", "--no-git"],
        tmp.path(),
    );
    assert_ne!(code, 0, "expected non-zero exit for unknown template");
    assert!(
        stderr.contains("unknown template") || stderr.contains("does-not-exist"),
        "stderr should mention the unknown template name: {stderr}",
    );
    // El folder my-app no debería haber quedado creado.
    assert!(
        !tmp.path().join("my-app").exists(),
        "aborted scaffolding should not leave the target directory behind",
    );
}

#[test]
fn new_template_and_http_are_mutually_exclusive() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(
        &[
            "new",
            "my-app",
            "--template",
            "liveviews",
            "--http",
            "--no-git",
        ],
        tmp.path(),
    );
    assert_ne!(code, 0, "expected clap to reject --template + --http");
    // clap suele mencionar el flag conflictivo por nombre.
    assert!(
        stderr.contains("--http") || stderr.contains("--template") || stderr.contains("conflict"),
        "stderr: {stderr}",
    );
}

// ---- Phase 11.5.b — multi-bin `[[bin]]` + --bin / --target flags ----

/// Helper: scaffolds a multi-bin project with a native server bin
/// and a wasm-client web bin.
fn scaffold_multi_bin_project(tmp: &Path, pkg: &str) {
    let src = tmp.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.join("fitz.toml"),
        format!(
            "[package]\nname = \"{pkg}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
             [[bin]]\nname = \"server\"\nmain = \"src/main.fitz\"\n\n\
             [[bin]]\nname = \"web\"\nmain = \"src/app.fitz\"\ntarget = \"wasm-client\"\nmount = \"#app\"\n"
        ),
    )
    .unwrap();
    std::fs::write(src.join("main.fitz"), "print(\"server bin\")\n").unwrap();
    std::fs::write(src.join("app.fitz"), "print(\"web bin\")\n").unwrap();
}

#[test]
fn phase_11_5_b_build_multi_bin_without_flag_errors_listing_bins() {
    let tmp = tempfile::tempdir().unwrap();
    scaffold_multi_bin_project(tmp.path(), "multi-app");
    let (_stdout, stderr, code) = run_fitz(&["build"], tmp.path());
    assert_ne!(code, 0, "expected build to reject ambiguous multi-bin");
    // The error must list ALL bin names so the user knows what to pick.
    assert!(stderr.contains("server"), "stderr: {stderr}");
    assert!(stderr.contains("web"), "stderr: {stderr}");
    assert!(
        stderr.contains("--bin"),
        "stderr should mention --bin: {stderr}"
    );
}

#[test]
fn phase_11_5_b_build_with_bin_missing_name_errors_listing_available() {
    let tmp = tempfile::tempdir().unwrap();
    scaffold_multi_bin_project(tmp.path(), "multi-app");
    let (_stdout, stderr, code) = run_fitz(&["build", "--bin", "does-not-exist"], tmp.path());
    assert_ne!(code, 0, "expected build to reject unknown bin");
    assert!(
        stderr.contains("does-not-exist"),
        "stderr should quote the requested name: {stderr}"
    );
    assert!(stderr.contains("server"), "stderr: {stderr}");
    assert!(stderr.contains("web"), "stderr: {stderr}");
}

#[test]
fn phase_11_5_c_build_with_bin_wasm_client_and_fitz_main_rejects_citing_11_5_d() {
    // The multi-bin fixture uses `main = "src/app.fitz"` (a
    // classic `.fitz`) with `target = "wasm-client"`. Phase 11.5.c
    // only wires the single-component `.fitzv` emit; composing
    // classic `.fitz` as WASM roots is 11.5.d work. The CLI
    // aborts before touching `wasm-pack` with a targeted
    // 11.5.d pointer.
    let tmp = tempfile::tempdir().unwrap();
    scaffold_multi_bin_project(tmp.path(), "multi-app");
    let (_stdout, stderr, code) = run_fitz(&["build", "--bin", "web"], tmp.path());
    assert_ne!(code, 0, "expected build to reject .fitz + wasm-client");
    assert!(stderr.contains(".fitzv"), "stderr: {stderr}");
    assert!(
        stderr.contains("11.5.d"),
        "stderr should cite 11.5.d: {stderr}"
    );
}

#[test]
fn phase_11_5_c_build_wasm_client_scaffolds_before_invoking_wasm_pack() {
    // A wasm-client bin pointing at a valid `.fitzv` should
    // successfully write the wasm-crate scaffold under
    // `target/wasm-build/<bin>/` BEFORE invoking `wasm-pack`.
    // We assert the scaffold artefacts exist regardless of whether
    // `wasm-pack` succeeds (the CLI may exit non-zero on missing
    // toolchain — that's a separate concern covered by another test).
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.path().join("fitz.toml"),
        "[package]\nname = \"webapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [[bin]]\nname = \"web\"\nmain = \"src/App.fitzv\"\ntarget = \"wasm-client\"\nmount = \"#app\"\n",
    )
    .unwrap();
    std::fs::write(
        src.join("App.fitzv"),
        "component App {\n  state { n: Int = 0 }\n  event tap() { n = n + 1 }\n  <template><button @click=\"tap\">{n}</button></template>\n}\n",
    )
    .unwrap();

    // Exit code is best-effort — we only care that the scaffold got
    // written. `wasm-pack` may not be on the runner.
    let _ = run_fitz(&["build", "--bin", "web"], tmp.path());

    let scaffold_dir = tmp.path().join("target").join("wasm-build").join("web");
    assert!(
        scaffold_dir.is_dir(),
        "scaffold dir was not created: {}",
        scaffold_dir.display()
    );
    let cargo_toml = scaffold_dir.join("Cargo.toml");
    assert!(
        cargo_toml.is_file(),
        "Cargo.toml missing at {}",
        cargo_toml.display()
    );
    let cargo_txt = std::fs::read_to_string(&cargo_toml).unwrap();
    assert!(
        cargo_txt.contains(r#"name = "web""#),
        "Cargo.toml must have package.name = \"web\": {cargo_txt}"
    );
    assert!(
        cargo_txt.contains("[package.metadata.wasm-pack.profile.release]"),
        "Cargo.toml must have the wasm-opt metadata knob: {cargo_txt}"
    );
    let lib_rs = scaffold_dir.join("src").join("lib.rs");
    assert!(
        lib_rs.is_file(),
        "src/lib.rs missing at {}",
        lib_rs.display()
    );
    let lib_txt = std::fs::read_to_string(&lib_rs).unwrap();
    assert!(
        lib_txt.contains("#[wasm_bindgen(start)]"),
        "src/lib.rs must have the composed entry point"
    );
    assert!(
        lib_txt.contains("let root = App::new();"),
        "src/lib.rs must mount the first-declared component `App`"
    );
    assert!(
        lib_txt.contains("root.mount(\"#app\")"),
        "src/lib.rs must interpolate the `#app` mount selector"
    );
    assert!(
        lib_txt.contains("Source: src/App.fitzv"),
        "src/lib.rs header must cite the bin's `main`"
    );
}

#[test]
fn phase_11_5_c_build_wasm_client_with_empty_fitzv_errors_before_wasm_pack() {
    // A `.fitzv` file with zero components declared errors at
    // `compose_lib_rs` — the composer needs a root component to
    // mount. The message must explain the requirement.
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.path().join("fitz.toml"),
        "[package]\nname = \"empty\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [[bin]]\nname = \"web\"\nmain = \"src/Empty.fitzv\"\ntarget = \"wasm-client\"\nmount = \"#app\"\n",
    )
    .unwrap();
    std::fs::write(src.join("Empty.fitzv"), "// no components\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["build", "--bin", "web"], tmp.path());
    assert_ne!(code, 0);
    assert!(
        stderr.contains("no `component X { ... }` block"),
        "stderr should explain the empty file: {stderr}"
    );
}

#[test]
fn phase_11_5_b_build_target_override_ssr_rejects_citing_11_6() {
    let tmp = tempfile::tempdir().unwrap();
    // Single-bin project. `--target ssr` overrides the default native.
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.path().join("fitz.toml"),
        "[package]\nname = \"srv\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n",
    )
    .unwrap();
    std::fs::write(src.join("main.fitz"), "print(\"x\")\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["build", "--target", "ssr"], tmp.path());
    assert_ne!(code, 0);
    assert!(stderr.contains("ssr"), "stderr: {stderr}");
    assert!(stderr.contains("11.6"), "stderr should cite 11.6: {stderr}");
}

#[test]
fn phase_11_5_b_build_unknown_target_value_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.path().join("fitz.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n",
    )
    .unwrap();
    std::fs::write(src.join("main.fitz"), "print(\"x\")\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["build", "--target", "bogus-target"], tmp.path());
    assert_ne!(code, 0);
    assert!(stderr.contains("bogus-target"), "stderr: {stderr}");
    assert!(
        stderr.contains("native") && stderr.contains("wasm-client") && stderr.contains("ssr"),
        "stderr should list accepted targets: {stderr}"
    );
}

#[test]
fn phase_11_5_b_legacy_bin_singular_still_works_no_flag_needed() {
    // Regression: pre-11.5.b manifests (with `[bin]` singular)
    // must keep building without --bin.
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.path().join("fitz.toml"),
        "[package]\nname = \"legacy\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n",
    )
    .unwrap();
    std::fs::write(src.join("main.fitz"), "print(\"ok\")\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["check"], tmp.path());
    assert_eq!(code, 0, "check must accept legacy [bin]: {stderr}");
}

#[test]
fn phase_11_5_b_manifest_fitzv_native_rejected_at_parse() {
    // Cross-field validation: `.fitzv` entry with target = native
    // errors at Manifest::parse — surfaces via any subcommand that
    // reads the manifest.
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.path().join("fitz.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[[bin]]\nname = \"web\"\nmain = \"src/counter.fitzv\"\n",
    )
    .unwrap();
    // Body doesn't matter — parse aborts on the manifest itself.
    std::fs::write(src.join("counter.fitzv"), "component X {}\n").unwrap();
    let (_stdout, stderr, code) = run_fitz(&["check"], tmp.path());
    assert_ne!(code, 0);
    assert!(stderr.contains(".fitzv"), "stderr: {stderr}");
    assert!(stderr.contains("wasm-client"), "stderr: {stderr}");
}

// ---------------------------------------------------------------------------
// Phase 11.6.d — Module loader integration for `.fitzv`
// ---------------------------------------------------------------------------
//
// The classic module loader (used by `fitz run` / `fitz build` /
// `fitz check`) transparently routes a `.fitzv` sibling through
// the view pipeline (parse → expand → check → emit_module_ssr)
// when it is resolved as an import target. The tests below
// exercise the shape from the CLI:
//
// 1. `main.fitz` imports `Card` from `Card.fitzv`. `.fitzv`
//    routing is transparent — the classic loader never sees the
//    view AST, but rather the emitted classic Fitz source.
// 2. When both `Card.fitz` and `Card.fitzv` exist, `.fitz` wins
//    (backward-compat with pre-11.6.d resolution).
// 3. A broken `.fitzv` file surfaces its view-pipeline error
//    (with the file path in the message) rather than a
//    generic "module not found".
//
// Since the emitted classic Fitz depends on
// `from fitz_liveviews import Html, html`, and the test tempdir
// does not have `fitz-liveviews` on the dep registry, these
// tests use `fitz check` and expect the check to progress FAR
// enough to reach the "fitz_liveviews module not found" error.
// That's the exact signal the classic loader received the
// transformed source (as opposed to falling back to
// `Card not found`, which would prove routing was broken).

#[test]
fn phase_11_6_d_import_of_fitzv_sibling_is_transformed_through_view_pipeline() {
    // 2-file project: main.fitz + Card.fitzv sibling.
    // Expectation: `fitz run` resolves `from Card import Card`
    // to `Card.fitzv`, runs the view pipeline, and hands the
    // emitted source to the classic pipeline. The classic
    // pipeline then tries to resolve `fitz_liveviews` (from
    // the emitted `from fitz_liveviews import Html, html`) and
    // fails — but the FAILURE cites `fitz_liveviews`, proving
    // the `.fitzv` transformation happened. Uses `fitz run`
    // (not `fitz check`) because `run` actually loads the
    // module via the evaluator loader, which is the code path
    // updated by 11.6.d.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "main.fitz", "from Card import Card\nprint(\"hi\")\n");
    write_file(
        root,
        "Card.fitzv",
        "component Card {\n  state { title: Str = \"hello\" }\n  <template><span>{title}</span></template>\n}\n",
    );

    let (_stdout, stderr, code) = run_fitz(&["run", "main.fitz"], root);
    assert_ne!(code, 0, "run must fail: stderr = {stderr}");
    assert!(
        stderr.contains("fitz_liveviews"),
        "loader must have transformed Card.fitzv and hit the missing \
         `fitz_liveviews` dep; stderr:\n{stderr}"
    );
    // Negative assertion: the resolver should NOT have failed on
    // finding `Card` — that would prove `.fitzv` fallback did not
    // fire.
    assert!(
        !stderr.contains("module `Card` not found") && !stderr.contains("`Card` no encontrado"),
        "the `.fitzv` extension fallback did not fire; stderr:\n{stderr}"
    );
}

#[test]
fn phase_11_6_d_import_prefers_dot_fitz_over_dot_fitzv_when_both_exist() {
    // Backward-compat: if a sibling has BOTH `Card.fitz` and
    // `Card.fitzv`, the classic `.fitz` wins. The user
    // migrating from classic to view can drop the `.fitzv`
    // sibling and keep the classic file authoritative until
    // the migration completes.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "main.fitz", "from Card import Card\nprint(Card)\n");
    // The classic Card.fitz declares `Card` as a plain type.
    write_file(
        root,
        "Card.fitz",
        "type Card {\n  title: Str = \"classic\"\n}\n",
    );
    // The view Card.fitzv would emit `@live_component`, but
    // must NOT be picked when the classic file exists.
    write_file(
        root,
        "Card.fitzv",
        "component Card {\n  state { title: Str = \"view\" }\n  <template><span>{title}</span></template>\n}\n",
    );

    // `fitz run` succeeds — the classic Card.fitz is a plain
    // type, the print emits the type Display, no `fitz_liveviews`
    // dep needed.
    let (stdout, stderr, code) = run_fitz(&["run", "main.fitz"], root);
    assert_eq!(code, 0, "should run cleanly: stderr = {stderr}");
    assert!(
        stdout.contains("Card") || stdout.contains("classic"),
        "output should reflect the classic Card definition: stdout = {stdout}"
    );
    // And the `fitz_liveviews` dep must NOT appear anywhere in
    // the output — a sure sign the `.fitzv` was ignored.
    assert!(
        !stderr.contains("fitz_liveviews"),
        "the `.fitzv` shadowed unexpectedly: stderr = {stderr}"
    );
}

#[test]
fn phase_11_6_d_broken_fitzv_import_surfaces_view_pipeline_error_with_path() {
    // A malformed `.fitzv` sibling must not surface as a
    // generic "module not found" — the loader routed the
    // file through the view pipeline, and the error cites
    // both the file path and a view-stage name.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(
        root,
        "main.fitz",
        "from Broken import Something\nprint(\"hi\")\n",
    );
    write_file(
        root,
        "Broken.fitzv",
        "component Broken { <template>", // opening never closes
    );

    // `fitz run` fails first at the module loader (evaluator)
    // when it tries to load Broken.fitzv. The pre-scan paths
    // have silent fallback, so `fitz check` may not surface
    // it — the runtime loader does.
    let (_stdout, stderr, code) = run_fitz(&["run", "main.fitz"], root);
    assert_ne!(code, 0, "run must fail: stderr = {stderr}");
    assert!(
        stderr.contains("Broken.fitzv"),
        "error must cite the offending .fitzv path: stderr = {stderr}"
    );
    assert!(
        stderr.contains("view parse error")
            || stderr.contains("view expand error")
            || stderr.contains("view check errors")
            || stderr.contains("view emit_ssr error"),
        "error must name at least one view stage: stderr = {stderr}"
    );
}

// ===================================================================
// Phase 11.6.e continuation (§9.bb, 2026-07-16) — cross-module
// `@live_component` auto-inject.
//
// A `.fitzv` sibling declaring `@live_component` + `@render_for` +
// `@on` is imported by main.fitz WITHOUT the manual
// `flv_register(...)` boot call. The compiler's cross-module
// pre-scan populates `TypeEnv.imported_live_components` and
// `inject_live_component_registrations` synthesises the call so
// the framework can dispatch — same UX as v0.20.1's local implicit
// register, extended across module boundaries.
//
// Uses a stub `fitz_liveviews.fitz` sibling so `fitz run` can
// satisfy the classic loader's `from fitz_liveviews import
// Html, html, flv_register` without requiring the real library
// (which lives in a separate git dep in the sibling repo).
// ===================================================================

/// Stub content for a `fitz_liveviews.fitz` sibling. Provides just
/// enough surface for the E2E tests below to boot: `Html` type,
/// `html(s) -> Html` factory, and `flv_register(...)` no-op. The
/// counter's emitted classic Fitz source uses `Html`/`html`; the
/// auto-injector emits `flv_register(...)` calls.
const FITZ_LIVEVIEWS_STUB_9BB: &str = "type Html { raw: Str = \"\" }\n\
fn html(s: Str) -> Html => Html { raw: s }\n\
fn flv_register(name: Str, initial_state: Any, render_fn: Any, events: Map<Str, Any>) -> Null => null\n";

/// Canonical Counter.fitzv used by the tests below. Emits (after
/// the view pipeline transform) a `@live_component("Counter")` type,
/// a `@render_for("Counter") fn Counter_render`, and one
/// `@on("Counter", "increment") fn Counter_increment`.
const COUNTER_FITZV_9BB: &str = "component Counter {\n  \
    state { count: Int = 0 }\n  \
    event increment() { count = count + 1 }\n  \
    <template><span>{count}</span></template>\n\
}\n";

#[test]
fn phase_11_6_e_bb_cross_module_live_component_auto_injects_flv_register() {
    // Canonical case: main.fitz imports Counter (state type) plus
    // its render + event fn names from a `.fitzv` sibling but
    // does NOT write a manual `flv_register(...)` call. The
    // cross-module pre-scan extracts the component metadata and
    // the injector synthesises the call. `fitz run` completes
    // successfully — proving the auto-inject path is wired end-
    // to-end for imported `.fitzv` modules.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "fitz_liveviews.fitz", FITZ_LIVEVIEWS_STUB_9BB);
    write_file(root, "Counter.fitzv", COUNTER_FITZV_9BB);
    write_file(
        root,
        "main.fitz",
        "from fitz_liveviews import Html, html, flv_register\n\
         from Counter import Counter, Counter_render, Counter_increment\n\
         print(\"boot OK\")\n",
    );
    let (stdout, stderr, code) = run_fitz(&["run", "main.fitz"], root);
    assert_eq!(
        code, 0,
        "cross-module auto-inject must succeed: stderr = {stderr}"
    );
    assert!(
        stdout.contains("boot OK"),
        "main.fitz must reach the print stmt (auto-inject appended after it, no side effect from the stub): stdout = {stdout}"
    );
}

#[test]
fn phase_11_6_e_bb_cross_module_missing_imports_errors_with_hint() {
    // When main.fitz forgets a name required by the imported
    // component (here `Counter_render` and `Counter_increment`),
    // the injector must error citing every missing name plus an
    // actionable `from <module> import ...` hint. Complements the
    // happy-path test above.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "fitz_liveviews.fitz", FITZ_LIVEVIEWS_STUB_9BB);
    write_file(root, "Counter.fitzv", COUNTER_FITZV_9BB);
    write_file(
        root,
        "main.fitz",
        "from fitz_liveviews import Html, html, flv_register\n\
         from Counter import Counter\n\
         print(\"boot\")\n",
    );
    let (_stdout, stderr, code) = run_fitz(&["run", "main.fitz"], root);
    assert_ne!(code, 0, "must fail with missing-imports error");
    assert!(
        stderr.contains("Counter_render") && stderr.contains("Counter_increment"),
        "error must list every missing name: stderr = {stderr}"
    );
    assert!(
        stderr.contains("Add `from Counter import"),
        "error must include the actionable fix: stderr = {stderr}"
    );
}
