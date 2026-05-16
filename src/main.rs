// main.rs — Entry point del compilador/intérprete de Fitz.
//
// Los módulos viven en `src/lib.rs` desde Fase 9.x.1.b (refactor
// lib + bin para que `fitz-lsp` pueda reusarlos sin compilación
// duplicada). Acá solo importamos lo que el CLI consume.

use fitz::{codegen, evaluator, http, lexer, manifest, openapi, parser, types};

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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file, no_typecheck } => {
            let resolved = resolve_entry(file);
            run_file(&resolved.entry, no_typecheck);
        }
        Commands::Build { file } => {
            let resolved = resolve_entry(file);
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
            build_file(&resolved.entry, override_dest.as_deref());
        }
        Commands::Check { file } => {
            let resolved = resolve_entry(file);
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
    }
}

// ---- Fase 9.y.2 — resolución de entry point (single-file vs manifest) ----

/// Contexto del manifest cargado durante `resolve_entry`. Cuando está
/// presente, el caller sabe que el run/build/check arrancó desde un
/// proyecto Fitz (no en modo single-file).
///
/// Por ahora solo lo consume `build_file` para decidir el destino del
/// binario. Los otros call sites (`run_file`, `check_file`) ignoran el
/// ctx — el entry resuelto ya es suficiente para reproducir el
/// comportamiento single-file pre-9.y.2.
struct ManifestCtx {
    manifest: manifest::Manifest,
    manifest_dir: PathBuf,
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
    ResolvedEntry {
        entry,
        manifest_ctx: Some(ManifestCtx {
            manifest,
            manifest_dir,
        }),
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
fn build_file(path: &PathBuf, override_dest: Option<&std::path::Path>) {
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
    let project = match codegen::generate_project(path, &program, &env) {
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

fn run_file(path: &PathBuf, no_typecheck: bool) {
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
        evaluator::eval_with_base_sync(program, base_dir)
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
