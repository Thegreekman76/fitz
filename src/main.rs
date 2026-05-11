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
        Commands::Run { file } => {
            run_file(&file);
        }
        Commands::Build { file } => {
            println!("🚧 Compilador en construcción — Fase 5");
            println!("   Por ahora usá: fitz run {}", file.display());
        }
        Commands::Check { file } => {
            println!("🚧 Type checker en construcción");
            println!("   Archivo: {}", file.display());
        }
    }
}

fn run_file(path: &PathBuf) {
    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error leyendo {}: {}", path.display(), e);
        std::process::exit(1);
    });

    println!("🏔️  Fitz v0.1.0");
    println!("   Ejecutando: {}", path.display());
    println!("   (intérprete en construcción — Fase 2)\n");

    // Fase 2.1: lexer
    let tokens = match lexer::tokenize(&source) {
        Ok(tokens) => {
            println!("--- Tokens ---");
            for tok in &tokens {
                println!("  {:>4}:{:<3}  {:?}", tok.line, tok.column, tok.token);
            }
            tokens
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Fase 2.3: parser
    let program = match parser::parse(tokens) {
        Ok(program) => {
            println!("\n--- AST ---");
            for (i, stmt) in program.iter().enumerate() {
                println!("  [{}] {:#?}", i, stmt);
            }
            program
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Fase 2.4: evaluador
    // Base dir para resolver `import`s: el directorio del archivo que
    // se está ejecutando. Si por algún motivo no podemos derivarlo
    // (path sin parent), caemos al cwd.
    let base_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    println!("\n--- Ejecución ---");
    if let Err(e) = evaluator::eval_with_base(program, base_dir) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
