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

/// `fitz build <archivo>` — Fase 5b.1. Compila el .fitz a binario
/// nativo via `rustc`. Flujo: lex → parse → checker (strict) →
/// codegen a Rust → `rustc` → copia el binario adyacente al
/// archivo fuente.
///
/// El Rust generado queda en `target/fitz-build/<nombre>/main.rs`
/// para inspección manual. Si rustc falla, el output crudo se
/// imprime tal cual.
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

    // Codegen.
    let rust_code = match codegen::generate_rust(&program, &env) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ codegen: {}", e);
            eprintln!("   (Fase 5b.1 soporta un subset; el resto llega en 5b.2+.)");
            std::process::exit(1);
        }
    };

    // Directorios.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("fitz_build");
    let build_dir = PathBuf::from("target")
        .join("fitz-build")
        .join(stem);
    if let Err(e) = fs::create_dir_all(&build_dir) {
        eprintln!("Error creando {}: {}", build_dir.display(), e);
        std::process::exit(1);
    }
    let rust_src = build_dir.join("main.rs");
    if let Err(e) = fs::write(&rust_src, &rust_code) {
        eprintln!("Error escribiendo {}: {}", rust_src.display(), e);
        std::process::exit(1);
    }

    // Output binario adyacente al .fitz: `hello.fitz` → `hello`
    // (Linux/macOS) o `hello.exe` (Windows).
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin_out = path
        .parent()
        .map(|p| p.join(&bin_name))
        .unwrap_or_else(|| PathBuf::from(&bin_name));

    // Invocamos rustc directamente. Para 5b.1 no necesitamos
    // dependencias externas; cuando lleguen (axum/tokio en 5b.6),
    // pasamos a generar Cargo.toml + src/main.rs y llamamos cargo.
    let output = std::process::Command::new("rustc")
        .args([
            "--edition",
            "2021",
            "-O",
        ])
        .arg(&rust_src)
        .arg("-o")
        .arg(&bin_out)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error invocando rustc: {}", e);
            eprintln!("   ¿Tenés rustc en el PATH? (`rustup` lo provee.)");
            std::process::exit(1);
        }
    };

    if !output.status.success() {
        eprintln!("✗ rustc falló al compilar el código generado:");
        eprintln!("   (revisá {} para ver qué se intentó compilar.)", rust_src.display());
        eprintln!("--- stderr de rustc ---");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
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
    let (eval_result, registry) = http::with_active_registry(|| {
        evaluator::eval_with_base(program, base_dir)
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
        if let Err(e) = http::serve(registry, addr) {
            eprintln!("Error del servidor HTTP: {}", e);
            std::process::exit(1);
        }
    }
}
