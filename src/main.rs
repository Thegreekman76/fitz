// main.rs — Entry point del compilador/intérprete de Fitz
//
// Por ahora solo estructura básica y CLI.
// Los módulos se irán implementando en las fases siguientes.

mod lexer;      // Fase 2.1 — tokenización
mod ast;        // Fase 2.2 — definición del AST
mod parser;     // Fase 2.3 — construcción del AST
mod value;      // Fase 2.4 — valores en runtime
mod env;        // Fase 2.4 — entornos / scopes
mod evaluator;  // Fase 2.4 — ejecución
mod error;      // manejo de errores del compilador
mod http;       // Fase 4 — HTTP nativo (registry + runtime)
mod types;      // Fase 5.2 — sistema de tipos resuelto + checker base
mod codegen;    // Fase 5b.1 — transpile AST → Rust → binario
mod openapi;    // Fase 7.1 — generador OpenAPI 3.1

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
    /// Ejecutar un archivo .fitz
    Run {
        /// Archivo a ejecutar
        file: PathBuf,
        /// Saltar el chequeo estático de tipos. Sin esta flag los
        /// errores del checker abortan la ejecución (modo strict).
        #[arg(long)]
        no_typecheck: bool,
    },
    /// Compilar a binario (Fase 5)
    Build {
        /// Archivo a compilar
        file: PathBuf,
    },
    /// Verificar tipos y sintaxis
    Check {
        /// Archivo a verificar
        file: PathBuf,
    },
    /// Emite el schema OpenAPI 3.1 del programa a stdout
    Openapi {
        /// Archivo a inspeccionar
        file: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file, no_typecheck } => {
            run_file(&file, no_typecheck);
        }
        Commands::Build { file } => {
            build_file(&file);
        }
        Commands::Check { file } => {
            check_file(&file);
        }
        Commands::Openapi { file } => {
            openapi_file(&file);
        }
    }
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
    let (_env, type_errors) = types::check_program(&program);
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

    let schema = openapi::generate_openapi(&registry, &program);
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
    let (_env, errors) = types::check_program(&program);
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
/// `cargo build --release` → copia el binario adyacente al archivo
/// fuente.
///
/// Desde 5b.5 generamos un Cargo project en lugar de invocar rustc
/// directamente. Razones: (a) los imports cross-archivo necesitan
/// múltiples `.rs` con `mod`, lo que se hace nativo con cargo; (b) cuando
/// llegue 5b.6 con HTTP, sumamos `axum`/`tokio`/`serde_json` al
/// `Cargo.toml` generado sin reescribir pipeline; (c) cargo cachea
/// incremental, lo que abarata segunda compilación. Trade-off: la
/// primera compilación cuesta ~1-2s más que `rustc` directo.
fn build_file(path: &PathBuf) {
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
    let (env, type_errors) = types::check_program(&program);
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
    let bin_out = path
        .parent()
        .map(|p| p.join(&output_filename))
        .unwrap_or_else(|| PathBuf::from(&output_filename));
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
    let (_type_env, type_errors) = types::check_program(&program);
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
