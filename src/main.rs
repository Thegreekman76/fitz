// main.rs — Entry point del compilador/intérprete de Fitz.
//
// Los módulos viven en `src/lib.rs` desde Fase 9.x.1.b (refactor
// lib + bin para que `fitz-lsp` pueda reusarlos sin compilación
// duplicada). Acá solo importamos lo que el CLI consume.

use fitz::{codegen, evaluator, fmt, http, lexer, lockfile, manifest, openapi, parser, types};

// Sub-comando `fitz py-types` (Fase 8.5) — solo con la feature `python`.
#[cfg(feature = "python")]
use fitz::py_types;

use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

/// Fitz — El lenguaje de programación nacido en la Patagonia 🏔️
#[derive(Parser)]
#[command(name = "fitz")]
#[command(version = "0.1.0")]
#[command(about = "El lenguaje de programación Fitz")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ejecutar un archivo .fitz (o el `[bin].main` del `fitz.toml` si
    /// no se pasa archivo — Fase 9.y.2).
    Run {
        /// Archivo a ejecutar. Si se omite, busca `fitz.toml` en el
        /// directorio actual o ancestros (Cargo-style) y ejecuta su
        /// `[bin].main`.
        file: Option<PathBuf>,
        /// Saltar el chequeo estático de tipos. Sin esta flag los
        /// errores del checker abortan la ejecución (modo strict).
        #[arg(long)]
        no_typecheck: bool,
    },
    /// Compilar a binario (Fase 5b). Sin archivo, lee el manifest
    /// (Fase 9.y.2) y emite el binario a `<manifest>/target/release/`
    /// con el nombre del paquete.
    Build {
        /// Archivo a compilar. Si se omite, busca `fitz.toml` y
        /// compila su `[bin].main` con output en
        /// `<manifest_dir>/target/release/<pkg-name>`.
        file: Option<PathBuf>,
    },
    /// Verificar tipos y sintaxis. Sin archivo, lee el manifest
    /// (Fase 9.y.2) y chequea el `[bin].main`.
    Check {
        /// Archivo a verificar. Si se omite, busca `fitz.toml` y
        /// chequea su `[bin].main`.
        file: Option<PathBuf>,
    },
    /// Emite el schema OpenAPI 3.1 del programa a stdout
    Openapi {
        /// Archivo a inspeccionar
        file: PathBuf,
    },
    /// Fase 8.5 — Genera `type` Fitz a partir de modelos SQLAlchemy
    /// definidos en un archivo Python. La introspección usa duck
    /// typing sobre `__table__.columns`: cualquier clase con ese
    /// shape se traduce (compatible con SQLAlchemy real y mocks
    /// equivalentes). Requiere binario `fitz` compilado con
    /// `--features python`.
    PyTypes {
        /// Archivo Python con modelos a introspeccionar
        source: PathBuf,
        /// Archivo destino. Si se omite, escribe a stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Fase 9.y.1 — Crea un proyecto Fitz nuevo en una carpeta.
    ///
    /// Genera `<name>/fitz.toml`, `<name>/src/main.fitz`, `<name>/.gitignore`,
    /// y (a menos que se pase `--no-git`) corre `git init`. El nombre
    /// debe matchear `^[a-z][a-z0-9_-]{0,63}$`.
    New {
        /// Nombre del proyecto (también nombre de la carpeta a crear).
        name: String,
        /// Template HTTP en vez de CLI hello world.
        #[arg(long)]
        http: bool,
        /// No correr `git init` en la carpeta creada.
        #[arg(long)]
        no_git: bool,
    },
    /// Fase 9.y.1 — Inicializa un proyecto Fitz en el directorio actual.
    ///
    /// Genera `./fitz.toml`, `./src/main.fitz`, `./.gitignore`, y (a
    /// menos que se pase `--no-git`) corre `git init`. El nombre del
    /// paquete se deriva del nombre del directorio actual, o del flag
    /// `--name` si se provee. Falla si ya existe un `fitz.toml`.
    Init {
        /// Sobrescribe el nombre del paquete (default: nombre del
        /// directorio actual). Debe matchear `^[a-z][a-z0-9_-]{0,63}$`.
        #[arg(long)]
        name: Option<String>,
        /// Template HTTP en vez de CLI hello world.
        #[arg(long)]
        http: bool,
        /// No correr `git init` en el directorio.
        #[arg(long)]
        no_git: bool,
    },
    /// Fase 9.y.4 — Agrega una dep al `fitz.toml` del proyecto actual
    /// y sincroniza el `fitz.lock`. Requiere `--path` o `--git`
    /// (versiones sueltas registry-style llegan en 9.y.5).
    ///
    /// Si la dep ya existía con el mismo nombre, se sobreescribe
    /// (cargo-style). Si la resolución posterior falla (path
    /// inexistente, git clone fallido, etc.), el manifest persiste
    /// igual — usá `fitz remove <name>` para revertir.
    Add {
        /// Nombre de la dep tal como aparecerá en `[dependencies]`.
        name: String,
        /// Path dep relativo al manifest del proyecto.
        #[arg(long, conflicts_with = "git")]
        path: Option<String>,
        /// URL del repo git. Requiere también `--tag` o `--rev`.
        #[arg(long, conflicts_with = "path")]
        git: Option<String>,
        /// Tag a checkout-ear (mutuamente exclusivo con `--rev`).
        #[arg(long, conflicts_with = "rev", requires = "git")]
        tag: Option<String>,
        /// Commit SHA a checkout-ear (mutuamente exclusivo con `--tag`).
        #[arg(long, conflicts_with = "tag", requires = "git")]
        rev: Option<String>,
    },
    /// Fase 9.y.4 — Quita una dep del `fitz.toml` del proyecto actual
    /// y sincroniza el `fitz.lock`. Si la dep no existía, error claro.
    Remove {
        /// Nombre de la dep a quitar (tal como aparece en
        /// `[dependencies]`).
        name: String,
    },
    /// Fase 9.y.4 — Re-resuelve las deps del proyecto actual. Para
    /// git deps, invalida el cache local y re-clona (útil cuando el
    /// tag upstream se movió o cuando querés un fetch fresh). Para
    /// path deps es no-op (siempre fresh). Sin args, actualiza todas.
    Update {
        /// Nombre de la dep específica a actualizar. Sin este flag,
        /// actualiza todas las deps del manifest.
        name: Option<String>,
    },
    /// Fase 9.z.1.a — Formatea código Fitz a su estilo canónico
    /// (cero config). 4 espacios indent, comillas dobles, trailing
    /// comma solo multi-línea. Sin argumentos, formatea todos los
    /// `.fitz` del proyecto actual (vía manifest). Con archivos
    /// explícitos, formatea solo esos.
    ///
    /// ⚠ ALPHA (9.z.1.a): el modo write borra comentarios y blank
    /// lines del usuario. Comment preservation llega en 9.z.1.b.
    /// El modo `--check` es safe (read-only) y no estropea nada.
    Fmt {
        /// Archivos `.fitz` a formatear. Si se omiten, formatea todo
        /// el proyecto (requiere `fitz.toml`).
        files: Vec<PathBuf>,
        /// Modo CI: no escribe, exit 1 si hay diffs. Read-only, sin
        /// pérdida de comments/blanks.
        #[arg(long)]
        check: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file, no_typecheck } => {
            let resolved = resolve_entry(file);
            sync_lockfile_if_needed(&resolved);
            let dep_registry = dep_registry_from(&resolved);
            run_file(&resolved.entry, no_typecheck, dep_registry);
        }
        Commands::Build { file } => {
            let resolved = resolve_entry(file);
            sync_lockfile_if_needed(&resolved);
            // En manifest mode, output a `<manifest_dir>/target/release/
            // <pkg-name>(.exe)` (Cargo-style). En single-file mode, el
            // copy adyacente al fuente se decide adentro de build_file.
            let override_dest = resolved.manifest_ctx.as_ref().map(|ctx| {
                let filename = if cfg!(windows) {
                    format!("{}.exe", ctx.manifest.package.name)
                } else {
                    ctx.manifest.package.name.clone()
                };
                ctx.manifest_dir
                    .join("target")
                    .join("release")
                    .join(filename)
            });
            let dep_registry = dep_registry_from(&resolved);
            build_file(&resolved.entry, override_dest.as_deref(), dep_registry);
        }
        Commands::Check { file } => {
            let resolved = resolve_entry(file);
            sync_lockfile_if_needed(&resolved);
            // `fitz check` no usa el loader (el checker no recursea en
            // módulos importados — los nombres del importer se tipan
            // como Any/nominal placeholder y la validación real ocurre
            // en `fitz run`/`build`). Por eso `check_file` no recibe el
            // dep_registry. Si en el futuro el checker quiere consumir
            // tipos del módulo importado, agregar acá el wiring.
            check_file(&resolved.entry);
        }
        Commands::Openapi { file } => {
            openapi_file(&file);
        }
        Commands::PyTypes { source, out } => {
            py_types_file(&source, out.as_deref());
        }
        Commands::New { name, http, no_git } => {
            new_project(&name, http, no_git);
        }
        Commands::Init { name, http, no_git } => {
            init_project(name.as_deref(), http, no_git);
        }
        Commands::Add { name, path, git, tag, rev } => {
            add_dep_cmd(&name, path.as_deref(), git.as_deref(), tag.as_deref(), rev.as_deref());
        }
        Commands::Remove { name } => {
            remove_dep_cmd(&name);
        }
        Commands::Update { name } => {
            update_deps_cmd(name.as_deref());
        }
        Commands::Fmt { files, check } => {
            fmt_cmd(files, check);
        }
    }
}

// ---- Fase 9.y.2 — resolución de entry point (single-file vs manifest) ----

/// Contexto del manifest cargado durante `resolve_entry`. Cuando está
/// presente, el caller sabe que el run/build/check arrancó desde un
/// proyecto Fitz (no en modo single-file).
///
/// Por ahora lo consumen: `build_file` para decidir el destino del
/// binario; `sync_lockfile_if_needed` para emitir el `fitz.lock`; y
/// el dispatch (Run/Build) para construir el `dep_registry` que
/// recibe el evaluator y el codegen (9.y.3.b).
struct ManifestCtx {
    manifest: manifest::Manifest,
    manifest_dir: PathBuf,
    /// Deps resueltas (path deps resueltas a `lib_entry` absoluto).
    /// Fase 9.y.3.a: poblado en `resolve_entry`, consumido por
    /// `sync_lockfile_if_needed`. Fase 9.y.3.b: también usado para
    /// armar el `dep_registry` que pasa al evaluator / codegen.
    resolved_deps: Vec<manifest::ResolvedDep>,
}

/// Resultado de resolver el entry point del comando. `entry` apunta al
/// `.fitz` a procesar; `manifest_ctx` está presente cuando se llegó
/// vía `fitz.toml` (manifest mode).
struct ResolvedEntry {
    entry: PathBuf,
    manifest_ctx: Option<ManifestCtx>,
}

/// Resuelve el entry point del subcomando:
///
/// - Si `file_opt.is_some()`, modo **single-file** (compatibilidad
///   pre-9.y.2): devuelve el path tal cual, sin manifest ctx.
/// - Si `file_opt.is_none()`, modo **manifest**: busca `fitz.toml`
///   subiendo desde el cwd (Cargo-style), lo parsea, y devuelve
///   `<manifest_dir>/[bin].main` como entry. Sale del proceso con
///   mensaje claro si:
///   - no hay `fitz.toml` arriba del cwd (sugiere `fitz new` o pasar
///     archivo explícito);
///   - el manifest no parsea;
///   - el manifest no tiene sección `[bin]` (el MVP de 9.y exige uno;
///     multi-bin queda 9.y.8+).
fn resolve_entry(file_opt: Option<PathBuf>) -> ResolvedEntry {
    if let Some(entry) = file_opt {
        return ResolvedEntry {
            entry,
            manifest_ctx: None,
        };
    }

    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer el directorio actual: {e}");
        std::process::exit(1);
    });

    let manifest_path = match manifest::find_manifest(&cwd) {
        Some(p) => p,
        None => {
            eprintln!(
                "✗ no se encontró `{}` en `{}` ni en directorios padre.\n   \
                 Pasá un archivo explícito (`fitz <cmd> archivo.fitz`) o creá un \
                 proyecto con `fitz new <nombre>` / `fitz init`.",
                manifest::MANIFEST_FILE,
                cwd.display()
            );
            std::process::exit(1);
        }
    };

    let manifest_text = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!(
            "✗ no se pudo leer `{}`: {e}",
            manifest_path.display()
        );
        std::process::exit(1);
    });

    let manifest = manifest::Manifest::parse(&manifest_text).unwrap_or_else(|e| {
        eprintln!("✗ `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    });

    let manifest_dir = manifest_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.clone());

    let bin = match &manifest.bin {
        Some(b) => b,
        None => {
            eprintln!(
                "✗ `{}` no tiene sección `[bin]` con un `main`. El MVP del package \
                 manager (Fase 9.y) requiere uno. Agregá:\n\n[bin]\nmain = \"src/main.fitz\"\n",
                manifest_path.display()
            );
            std::process::exit(1);
        }
    };

    let entry = manifest_dir.join(&bin.main);

    // Fase 9.y.3.a — resolver deps eager (fail-fast con mensaje del
    // resolver). Si hay errores, abortamos antes de tocar el lockfile
    // o invocar al evaluator/codegen.
    let resolved_deps = manifest::resolve_dependencies(&manifest, &manifest_dir)
        .unwrap_or_else(|e| {
            eprintln!("✗ no se pudieron resolver las dependencias: {e}");
            std::process::exit(1);
        });

    ResolvedEntry {
        entry,
        manifest_ctx: Some(ManifestCtx {
            manifest,
            manifest_dir,
            resolved_deps,
        }),
    }
}

/// Fase 9.y.3.b — construye el `DepRegistry` (map `dep-name →
/// lib_entry-absoluto`) consumido por `eval_with_base_and_deps_sync`
/// (`fitz run`) y `codegen::generate_project` (`fitz build`).
///
/// Devuelve registry vacío en single-file mode (sin manifest) o cuando
/// el manifest no tiene `[dependencies]`. El loader trata empty igual
/// que pre-9.y.3.b: solo path-relativo, sin shortcuts.
fn dep_registry_from(resolved: &ResolvedEntry) -> manifest::DepRegistry {
    match &resolved.manifest_ctx {
        Some(ctx) => manifest::build_dep_registry(&ctx.resolved_deps),
        None => manifest::DepRegistry::new(),
    }
}

/// Fase 9.y.3.a — sincroniza el `fitz.lock` con las deps del manifest.
/// No-op en modo single-file y cuando el manifest no tiene deps.
///
/// Para 9.y.3.a las deps son solo path deps (resolución determinística
/// trivial). El lockfile se regenera siempre; `write_lockfile_if_changed`
/// hace short-circuit byte-a-byte cuando el contenido coincide para
/// no spamear mtime y diff vacío.
fn sync_lockfile_if_needed(resolved: &ResolvedEntry) {
    let ctx = match &resolved.manifest_ctx {
        Some(c) => c,
        None => return,
    };
    if ctx.resolved_deps.is_empty() {
        return;
    }

    let lock = lockfile::Lockfile::from_resolved(&ctx.resolved_deps);
    let path = lockfile::lockfile_path(&ctx.manifest_dir);
    match lockfile::write_lockfile_if_changed(&path, &lock) {
        Ok(true) => {
            // Solo notificamos cuando escribimos algo nuevo. La
            // regeneración silenciosa es el caso 90% y no merece spam.
            println!("✓ actualizado {}", path.display());
        }
        Ok(false) => {} // sin cambios
        Err(e) => {
            eprintln!("✗ no se pudo escribir `{}`: {e}", path.display());
            std::process::exit(1);
        }
    }
}

/// `fitz py-types <archivo.py> [--out <archivo.fitz>]` — Fase 8.5.
/// Importa el archivo Python via PyO3, introspecciona las clases con
/// `__table__.columns` (compatible con SQLAlchemy real y mocks), y
/// genera `type` Fitz correspondientes. Escribe a stdout o al
/// `--out` indicado.
///
/// Sin feature `python`, emite error claro citando el flag de build.
#[cfg(feature = "python")]
fn py_types_file(source: &std::path::Path, out: Option<&std::path::Path>) {
    let output = match py_types::generate_from_file(source) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ py-types: {}", e);
            std::process::exit(1);
        }
    };
    match out {
        Some(path) => match fs::write(path, &output) {
            Ok(_) => println!("✓ types Fitz emitidos a {}", path.display()),
            Err(e) => {
                eprintln!("✗ py-types: no se pudo escribir `{}`: {}", path.display(), e);
                std::process::exit(1);
            }
        },
        None => print!("{}", output),
    }
}

#[cfg(not(feature = "python"))]
fn py_types_file(_source: &std::path::Path, _out: Option<&std::path::Path>) {
    eprintln!(
        "✗ `fitz py-types` requiere recompilar `fitz` con interop Python habilitada. \
         Este binario se compiló sin la feature `python`. \
         Recompilá con `cargo install --features python` (o \
         `cargo build --features python`)."
    );
    std::process::exit(1);
}

/// `fitz openapi <archivo>` — Fase 7.1. Lex + parse + check + eval
/// con un `HttpRegistry` activo para que los decoradores HTTP registren
/// sus rutas; después escupe el schema OpenAPI 3.1 a stdout
/// (pretty-printed).
///
/// No levanta el server: el registry se popula durante `eval` (los
/// decoradores HTTP son side-effects del top-level) y el schema se
/// puede derivar de ahí + el AST.
///
/// Útil para CI, generar SDKs con openapi-generator, snapshot testing
/// del contrato.
fn openapi_file(path: &PathBuf) {
    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error leyendo {}: {}", path.display(), e);
        std::process::exit(1);
    });

    let tokens = match lexer::tokenize(&source) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Checker estricto: no tiene sentido emitir un schema de un programa
    // con errores de tipo (el handler quizá ni siquiera tipa). Mismo
    // criterio que `fitz build`.
    let (_env, _types, _defs, type_errors) = types::check_program(&program);
    if !type_errors.is_empty() {
        eprintln!(
            "✗ {} — {} error(es) de tipo:",
            path.display(),
            type_errors.len()
        );
        for e in &type_errors {
            eprintln!("  {}", e);
        }
        std::process::exit(1);
    }

    let base_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let (eval_result, registry) = http::with_active_registry(|| {
        evaluator::eval_with_base_sync(program.clone(), base_dir)
    });
    if let Err(e) = eval_result {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    let routes = openapi::routes_from_registry(&registry);
    // Q.2: `@server(api_version=...)` override.
    let api_version = registry
        .server_config
        .as_ref()
        .and_then(|c| c.api_version.clone());
    let schema = openapi::generate_openapi_with_version(
        &routes,
        &program,
        api_version.as_deref(),
    );
    match serde_json::to_string_pretty(&schema) {
        Ok(s) => println!("{}", s),
        Err(e) => {
            eprintln!("Error serializando schema: {}", e);
            std::process::exit(1);
        }
    }
}

/// `fitz check <archivo>` — corre lexer + parser + checker estático
/// y reporta errores. Exit code 0 si está limpio, 1 si hay errores
/// de cualquier tipo.
fn check_file(path: &PathBuf) {
    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error leyendo {}: {}", path.display(), e);
        std::process::exit(1);
    });
    let tokens = match lexer::tokenize(&source) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let (_env, _types, _defs, errors) = types::check_program(&program);
    if errors.is_empty() {
        println!("✓ {} — sin errores de tipo", path.display());
    } else {
        eprintln!(
            "✗ {} — {} error(es) de tipo:",
            path.display(),
            errors.len()
        );
        for e in &errors {
            eprintln!("  {}", e);
        }
        std::process::exit(1);
    }
}

/// `fitz build <archivo>` — Fase 5b. Compila el .fitz a binario nativo.
/// Flujo: lex → parse → checker (strict) → codegen a Cargo project →
/// `cargo build --release` → copia el binario.
///
/// Destino del binario:
/// - **Single-file mode** (`override_dest = None`): adyacente al `.fitz`
///   con el stem original (`hello.fitz` → `hello.exe`). Comportamiento
///   pre-9.y.2.
/// - **Manifest mode** (`override_dest = Some(p)`): el caller provee la
///   ruta destino completa (típicamente `<manifest_dir>/target/release/
///   <pkg-name>(.exe)`). Llega desde el dispatch en `main()` cuando el
///   user corre `fitz build` sin args y hay un `fitz.toml`.
///
/// Desde 5b.5 generamos un Cargo project en lugar de invocar rustc
/// directamente. Razones: (a) los imports cross-archivo necesitan
/// múltiples `.rs` con `mod`, lo que se hace nativo con cargo; (b) cuando
/// llegue 5b.6 con HTTP, sumamos `axum`/`tokio`/`serde_json` al
/// `Cargo.toml` generado sin reescribir pipeline; (c) cargo cachea
/// incremental, lo que abarata segunda compilación. Trade-off: la
/// primera compilación cuesta ~1-2s más que `rustc` directo.
fn build_file(
    path: &PathBuf,
    override_dest: Option<&std::path::Path>,
    dep_registry: manifest::DepRegistry,
) {
    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error leyendo {}: {}", path.display(), e);
        std::process::exit(1);
    });

    let tokens = match lexer::tokenize(&source) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Checker en modo strict — no hay `--no-typecheck` en build.
    let (env, _types, _defs, type_errors) = types::check_program(&program);
    if !type_errors.is_empty() {
        eprintln!(
            "✗ {} — {} error(es) de tipo:",
            path.display(),
            type_errors.len()
        );
        for e in &type_errors {
            eprintln!("  {}", e);
        }
        eprintln!("   Usá `fitz check` para revisar antes de buildear.");
        std::process::exit(1);
    }

    // Codegen a Cargo project.
    let project = match codegen::generate_project(path, &program, &env, dep_registry) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ codegen: {}", e);
            eprintln!("   (Fase 5b soporta un subset progresivo; los mensajes citan el sub-paso correspondiente.)");
            std::process::exit(1);
        }
    };

    // Layout del Cargo project: target/fitz-build/<stem>/{Cargo.toml, src/...}.
    let build_dir = PathBuf::from("target")
        .join("fitz-build")
        .join(&project.bin_name);
    let src_dir = build_dir.join("src");
    if let Err(e) = fs::create_dir_all(&src_dir) {
        eprintln!("Error creando {}: {}", src_dir.display(), e);
        std::process::exit(1);
    }

    // Escribir Cargo.toml.
    let cargo_toml_path = build_dir.join("Cargo.toml");
    if let Err(e) = fs::write(&cargo_toml_path, &project.cargo_toml) {
        eprintln!("Error escribiendo {}: {}", cargo_toml_path.display(), e);
        std::process::exit(1);
    }

    // Escribir src/main.rs.
    let main_rs_path = src_dir.join("main.rs");
    if let Err(e) = fs::write(&main_rs_path, &project.main_rs) {
        eprintln!("Error escribiendo {}: {}", main_rs_path.display(), e);
        std::process::exit(1);
    }

    // Escribir cada mod file (5b.5+).
    for mod_file in &project.mod_files {
        let dest = src_dir.join(&mod_file.rel_path);
        if let Some(parent) = dest.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("Error creando {}: {}", parent.display(), e);
                std::process::exit(1);
            }
        }
        if let Err(e) = fs::write(&dest, &mod_file.content) {
            eprintln!("Error escribiendo {}: {}", dest.display(), e);
            std::process::exit(1);
        }
    }

    // Invocar cargo build --release. Trabajamos contra el manifiesto
    // del project generado; el target dir se hereda (cargo decide).
    let output = std::process::Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&cargo_toml_path)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error invocando cargo: {}", e);
            eprintln!("   ¿Tenés cargo en el PATH? (`rustup` lo provee.)");
            std::process::exit(1);
        }
    };

    if !output.status.success() {
        eprintln!("✗ cargo build falló al compilar el código generado:");
        eprintln!("   (revisá {} para ver qué se intentó compilar.)", src_dir.display());
        eprintln!("--- stderr de cargo ---");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }

    // Binario en target/release/<bin_name>; copiar adyacente al .fitz
    // con el `output_basename` (= stem original del .fitz, sin
    // sanitizar). Si el usuario buildea `02-hola.fitz`, el archivo
    // final es `02-hola.exe` aunque el crate dentro de Cargo se llame
    // `fitz_02-hola`.
    let release_bin_filename = if cfg!(windows) {
        format!("{}.exe", project.bin_name)
    } else {
        project.bin_name.clone()
    };
    let output_filename = if cfg!(windows) {
        format!("{}.exe", project.output_basename)
    } else {
        project.output_basename.clone()
    };
    let release_bin_path = build_dir
        .join("target")
        .join("release")
        .join(&release_bin_filename);

    // Destino: override del manifest (9.y.2) o adyacente al fuente.
    let bin_out = match override_dest {
        Some(p) => p.to_path_buf(),
        None => path
            .parent()
            .map(|p| p.join(&output_filename))
            .unwrap_or_else(|| PathBuf::from(&output_filename)),
    };

    // Crear el directorio destino si hace falta (manifest mode: el
    // primer build de un proyecto recién creado no tiene target/release/
    // todavía).
    if let Some(parent) = bin_out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("Error creando {}: {}", parent.display(), e);
                std::process::exit(1);
            }
        }
    }

    if let Err(e) = fs::copy(&release_bin_path, &bin_out) {
        eprintln!(
            "Error copiando {} a {}: {}",
            release_bin_path.display(),
            bin_out.display(),
            e
        );
        std::process::exit(1);
    }

    println!("✓ binario: {}", bin_out.display());
}

fn run_file(path: &PathBuf, no_typecheck: bool, dep_registry: manifest::DepRegistry) {
    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error leyendo {}: {}", path.display(), e);
        std::process::exit(1);
    });

    // Fase 2.1: lexer
    let tokens = match lexer::tokenize(&source) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Fase 2.3: parser
    let program = match parser::parse(tokens) {
        Ok(program) => program,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Fase 5.4: checker estático en modo strict por default. Los
    // errores de tipo abortan la ejecución antes de pasar al
    // evaluator. La flag `--no-typecheck` cambia el comportamiento
    // a warning (los reporta pero sigue ejecutando), pensada para
    // legacy code o para diagnosticar bugs del checker.
    let (_type_env, _types, _defs, type_errors) = types::check_program(&program);
    if !type_errors.is_empty() {
        if no_typecheck {
            eprintln!(
                "⚠ {} warning(s) del checker de tipos (modo `--no-typecheck`):",
                type_errors.len()
            );
            for e in &type_errors {
                eprintln!("  {}", e);
            }
        } else {
            eprintln!(
                "✗ {} — {} error(es) de tipo:",
                path.display(),
                type_errors.len()
            );
            for e in &type_errors {
                eprintln!("  {}", e);
            }
            eprintln!(
                "   Usá `fitz check` para revisar, o `fitz run --no-typecheck {}` para correr igual.",
                path.display()
            );
            std::process::exit(1);
        }
    }

    // Base dir para resolver `import`s: el directorio del archivo que
    // se está ejecutando. Si por algún motivo no podemos derivarlo
    // (path sin parent), caemos al cwd.
    let base_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Fase 2.4 + Fase 4: evaluamos el programa dentro de un
    // `HttpRegistry` activo. Los decoradores HTTP registran rutas
    // ahí mientras corre el eval. Si después de eval el registry
    // tiene rutas, arrancamos el servidor; si no, terminamos como un
    // programa CLI normal.
    //
    // Fase 7.2: el server necesita el AST original también para
    // precomputar el schema OpenAPI (`components.schemas` recorre los
    // `Stmt::TypeDef`). Clonamos antes de moverlo al evaluator.
    let program_for_server = program.clone();
    let (eval_result, registry) = http::with_active_registry(|| {
        evaluator::eval_with_base_and_deps_sync(program, base_dir, dep_registry)
    });

    if let Err(e) = eval_result {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    if !registry.is_empty() {
        // Si el programa declaró `@server(port, host)`, usamos eso;
        // si no, default 127.0.0.1:3000.
        let config = registry.resolved_config();
        let addr = match config.to_socket_addr() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Error en @server: {}", e);
                std::process::exit(1);
            }
        };
        if let Err(e) = http::serve(registry, program_for_server, addr) {
            eprintln!("Error del servidor HTTP: {}", e);
            std::process::exit(1);
        }
    }
}

// ---- Fase 9.y.1 — scaffolding (`fitz new` / `fitz init`) ----

/// Template para el `src/main.fitz` default (CLI hello world).
/// Sigue el estilo del cap 2 de la guía (`examples/guide/02-hola.fitz`):
/// top-level `print(...)` sin `fn main`.
fn template_cli(name: &str) -> String {
    format!(
        "// main.fitz — generado por `fitz new`\n\
         //\n\
         // Tu primer programa Fitz. Corrélo con `fitz run src/main.fitz`.\n\
         // Cuando 9.y.2 aterrice, también vas a poder simplemente `fitz run`\n\
         // desde la raíz del proyecto (lee `fitz.toml` automáticamente).\n\
         \n\
         print(\"Hola desde {name} 🏔️\")\n"
    )
}

/// Template para `src/main.fitz` con `--http`. Servidor mínimo que
/// responde un GET en `/`. Sigue el patrón canónico
/// `@server(...) fn main() => 0` del cap 17 de la guía.
fn template_http(name: &str) -> String {
    format!(
        "// main.fitz — generado por `fitz new --http`\n\
         //\n\
         // Servidor HTTP mínimo. Corrélo con `fitz run src/main.fitz` y\n\
         // probá: curl http://127.0.0.1:3000/\n\
         \n\
         @get(\"/\")\n\
         fn index() -> Str {{\n\
         \x20   return \"Hola desde {name} 🏔️\"\n\
         }}\n\
         \n\
         @server(3000)\n\
         fn main() => 0\n"
    )
}

/// Template para el `.gitignore`. `fitz.lock` NO está acá: el lockfile
/// se commitea (Cargo-style), no se ignora.
fn template_gitignore() -> &'static str {
    "# Artefactos de compilación\n\
     target/\n\
     \n\
     # Binarios generados por `fitz build` adyacentes al fuente.\n\
     # Si publicás un paquete, ajustá esto a tus necesidades.\n\
     *.exe\n\
     *.pdb\n"
}

/// `fitz new <nombre> [--http] [--no-git]` — crea un proyecto Fitz
/// nuevo en una carpeta. Falla si la carpeta ya existe.
fn new_project(name: &str, http: bool, no_git: bool) {
    if !manifest::is_valid_package_name(name) {
        eprintln!(
            "✗ nombre inválido: `{name}`. Debe matchear `^[a-z][a-z0-9_-]{{0,63}}$` \
             (lowercase, empezar con letra, contener solo letras/dígitos/`-`/`_`, máx \
             64 caracteres)."
        );
        std::process::exit(1);
    }

    let target = PathBuf::from(name);
    if target.exists() {
        eprintln!("✗ `{}` ya existe — borralo o elegí otro nombre.", target.display());
        std::process::exit(1);
    }

    scaffold_project(&target, name, http, no_git);
    println!("✓ proyecto Fitz creado en `{}`", target.display());
    println!();
    println!("Para probarlo:");
    println!("  cd {}", target.display());
    println!("  fitz run src/main.fitz");
}

/// `fitz init [--name X] [--http] [--no-git]` — inicializa un proyecto
/// Fitz en el directorio actual. Falla si ya existe un `fitz.toml`.
fn init_project(name_override: Option<&str>, http: bool, no_git: bool) {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer el directorio actual: {e}");
        std::process::exit(1);
    });

    let name = match name_override {
        Some(n) => n.to_string(),
        None => match cwd.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => {
                eprintln!(
                    "✗ no se pudo derivar el nombre del directorio actual. \
                     Pasalo explícito con `--name <nombre>`."
                );
                std::process::exit(1);
            }
        },
    };

    if !manifest::is_valid_package_name(&name) {
        eprintln!(
            "✗ nombre inválido: `{name}`. Debe matchear `^[a-z][a-z0-9_-]{{0,63}}$`. \
             Pasá `--name <nombre-válido>` si el directorio no respeta el formato."
        );
        std::process::exit(1);
    }

    if cwd.join(manifest::MANIFEST_FILE).exists() {
        eprintln!(
            "✗ `{}` ya existe en el directorio actual.",
            manifest::MANIFEST_FILE
        );
        std::process::exit(1);
    }

    scaffold_project(&cwd, &name, http, no_git);
    println!(
        "✓ proyecto Fitz `{name}` inicializado en `{}`",
        cwd.display()
    );
    println!();
    println!("Para probarlo:");
    println!("  fitz run src/main.fitz");
}

/// Common scaffolding: crea `<target>/fitz.toml`, `<target>/src/main.fitz`,
/// `<target>/.gitignore`, y (a menos que `no_git`) corre `git init`.
///
/// Sale del proceso con código 1 ante cualquier error de I/O.
fn scaffold_project(target: &std::path::Path, name: &str, http: bool, no_git: bool) {
    // Crear directorios.
    let src = target.join("src");
    if let Err(e) = fs::create_dir_all(&src) {
        eprintln!("✗ no se pudo crear `{}`: {e}", src.display());
        std::process::exit(1);
    }

    // Escribir fitz.toml.
    let m = match manifest::Manifest::new_default(name) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let toml_text = match m.to_toml_string() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let toml_path = target.join(manifest::MANIFEST_FILE);
    if let Err(e) = fs::write(&toml_path, toml_text) {
        eprintln!("✗ no se pudo escribir `{}`: {e}", toml_path.display());
        std::process::exit(1);
    }

    // Escribir src/main.fitz con el template elegido.
    let main_text = if http {
        template_http(name)
    } else {
        template_cli(name)
    };
    let main_path = src.join("main.fitz");
    if let Err(e) = fs::write(&main_path, main_text) {
        eprintln!("✗ no se pudo escribir `{}`: {e}", main_path.display());
        std::process::exit(1);
    }

    // Escribir .gitignore.
    let gi_path = target.join(".gitignore");
    if let Err(e) = fs::write(&gi_path, template_gitignore()) {
        eprintln!("✗ no se pudo escribir `{}`: {e}", gi_path.display());
        std::process::exit(1);
    }

    // git init (opcional). No abortamos si falla: el proyecto sigue
    // siendo válido sin git; solo lo notamos como warning.
    if !no_git {
        match std::process::Command::new("git")
            .arg("init")
            .arg("--quiet")
            .current_dir(target)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!(
                    "  (aviso: `git init` salió con código {} — el proyecto se creó igual. \
                     Pasá `--no-git` para silenciar este aviso.)",
                    status.code().unwrap_or(-1)
                );
            }
            Err(e) => {
                eprintln!(
                    "  (aviso: no se pudo ejecutar `git init` ({e}). El proyecto se creó \
                     igual. Pasá `--no-git` para silenciar este aviso.)"
                );
            }
        }
    }
}

// ---- Fase 9.y.4 — `fitz add` / `fitz remove` / `fitz update` ----

/// `fitz add <name> [--path <p>] [--git <url> --tag <t>|--rev <r>]`
/// — Fase 9.y.4. Modifica el `[dependencies]` del `fitz.toml` del
/// proyecto actual (cwd o ancestros), preserva formatting con
/// `toml_edit`, y sincroniza el `fitz.lock` resolviendo todas las
/// deps incluida la nueva. Si la dep ya existía, se sobreescribe.
fn add_dep_cmd(
    name: &str,
    path_opt: Option<&str>,
    git_opt: Option<&str>,
    tag_opt: Option<&str>,
    rev_opt: Option<&str>,
) {
    // Build el spec según los flags. clap ya validó conflicts_with /
    // requires entre path/git/tag/rev; igual chequeamos defensivo.
    let spec = match (path_opt, git_opt) {
        (Some(p), None) => manifest::AddDepSpec::Path { path: p.to_string() },
        (None, Some(g)) => {
            let gitref = match (tag_opt, rev_opt) {
                (Some(t), None) => fitz::git_dep::GitRef::Tag(t.to_string()),
                (None, Some(r)) => fitz::git_dep::GitRef::Rev(r.to_string()),
                (Some(_), Some(_)) => {
                    eprintln!("✗ `--tag` y `--rev` son mutuamente exclusivos.");
                    std::process::exit(1);
                }
                (None, None) => {
                    eprintln!(
                        "✗ `--git` requiere también `--tag <tag>` o `--rev <commit>` para \
                         reproducibilidad. `branch` no se soporta intencionalmente."
                    );
                    std::process::exit(1);
                }
            };
            manifest::AddDepSpec::Git {
                url: g.to_string(),
                gitref,
            }
        }
        (Some(_), Some(_)) => {
            // clap debería haber bloqueado esto.
            eprintln!("✗ `--path` y `--git` son mutuamente exclusivos.");
            std::process::exit(1);
        }
        (None, None) => {
            eprintln!(
                "✗ `fitz add` requiere `--path <p>` o `--git <url> --tag <t>`. \
                 Las versiones registry-style (`foo@1.0.0`) llegan en 9.y.5."
            );
            std::process::exit(1);
        }
    };

    let manifest_path = find_local_manifest_or_exit();
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    });
    let new_text = match manifest::add_dep_to_manifest(&text, name, &spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::write(&manifest_path, &new_text) {
        eprintln!("✗ no se pudo escribir `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    }
    println!("✓ agregado `{name}` a `{}`", manifest_path.display());

    // Re-resolver + sync lockfile (manifest mode con file=None).
    // resolve_entry carga el manifest actualizado y resuelve TODAS
    // las deps (la nueva incluida). Si la resolución falla, el
    // manifest queda persistido — el usuario puede `fitz remove`
    // para revertir.
    let resolved = resolve_entry(None);
    sync_lockfile_if_needed(&resolved);
}

/// `fitz remove <name>` — Fase 9.y.4. Quita la entry del manifest y
/// re-sincroniza el lockfile. Si la dep no existía, error claro.
fn remove_dep_cmd(name: &str) {
    let manifest_path = find_local_manifest_or_exit();
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    });
    let (new_text, removed) = match manifest::remove_dep_from_manifest(&text, name) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if !removed {
        eprintln!("✗ la dep `{name}` no estaba en `[dependencies]` de `{}`.", manifest_path.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(&manifest_path, &new_text) {
        eprintln!("✗ no se pudo escribir `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    }
    println!("✓ quitada `{name}` de `{}`", manifest_path.display());

    // Re-resolver para que el lockfile refleje la nueva lista de deps.
    // Si la dep removida era la única, sync_lockfile_if_needed
    // detectará deps vacías y no escribe (pero el lockfile viejo
    // sigue ahí con la entry stale). Limpiamos eso a mano:
    let resolved = resolve_entry(None);
    if let Some(ctx) = &resolved.manifest_ctx {
        if ctx.resolved_deps.is_empty() {
            let lock_path = lockfile::lockfile_path(&ctx.manifest_dir);
            if lock_path.exists() {
                if let Err(e) = std::fs::remove_file(&lock_path) {
                    eprintln!("  (aviso: no se pudo borrar `{}`: {e})", lock_path.display());
                } else {
                    println!("✓ borrado {} (deps vacías)", lock_path.display());
                }
            }
        }
    }
    sync_lockfile_if_needed(&resolved);
}

/// `fitz update [name]` — Fase 9.y.4. Re-resuelve las deps; para
/// git deps, invalida el cache local (borra el dir) y fuerza
/// re-clone con el commit más reciente del tag/rev pedido. Para
/// path deps es no-op (siempre fresh). Sin `name`, actualiza todas;
/// con `name`, solo esa dep.
fn update_deps_cmd(name_filter: Option<&str>) {
    let manifest_path = find_local_manifest_or_exit();

    // Parse del manifest sin tocar el resolver — solo necesitamos el
    // listado de [dependencies] para iterar.
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    });
    let parsed = match manifest::Manifest::parse(&text) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("✗ `{}`: {e}", manifest_path.display());
            std::process::exit(1);
        }
    };

    let mut busted: Vec<String> = Vec::new();
    for (dep_name, dep) in &parsed.dependencies {
        if let Some(filter) = name_filter {
            if dep_name != filter {
                continue;
            }
        }
        // Solo git deps tienen cache para invalidar; path deps son no-op.
        if let manifest::Dependency::Detailed(d) = dep {
            if let Some(url) = &d.git {
                let gitref = match (&d.tag, &d.rev) {
                    (Some(t), None) => fitz::git_dep::GitRef::Tag(t.clone()),
                    (None, Some(r)) => fitz::git_dep::GitRef::Rev(r.clone()),
                    _ => continue, // shape inválido — el resolver reportará
                };
                let cache_path = match fitz::git_dep::cache_path_for(url, &gitref) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("✗ dep `{dep_name}`: no se pudo computar el cache path: {e}");
                        std::process::exit(1);
                    }
                };
                if cache_path.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&cache_path) {
                        eprintln!(
                            "✗ no se pudo borrar el cache de `{dep_name}` en `{}`: {e}",
                            cache_path.display()
                        );
                        std::process::exit(1);
                    }
                    busted.push(dep_name.clone());
                }
            }
        }
    }

    // Validar que el `--name` filter haya matcheado algo (UX: si el
    // user typea mal el nombre, no quiero silencio).
    if let Some(filter) = name_filter {
        if !parsed.dependencies.contains_key(filter) {
            eprintln!("✗ la dep `{filter}` no está en `[dependencies]` de `{}`.", manifest_path.display());
            std::process::exit(1);
        }
    }

    if busted.is_empty() {
        match name_filter {
            Some(_) => println!("(no había nada que actualizar — dep sin cache)"),
            None => println!("(no había git deps con cache para invalidar)"),
        }
    } else {
        println!("✓ cache invalidado para: {}", busted.join(", "));
    }

    // Re-resolver via manifest mode (que va a re-clonar las git deps
    // porque su cache ya no existe) + sync lockfile. Pasamos `None`
    // para que resolve_entry haga `find_manifest` desde el cwd; ya
    // sabemos que el manifest existe (manifest_path arriba lo
    // confirmó).
    let _ = manifest_path; // mantener vivo el lifetime; resolve_entry hace el discover
    let resolved = resolve_entry(None);
    sync_lockfile_if_needed(&resolved);
}

// ---- Fase 9.z.1 — `fitz fmt` ----

/// `fitz fmt [files...] [--check]` — Fase 9.z.1. Formatea archivos
/// `.fitz` al estilo canónico. Sin `files`, formatea todo el proyecto
/// (descubre vía manifest). Con `--check`, no escribe — exit 1 si
/// algún archivo difiere de su forma canónica (modo CI).
///
/// El descubrimiento de archivos en project mode incluye
/// `src/main.fitz` (del `[bin].main`), `src/lib.fitz` (del
/// `[lib].entry`), y cualquier `.fitz` adicional en `src/` (walk
/// recursivo). Excluye `target/` y cualquier dir oculto.
fn fmt_cmd(files: Vec<PathBuf>, check: bool) {
    let targets = if files.is_empty() {
        // Project mode — descubrir vía manifest.
        discover_project_fitz_files()
    } else {
        files
    };

    if targets.is_empty() {
        eprintln!("✗ no se encontraron archivos `.fitz` para formatear.");
        std::process::exit(1);
    }

    // ⚠ Warning loud en modo write (9.z.1.a alpha). El modo --check
    // es read-only y no necesita warning.
    if !check {
        eprintln!(
            "⚠ aviso (9.z.1.a alpha): `fitz fmt` actualmente borra \
             comentarios y blank lines (preservación llega en 9.z.1.b). \
             Asegurate de tener los cambios versionados antes. Usá \
             `fitz fmt --check` para ver diffs sin escribir."
        );
    }

    let mut any_diff = false;
    let mut errors = 0usize;
    for path in &targets {
        match fmt_one_file(path, check) {
            Ok(FmtResult::Unchanged) => {}
            Ok(FmtResult::Wrote) => {
                println!("✓ formateado {}", path.display());
            }
            Ok(FmtResult::WouldChange) => {
                println!("✗ {} no está en formato canónico", path.display());
                any_diff = true;
            }
            Err(e) => {
                eprintln!("✗ {}: {e}", path.display());
                errors += 1;
            }
        }
    }

    if errors > 0 {
        eprintln!("\n{errors} archivo(s) con errores de parsing — fmt no pudo procesarlos.");
        std::process::exit(1);
    }
    if check && any_diff {
        eprintln!("\nuso `fitz fmt` (sin `--check`) para aplicar el formato.");
        std::process::exit(1);
    }
}

enum FmtResult {
    /// El archivo ya estaba en forma canónica.
    Unchanged,
    /// Escribimos el archivo con la forma canónica.
    Wrote,
    /// `--check` mode: el archivo cambiaría si se formateara.
    WouldChange,
}

fn fmt_one_file(path: &std::path::Path, check_only: bool) -> Result<FmtResult, String> {
    let source = fs::read_to_string(path).map_err(|e| format!("no se pudo leer: {e}"))?;
    let formatted = fmt::format_source(&source).map_err(|e| e.to_string())?;
    if formatted == source {
        return Ok(FmtResult::Unchanged);
    }
    if check_only {
        return Ok(FmtResult::WouldChange);
    }
    fs::write(path, &formatted).map_err(|e| format!("no se pudo escribir: {e}"))?;
    Ok(FmtResult::Wrote)
}

/// Descubre archivos `.fitz` del proyecto actual via manifest. Lee
/// `[bin].main` y `[lib].entry` (si existen) + walk recursivo de
/// `src/`. Excluye `target/` y dirs ocultos (`.git/`, etc.).
fn discover_project_fitz_files() -> Vec<PathBuf> {
    let manifest_path = find_local_manifest_or_exit();
    let manifest_dir = manifest_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let mut targets: Vec<PathBuf> = Vec::new();
    let src_dir = manifest_dir.join("src");
    if src_dir.is_dir() {
        collect_fitz_recursive(&src_dir, &mut targets);
    }
    // Dedup por path canonicalizado para evitar formatear el mismo
    // archivo dos veces si aparece como `[bin].main` y también en
    // el walk de `src/`.
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    targets.retain(|p| {
        let canon = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        seen.insert(canon)
    });
    targets.sort();
    targets
}

fn collect_fitz_recursive(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // Skip dirs ocultos (`.git`, `.fitz-cache`) y `target/`.
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            collect_fitz_recursive(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("fitz") {
            out.push(path);
        }
    }
}

/// Helper compartido por add/remove/update: encuentra el `fitz.toml`
/// del proyecto actual o sale con error claro.
fn find_local_manifest_or_exit() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer el directorio actual: {e}");
        std::process::exit(1);
    });
    match manifest::find_manifest(&cwd) {
        Some(p) => p,
        None => {
            eprintln!(
                "✗ no se encontró `{}` en `{}` ni en directorios padre. \
                 Creá un proyecto con `fitz new <nombre>` / `fitz init` antes de \
                 usar `add`/`remove`/`update`.",
                manifest::MANIFEST_FILE,
                cwd.display()
            );
            std::process::exit(1);
        }
    }
}
