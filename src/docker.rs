// docker.rs — Fase 12.4 (Dockerfile autogenerado + `fitz docker`)
//
// Sub-comando `fitz docker init` produce tres archivos en el directorio
// del manifest:
//   - `Dockerfile` multi-stage: builder con la imagen oficial de Fitz
//     (`ghcr.io/thegreekman76/fitz:<tag>`) que invoca `fitz build`, +
//     runtime `gcr.io/distroless/cc-debian12` con solo el binario.
//   - `.dockerignore` con lo típico que NO debe entrar al build context.
//   - `docker-compose.yml` smart: si el programa usa `db.connect(...)`,
//     suma service `postgres:16-alpine` con healthcheck + DATABASE_URL.
//
// 12.4.a — alcance MVP: detección AST-only de `@server(port)` y
// `db.connect(...)`. Multi-archivo solo el entry point (el smart compose
// no recursa en módulos importados — el caso típico tiene los decoradores
// en el archivo principal).
//
// 12.4.b — futuro: Python bundleado (fallback a debian:bookworm-slim),
// healthchecks via `@healthz/@readyz`, restart policies via `@cron`,
// `fitz docker build [--tag X]` wrapper.

use crate::ast::{Expr, Program, Stmt, StrPart};
use std::fs;
use std::path::{Path, PathBuf};

/// Snapshot del shape del programa relevante para Docker.
///
/// `package_name` viene del `fitz.toml`; el resto se infiere recorriendo
/// el AST del entry point.
#[derive(Debug, Clone, PartialEq)]
pub struct DockerShape {
    pub package_name: String,
    /// `Some(port)` si encuentra `@server(N, ...)` con `N` Int literal en
    /// `[1, 65535]`. `None` si no hay `@server` (programa CLI) o si el
    /// port es expresión no literal.
    pub server_port: Option<u16>,
    /// `true` si encuentra alguna llamada `db.X(...)` en el AST (típico
    /// `db.connect(...)`). Heurística generosa: cualquier method call
    /// con receptor `db` cuenta.
    pub uses_db: bool,
}

/// Recorre el AST del programa y produce un `DockerShape` con lo que
/// haga falta para parametrizar los templates.
pub fn detect_shape(program: &Program, package_name: String) -> DockerShape {
    let server_port = find_server_port(program);
    let uses_db = program.iter().any(stmt_uses_db);
    DockerShape {
        package_name,
        server_port,
        uses_db,
    }
}

fn find_server_port(program: &Program) -> Option<u16> {
    for stmt in program {
        if let Stmt::FnDef { decorators, .. } = stmt {
            for deco in decorators {
                if deco.name != "server" {
                    continue;
                }
                if let Some(Expr::Int(n, _)) = deco.args.first() {
                    if (1..=65535).contains(n) {
                        return Some(*n as u16);
                    }
                }
            }
        }
    }
    None
}

fn stmt_uses_db(s: &Stmt) -> bool {
    match s {
        Stmt::Expr(e, _) | Stmt::Return(e, _) => expr_uses_db(e),
        Stmt::Assign { value, .. } => expr_uses_db(value),
        Stmt::FnDef { body, .. } => body.iter().any(stmt_uses_db),
        Stmt::While {
            condition, body, ..
        } => expr_uses_db(condition) || body.iter().any(stmt_uses_db),
        Stmt::For { iter, body, .. } => expr_uses_db(iter) || body.iter().any(stmt_uses_db),
        _ => false,
    }
}

fn expr_uses_db(e: &Expr) -> bool {
    match e {
        Expr::Call { callee, args, .. } => {
            if let Expr::Field { object, .. } = callee.as_ref() {
                if let Expr::Ident(recv, _) = object.as_ref() {
                    if recv == "db" {
                        return true;
                    }
                }
            }
            expr_uses_db(callee) || args.iter().any(expr_uses_db)
        }
        Expr::Field { object, .. } => expr_uses_db(object),
        Expr::Index { object, index, .. } => expr_uses_db(object) || expr_uses_db(index),
        Expr::BinOp { left, right, .. } => expr_uses_db(left) || expr_uses_db(right),
        Expr::UnaryOp { operand, .. } => expr_uses_db(operand),
        Expr::Await(inner, _) | Expr::Try(inner, _) | Expr::Ok(inner, _) | Expr::Err(inner, _) => {
            expr_uses_db(inner)
        }
        Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_db),
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            expr_uses_db(condition)
                || then.iter().any(stmt_uses_db)
                || else_.as_ref().is_some_and(|b| b.iter().any(stmt_uses_db))
        }
        Expr::Match { value, arms, .. } => {
            expr_uses_db(value) || arms.iter().any(|a| a.body.iter().any(stmt_uses_db))
        }
        Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
            StrPart::Lit(_) => false,
            StrPart::Expr(inner, _) => expr_uses_db(inner),
        }),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// Renderiza el `Dockerfile` multi-stage. El runtime es siempre
/// `gcr.io/distroless/cc-debian12` en 12.4.a — el binario standalone que
/// emite `fitz build` no necesita Python ni shell. `EXPOSE` solo cuando
/// hay `@server(port)`.
pub fn render_dockerfile(shape: &DockerShape) -> String {
    let mut out = String::new();
    out.push_str("# Dockerfile generado por `fitz docker init` (Fase 12.4).\n");
    out.push_str("#\n");
    out.push_str("# Multi-stage:\n");
    out.push_str("#   Stage 1 (builder) — imagen oficial de Fitz con `fitz` + Rust toolchain\n");
    out.push_str("#     pre-instalados; compila el `.fitz` a binario nativo Linux.\n");
    out.push_str("#   Stage 2 (runtime) — `gcr.io/distroless/cc-debian12` minimal. Solo glibc +\n");
    out.push_str("#     libgcc + ca-certificates. Sin shell, sin package manager.\n");
    out.push_str("#\n");
    out.push_str("# Build:\n");
    out.push_str(&format!("#   docker build -t {} .\n", shape.package_name));
    if let Some(port) = shape.server_port {
        out.push_str("#\n");
        out.push_str(&format!(
            "# Correr (HTTP):\n#   docker run --rm -p {0}:{0} {1}\n",
            port, shape.package_name,
        ));
    }
    out.push_str("#\n");
    out.push_str("# Pineá la versión de Fitz para reproducibilidad:\n");
    out.push_str(&format!(
        "#   docker build --build-arg FITZ_TAG=v0.12.1 -t {} .\n",
        shape.package_name,
    ));
    out.push('\n');

    out.push_str("ARG FITZ_TAG=latest\n\n");

    out.push_str("# ---- Stage 1: builder ----------------------------------------------\n");
    out.push_str("FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder\n\n");
    out.push_str("WORKDIR /app\n");
    out.push_str("COPY fitz.toml ./\n");
    out.push_str("COPY src/ ./src/\n\n");
    out.push_str("# `fitz build` lee el manifest del cwd (Fase 9.y.2) y emite el binario\n");
    out.push_str("# a `target/release/<package.name>`.\n");
    out.push_str("RUN fitz build\n\n");

    out.push_str("# ---- Stage 2: runtime ----------------------------------------------\n");
    out.push_str("FROM gcr.io/distroless/cc-debian12\n\n");
    out.push_str(&format!(
        "COPY --from=builder /app/target/release/{} /usr/local/bin/app\n\n",
        shape.package_name,
    ));

    if let Some(port) = shape.server_port {
        out.push_str(&format!(
            "# El programa declara `@server({}, ...)`. Para que `-p {0}:{0}` desde el\n",
            port,
        ));
        out.push_str(
            "# host route requests adentro del container, declará `@server(N, \"0.0.0.0\")`\n",
        );
        out.push_str("# en el código Fitz (bind a todas las interfaces).\n");
        out.push_str(&format!("EXPOSE {}\n\n", port));
    }

    out.push_str("ENTRYPOINT [\"/usr/local/bin/app\"]\n");

    out
}

/// `.dockerignore` con lo típico que NO debe entrar al build context:
/// outputs del compilador, configs locales, secretos, archivos del editor.
/// Independiente del shape del programa.
pub fn render_dockerignore() -> String {
    let mut out = String::new();
    out.push_str("# Generado por `fitz docker init` (Fase 12.4).\n");
    out.push_str("# Lo que NO debe entrar al build context del Dockerfile.\n\n");
    out.push_str("# Outputs del compilador\n");
    out.push_str("target/\n");
    out.push_str(".fitz/\n");
    out.push_str("fitz.lock\n\n");
    out.push_str("# Git + configs locales\n");
    out.push_str(".git/\n");
    out.push_str(".gitignore\n");
    out.push_str(".github/\n\n");
    out.push_str("# Secretos / env locales\n");
    out.push_str(".env\n");
    out.push_str(".env.*\n");
    out.push_str("!.env.example\n\n");
    out.push_str("# Editor + OS\n");
    out.push_str(".vscode/\n");
    out.push_str(".idea/\n");
    out.push_str(".DS_Store\n");
    out.push_str("Thumbs.db\n\n");
    out.push_str("# Docker mismo (no necesita estar en el build context)\n");
    out.push_str("Dockerfile\n");
    out.push_str("docker-compose.yml\n");
    out.push_str(".dockerignore\n\n");
    out.push_str("# Python (si el programa usa interop)\n");
    out.push_str("__pycache__/\n");
    out.push_str("*.pyc\n");
    out.push_str(".venv/\n");
    out.push_str("venv/\n\n");
    out.push_str("# Boilerplate misc\n");
    out.push_str("node_modules/\n");
    out
}

/// `docker-compose.yml` smart:
/// - Si `uses_db`, suma service `db` con `postgres:16-alpine` + healthcheck
///   y `DATABASE_URL` en el env del service principal.
/// - Si `server_port` es `Some`, mapea ese port. Si es `None`, sin `ports:`
///   (programa CLI dentro de un container).
pub fn render_compose(shape: &DockerShape) -> String {
    let mut out = String::new();
    out.push_str("# Generado por `fitz docker init` (Fase 12.4).\n");
    out.push_str("#\n");
    out.push_str("# Uso:\n");
    out.push_str("#   docker compose up --build\n");
    if shape.uses_db {
        out.push_str("#\n");
        out.push_str(
            "# Postgres queda con credenciales fitz/fitz/fitz por default. Sobreescribilas\n",
        );
        out.push_str(
            "# con un `.env` adyacente (POSTGRES_USER / POSTGRES_PASSWORD / POSTGRES_DB).\n",
        );
    }
    out.push_str("\nservices:\n");

    if shape.uses_db {
        out.push_str("  db:\n");
        out.push_str("    image: postgres:16-alpine\n");
        out.push_str(&format!("    container_name: {}-db\n", shape.package_name,));
        out.push_str("    environment:\n");
        out.push_str("      POSTGRES_USER: ${POSTGRES_USER:-fitz}\n");
        out.push_str("      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-fitz}\n");
        out.push_str("      POSTGRES_DB: ${POSTGRES_DB:-fitz}\n");
        out.push_str("    volumes:\n");
        out.push_str("      - pgdata:/var/lib/postgresql/data\n");
        out.push_str("    healthcheck:\n");
        out.push_str(
            "      test: [\"CMD-SHELL\", \"pg_isready -U ${POSTGRES_USER:-fitz} -d ${POSTGRES_DB:-fitz}\"]\n",
        );
        out.push_str("      interval: 5s\n");
        out.push_str("      timeout: 5s\n");
        out.push_str("      retries: 5\n\n");
    }

    out.push_str("  app:\n");
    out.push_str("    build: .\n");
    out.push_str(&format!("    container_name: {}\n", shape.package_name));

    if shape.uses_db {
        out.push_str("    environment:\n");
        out.push_str(
            "      DATABASE_URL: \"postgres://${POSTGRES_USER:-fitz}:${POSTGRES_PASSWORD:-fitz}@db:5432/${POSTGRES_DB:-fitz}?sslmode=disable\"\n",
        );
    }

    if let Some(port) = shape.server_port {
        out.push_str("    ports:\n");
        out.push_str(&format!("      - \"{0}:{0}\"\n", port));
    }

    if shape.uses_db {
        out.push_str("    depends_on:\n");
        out.push_str("      db:\n");
        out.push_str("        condition: service_healthy\n");
    }

    if shape.uses_db {
        out.push_str("\nvolumes:\n");
        out.push_str("  pgdata:\n");
    }

    out
}

// ---------------------------------------------------------------------------
// Init — escribe los 3 archivos
// ---------------------------------------------------------------------------

/// Resultado de `init`: qué se escribió y qué se saltó porque ya existía
/// (y no se pasó `force = true`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InitResult {
    pub written: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

/// Escribe `Dockerfile` + `.dockerignore` + `docker-compose.yml` en
/// `target_dir`. Si un archivo ya existe y `force == false`, lo skipea
/// (registrado en `skipped`); si `force == true`, lo sobrescribe.
pub fn init(target_dir: &Path, shape: &DockerShape, force: bool) -> Result<InitResult, String> {
    if !target_dir.is_dir() {
        return Err(format!(
            "directorio destino no existe o no es un directorio: {}",
            target_dir.display(),
        ));
    }

    let mut result = InitResult::default();

    let files: Vec<(&str, String)> = vec![
        ("Dockerfile", render_dockerfile(shape)),
        (".dockerignore", render_dockerignore()),
        ("docker-compose.yml", render_compose(shape)),
    ];

    for (filename, contents) in files {
        let path = target_dir.join(filename);
        if path.exists() && !force {
            result.skipped.push(path);
            continue;
        }
        fs::write(&path, contents).map_err(|e| format!("escribiendo {}: {}", path.display(), e))?;
        result.written.push(path);
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer, parser};
    use tempfile::tempdir;

    fn shape_from_source(source: &str, name: &str) -> DockerShape {
        let tokens = lexer::tokenize(source).expect("lex");
        let program = parser::parse(tokens).expect("parse");
        detect_shape(&program, name.to_string())
    }

    #[test]
    fn detect_shape_cli_puro_sin_server_sin_db() {
        let shape = shape_from_source("print(\"hola\")", "demo");
        assert_eq!(shape.package_name, "demo");
        assert_eq!(shape.server_port, None);
        assert!(!shape.uses_db);
    }

    #[test]
    fn detect_shape_server_con_port_literal() {
        let src = r#"
@server(3000)
fn main() => 0

@get("/")
fn root() => "ok"
"#;
        let shape = shape_from_source(src, "demo");
        assert_eq!(shape.server_port, Some(3000));
        assert!(!shape.uses_db);
    }

    #[test]
    fn detect_shape_server_con_port_y_host() {
        let src = r#"
@server(8080, "0.0.0.0")
fn main() => 0
"#;
        let shape = shape_from_source(src, "demo");
        assert_eq!(shape.server_port, Some(8080));
    }

    #[test]
    fn detect_shape_server_sin_port_default_none() {
        // `@server()` sin args válidos → no detectamos port literal;
        // el template usa la lógica default (sin EXPOSE / sin ports).
        let src = r#"
@server()
fn main() => 0
"#;
        let shape = shape_from_source(src, "demo");
        assert_eq!(shape.server_port, None);
    }

    #[test]
    fn detect_shape_uses_db_connect() {
        let src = r#"
async fn main() -> Null {
    let conn = db.connect("postgres://x")
    print("ok")
}
"#;
        let shape = shape_from_source(src, "demo");
        assert!(shape.uses_db);
    }

    #[test]
    fn detect_shape_uses_db_query_chain() {
        // `db.X(...)` adentro de cualquier expresión cuenta.
        let src = r#"
async fn run() -> Null {
    let xs = db.query("select 1")
    print("ok")
}
"#;
        let shape = shape_from_source(src, "demo");
        assert!(shape.uses_db);
    }

    #[test]
    fn detect_shape_db_local_no_es_global() {
        // `db.X(...)` donde `db` es ident local: la heurística marca igual
        // como `uses_db = true` (lo mismo que codegen). 12.4.a privilegia
        // simplicidad sobre precisión — el helper Docker es de inferencia,
        // no de chequeo estático.
        let src = r#"
fn run(db) -> Int {
    let _ = db.query("x")
    0
}
"#;
        let shape = shape_from_source(src, "demo");
        assert!(shape.uses_db);
    }

    #[test]
    fn render_dockerfile_sin_server_no_emite_expose() {
        let shape = DockerShape {
            package_name: "demo".into(),
            server_port: None,
            uses_db: false,
        };
        let dockerfile = render_dockerfile(&shape);
        assert!(!dockerfile.contains("EXPOSE"));
        assert!(dockerfile.contains("FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder"));
        assert!(dockerfile.contains("FROM gcr.io/distroless/cc-debian12"));
        assert!(
            dockerfile.contains("COPY --from=builder /app/target/release/demo /usr/local/bin/app")
        );
        assert!(dockerfile.contains("ENTRYPOINT [\"/usr/local/bin/app\"]"));
    }

    #[test]
    fn render_dockerfile_con_server_emite_expose_y_port_runtime() {
        let shape = DockerShape {
            package_name: "myapp".into(),
            server_port: Some(8080),
            uses_db: false,
        };
        let dockerfile = render_dockerfile(&shape);
        assert!(dockerfile.contains("EXPOSE 8080"));
        // Comentario con el ejemplo `docker run -p`.
        assert!(dockerfile.contains("docker run --rm -p 8080:8080 myapp"));
    }

    #[test]
    fn render_dockerignore_excluye_target_env_y_git() {
        let dockerignore = render_dockerignore();
        assert!(dockerignore.contains("target/"));
        assert!(dockerignore.contains(".git/"));
        assert!(dockerignore.contains(".env"));
        assert!(dockerignore.contains("!.env.example"));
        assert!(dockerignore.contains("Dockerfile"));
        assert!(dockerignore.contains("__pycache__/"));
    }

    #[test]
    fn render_compose_sin_db_sin_server_solo_app() {
        let shape = DockerShape {
            package_name: "cli".into(),
            server_port: None,
            uses_db: false,
        };
        let compose = render_compose(&shape);
        assert!(compose.contains("services:"));
        assert!(compose.contains("  app:"));
        assert!(compose.contains("    build: ."));
        assert!(compose.contains("container_name: cli"));
        // Sin DB → sin service db, sin volume pgdata, sin DATABASE_URL.
        assert!(!compose.contains("  db:"));
        assert!(!compose.contains("postgres:16-alpine"));
        assert!(!compose.contains("DATABASE_URL"));
        assert!(!compose.contains("volumes:"));
        // Sin server → sin ports.
        assert!(!compose.contains("    ports:"));
    }

    #[test]
    fn render_compose_con_server_emite_ports() {
        let shape = DockerShape {
            package_name: "web".into(),
            server_port: Some(3000),
            uses_db: false,
        };
        let compose = render_compose(&shape);
        assert!(compose.contains("    ports:\n      - \"3000:3000\""));
    }

    #[test]
    fn render_compose_con_db_emite_postgres_y_database_url() {
        let shape = DockerShape {
            package_name: "api".into(),
            server_port: Some(3000),
            uses_db: true,
        };
        let compose = render_compose(&shape);
        assert!(compose.contains("  db:"));
        assert!(compose.contains("image: postgres:16-alpine"));
        assert!(compose.contains("container_name: api-db"));
        assert!(compose.contains("pg_isready"));
        assert!(compose.contains("healthcheck:"));
        assert!(compose.contains("DATABASE_URL:"));
        assert!(compose.contains("postgres://${POSTGRES_USER:-fitz}"));
        assert!(compose.contains("@db:5432"));
        assert!(compose.contains("depends_on:"));
        assert!(compose.contains("service_healthy"));
        assert!(compose.contains("\nvolumes:\n  pgdata:"));
    }

    #[test]
    fn render_compose_con_db_sin_server_no_emite_ports() {
        let shape = DockerShape {
            package_name: "worker".into(),
            server_port: None,
            uses_db: true,
        };
        let compose = render_compose(&shape);
        assert!(compose.contains("  db:"));
        assert!(!compose.contains("    ports:"));
    }

    #[test]
    fn init_escribe_tres_archivos_en_dir_vacio() {
        let dir = tempdir().expect("tempdir");
        let shape = DockerShape {
            package_name: "demo".into(),
            server_port: Some(3000),
            uses_db: false,
        };
        let result = init(dir.path(), &shape, false).expect("init ok");
        assert_eq!(result.written.len(), 3);
        assert!(result.skipped.is_empty());
        assert!(dir.path().join("Dockerfile").exists());
        assert!(dir.path().join(".dockerignore").exists());
        assert!(dir.path().join("docker-compose.yml").exists());
    }

    #[test]
    fn init_skipea_archivos_existentes_sin_force() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("Dockerfile"), "viejo contenido").unwrap();
        let shape = DockerShape {
            package_name: "demo".into(),
            server_port: None,
            uses_db: false,
        };
        let result = init(dir.path(), &shape, false).expect("init ok");
        assert_eq!(result.written.len(), 2);
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(
            result.skipped[0].file_name().and_then(|s| s.to_str()),
            Some("Dockerfile"),
        );
        // Y el archivo viejo se preservó.
        let viejo = fs::read_to_string(dir.path().join("Dockerfile")).unwrap();
        assert_eq!(viejo, "viejo contenido");
    }

    #[test]
    fn init_force_sobrescribe_archivos_existentes() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join(".dockerignore"), "viejo").unwrap();
        let shape = DockerShape {
            package_name: "demo".into(),
            server_port: None,
            uses_db: false,
        };
        let result = init(dir.path(), &shape, true).expect("init ok");
        assert_eq!(result.written.len(), 3);
        assert!(result.skipped.is_empty());
        let nuevo = fs::read_to_string(dir.path().join(".dockerignore")).unwrap();
        assert!(nuevo.contains("target/"));
    }

    #[test]
    fn init_error_si_target_dir_no_existe() {
        let shape = DockerShape {
            package_name: "demo".into(),
            server_port: None,
            uses_db: false,
        };
        let result = init(Path::new("/no/existe/aca"), &shape, false);
        assert!(result.is_err());
    }
}
