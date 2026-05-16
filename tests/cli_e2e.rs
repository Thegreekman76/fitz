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

// ---- Fase 9.y.2 — `fitz run`/`build`/`check` integrados con manifest ----

/// Helper: crea un proyecto con `fitz new <name> --no-git` adentro de
/// `tmp` y devuelve el path del proyecto.
fn create_project(tmp_root: &Path, name: &str) -> std::path::PathBuf {
    let (_stdout, stderr, code) =
        run_fitz(&["new", name, "--no-git"], tmp_root);
    assert_eq!(code, 0, "fitz new falló: {stderr}");
    tmp_root.join(name)
}

#[test]
fn run_sin_args_dentro_de_proyecto_ejecuta_bin_main() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "run-test");
    let (stdout, stderr, code) = run_fitz(&["run"], &project);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("Hola desde run-test"),
        "stdout inesperado: {stdout}"
    );
}

#[test]
fn check_sin_args_dentro_de_proyecto_chequea_bin_main() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "check-test");
    let (stdout, _stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("sin errores de tipo"),
        "stdout inesperado: {stdout}"
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
    assert!(stdout.contains("Hola desde walk-test"), "stdout: {stdout}");
}

#[test]
fn run_con_archivo_explicito_ignora_manifest_y_corre_en_single_file_mode() {
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
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("single-file mode OK"),
        "stdout: {stdout}"
    );
}

#[test]
fn run_sin_manifest_y_sin_archivo_aborta_con_mensaje_claro() {
    let tmp = tempfile::tempdir().unwrap();
    // No creamos proyecto: el tempdir está vacío.
    let (_stdout, stderr, code) = run_fitz(&["run"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("fitz.toml"), "stderr: {stderr}");
    assert!(stderr.contains("fitz new"), "stderr: {stderr}");
}

#[test]
fn check_sin_manifest_y_sin_archivo_aborta() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_fitz(&["check"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("fitz.toml"), "stderr: {stderr}");
}

#[test]
fn manifest_sin_seccion_bin_aborta_con_mensaje_claro() {
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
fn manifest_corrupto_aborta_con_mensaje_claro() {
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
        "// lib mínima para tests\nfn helper(x: Int) -> Int => x + 1\n",
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
fn check_con_path_dep_emite_lockfile() {
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
fn check_es_idempotente_sin_re_escribir_lockfile() {
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
        "segunda corrida no debía notificar update: {stdout2}"
    );
}

#[test]
fn lockfile_se_regenera_cuando_dep_cambia_version() {
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
    assert!(lock_text.contains("version = \"0.5.1\""), "lock: {lock_text}");
}

#[test]
fn proyecto_sin_deps_no_emite_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "no-deps");
    let (_stdout, _stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 0);
    assert!(
        !project.join("fitz.lock").exists(),
        "no debió crearse fitz.lock para proyecto sin deps"
    );
}

#[test]
fn version_corta_aborta_citando_9y5() {
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
fn git_dep_sin_tag_ni_rev_aborta_pidiendo_uno() {
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
    assert!(stderr.contains("tag") && stderr.contains("rev"), "stderr: {stderr}");
    assert!(stderr.contains("reproducibilidad"), "stderr: {stderr}");
}

#[test]
fn path_dep_inexistente_aborta() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "missing-app");
    add_path_dep(&project, "missing-app", "ghost", "../no-existe");
    let (_stdout, stderr, code) = run_fitz(&["check"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("no existe"), "stderr: {stderr}");
}

#[test]
fn path_dep_sin_lib_aborta_con_sugerencia() {
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
fn run_resuelve_from_dep_import_via_dep_registry() {
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
fn run_dep_no_referenciada_no_falla_si_no_se_importa() {
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
    assert!(app_dir.join("fitz.lock").is_file(), "lockfile debió emitirse");
}

#[test]
fn run_local_fitz_no_es_shadoweado_por_archivo_inexistente() {
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
fn run_dep_shadowea_archivo_local_con_mismo_nombre() {
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
            "git {} falló:\n{}",
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
fn run_fitz_with_cache(
    args: &[&str],
    cwd: &Path,
    cache_dir: &Path,
) -> (String, String, i32) {
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
fn git_dep_clona_al_cache_y_emite_lockfile_con_commit() {
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
    assert_eq!(entries.len(), 1, "se esperaba un dir clonado, hay: {entries:?}");

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
fn git_dep_reusa_cache_sin_re_clonar() {
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
        "el marker se borró — el cache fue re-clonado en lugar de reusado"
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "no me toques",
        "el marker fue sobrescrito — re-clone destructivo"
    );
}

#[test]
fn git_dep_lockfile_idempotente_si_commit_no_cambia() {
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
        "lockfile no debió re-escribirse: {stdout2}"
    );
}

#[test]
fn git_dep_tag_inexistente_aborta_con_mensaje_de_git() {
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
fn build_resuelve_from_dep_import_via_dep_registry() {
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
    assert!(bin_path.is_file(), "binario no existe en {}", bin_path.display());

    let output = Command::new(&bin_path).output().expect("ejecutar binario");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("d=42"), "stdout del binario: {stdout}");
}

// ---- Fase 9.y.4 — `fitz add` / `fitz remove` / `fitz update` ----

#[test]
fn add_path_dep_modifica_manifest_y_emite_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = tmp.path().join("utils-lib");
    let _ = run_fitz(&["new", "utils-lib", "--no-git"], tmp.path());
    convert_to_lib(&lib_dir, "utils-lib", "0.1.0");

    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");

    let (stdout, stderr, code) = run_fitz(
        &["add", "utils-lib", "--path", "../utils-lib"],
        &app_dir,
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("agregado"), "stdout: {stdout}");

    let manifest = std::fs::read_to_string(app_dir.join("fitz.toml")).unwrap();
    assert!(
        manifest.contains("utils-lib = { path = \"../utils-lib\" }"),
        "manifest:\n{manifest}"
    );
    let lockfile = std::fs::read_to_string(app_dir.join("fitz.lock")).unwrap();
    assert!(lockfile.contains("name = \"utils-lib\""), "lockfile:\n{lockfile}");
}

#[test]
fn add_sin_flags_aborta_pidiendo_path_o_git() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "myapp");
    let (_stdout, stderr, code) = run_fitz(&["add", "foo"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("--path") && stderr.contains("--git"), "stderr: {stderr}");
    assert!(stderr.contains("9.y.5"), "stderr debería mencionar el registry: {stderr}");
}

#[test]
fn add_git_sin_tag_ni_rev_aborta_pidiendo_uno() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "myapp");
    let (_stdout, stderr, code) =
        run_fitz(&["add", "foo", "--git", "https://x.com/r"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("tag") && stderr.contains("rev"), "stderr: {stderr}");
    assert!(stderr.contains("reproducibilidad"), "stderr: {stderr}");
}

#[test]
fn add_path_y_git_juntos_aborta_clap_conflict() {
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
fn add_fuera_de_proyecto_aborta_con_mensaje_claro() {
    let tmp = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) =
        run_fitz(&["add", "foo", "--path", "../x"], tmp.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("fitz.toml"), "stderr: {stderr}");
    assert!(stderr.contains("fitz new"), "stderr: {stderr}");
}

#[test]
fn add_sobreescribe_dep_existente() {
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
        "el path viejo no debió persistir:\n{manifest}"
    );
}

#[test]
fn remove_dep_existente_quita_y_actualiza_lockfile() {
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
        "fitz.lock debió borrarse al quedar sin deps"
    );
}

#[test]
fn remove_dep_inexistente_aborta() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_project(tmp.path(), "myapp");
    let (_stdout, stderr, code) = run_fitz(&["remove", "no-existe"], &project);
    assert_eq!(code, 1);
    assert!(stderr.contains("no estaba"), "stderr: {stderr}");
}

#[test]
fn update_sin_git_deps_reporta_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let _ = run_fitz(&["new", "u", "--no-git"], tmp.path());
    convert_to_lib(&tmp.path().join("u"), "u", "0.1.0");
    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    let _ = run_fitz(&["add", "u", "--path", "../u"], &app_dir);

    let (stdout, _, code) = run_fitz(&["update"], &app_dir);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("no había git deps"),
        "stdout: {stdout}"
    );
}

#[test]
fn update_invalida_cache_de_git_dep_y_re_clona() {
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
    assert!(stdout.contains("cache invalidado"), "stdout: {stdout}");

    // El cache fue re-clonado: el marker NO debería existir más.
    let cache_entries_after: Vec<_> = std::fs::read_dir(&git_cache).unwrap().collect();
    assert_eq!(cache_entries_after.len(), 1, "debió quedar exactamente un clone");
    let new_clone = cache_entries_after[0].as_ref().unwrap().path();
    assert!(
        !new_clone.join("FITZ_TEST_MARKER").exists(),
        "el marker debió desaparecer tras re-clone"
    );
}

#[test]
fn update_dep_inexistente_aborta() {
    let tmp = tempfile::tempdir().unwrap();
    let _ = run_fitz(&["new", "u", "--no-git"], tmp.path());
    convert_to_lib(&tmp.path().join("u"), "u", "0.1.0");
    let _ = run_fitz(&["new", "myapp", "--no-git"], tmp.path());
    let app_dir = tmp.path().join("myapp");
    let _ = run_fitz(&["add", "u", "--path", "../u"], &app_dir);

    let (_, stderr, code) = run_fitz(&["update", "no-existe"], &app_dir);
    assert_eq!(code, 1);
    assert!(stderr.contains("no está"), "stderr: {stderr}");
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
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let after = std::fs::read_to_string(&file).unwrap();
    assert!(
        after.contains("fn double(n: Int) -> Int {\n    return n * 2\n}"),
        "fn no quedó multi-línea con indent:\n{after}"
    );
    assert!(after.ends_with('\n'), "debería terminar con newline");
}

#[test]
fn fmt_check_idempotente_devuelve_0() {
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
fn fmt_check_no_canonico_devuelve_1_sin_modificar() {
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
fn fmt_emite_warning_loud_en_modo_write() {
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
    assert!(stderr.contains("⚠"), "stderr no incluye warning: {stderr}");
    assert!(stderr.contains("9.z.1.b"), "stderr no menciona la deuda: {stderr}");
    assert!(
        stderr.contains("comentarios") || stderr.contains("blank"),
        "stderr no explica qué se pierde: {stderr}"
    );
}

#[test]
fn fmt_check_no_emite_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("ok.fitz");
    std::fs::write(&file, "let x = 1\n").unwrap();

    let output = Command::new(fitz_bin())
        .args(["fmt", "--check"])
        .arg(&file)
        .output()
        .expect("fitz fmt --check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("⚠"),
        "--check no debe emitir warning (read-only): {stderr}"
    );
}

#[test]
fn fmt_archivo_con_error_de_sintaxis_aborta_sin_escribir() {
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
fn fmt_sin_args_dentro_de_proyecto_descubre_archivos_de_src() {
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
        "src/main.fitz no se reformateó:\n{after}"
    );
}

/// Build sin args en manifest mode: produce el binario en
/// `<manifest_dir>/target/release/<pkg-name>(.exe)` con el nombre del
/// paquete (NO el stem del fuente). Test pesado (~3s) porque invoca
/// rustc real adentro de cargo build.
#[test]
fn build_sin_args_emite_a_target_release_con_pkg_name() {
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
        "binario no existe en {}",
        bin_path.display()
    );

    // Ejecutar el binario y verificar el output.
    let output = Command::new(&bin_path).output().expect("ejecutar binario");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Hola desde build-target-test"),
        "stdout del binario: {stdout}"
    );
}
