// main.rs — Entry point del compilador/intérprete de Fitz
//
// Por ahora solo estructura básica y CLI.
// Los módulos se irán implementando en las fases siguientes.

mod lexer;      // Fase 2.1 — tokenización
mod ast;        // Fase 2.2 — definición del AST
mod parser;     // Fase 2.3 — construcción del AST
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

    // TODO Fase 2:
    // let tokens = lexer::tokenize(&source);
    // let ast = parser::parse(tokens);
    // evaluator::eval(ast);

    // Por ahora, mostrar el source como placeholder
    println!("--- Fuente ---");
    println!("{}", source);
}
