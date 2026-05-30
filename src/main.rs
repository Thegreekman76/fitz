// main.rs — Entry point del compilador/intérprete de Fitz.
//
// Los módulos viven en `src/lib.rs` desde Fase 9.x.1.b (refactor
// lib + bin para que `fitz-lsp` pueda reusarlos sin compilación
// duplicada). Acá solo importamos lo que el CLI consume.

use fitz::{
    ast, codegen, cron_jobs, db, error, evaluator, fmt, http, launcher_template, lexer, lint,
    lockfile, manifest, migrations, openapi, parser, pbs, pyi_loader, testing, types,
};

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
        /// Fase 8.b — Bundlear CPython embebido en el binario final.
        /// El output es un binario standalone que NO requiere Python
        /// instalado en el destino. Internamente: launcher Datasette-
        /// style + tarball PBS + real binary, todos embebidos.
        /// Requiere que el programa use `from python import ...`.
        /// Suma ~30 MB (Windows) / ~45 MB (Linux x64) al binario.
        #[arg(long = "bundle-python")]
        bundle_python: bool,
        /// Fase 8.c — Bundlear paquetes pip junto al CPython embebido.
        /// Flag repetible: `--bundle-pip sqlalchemy --bundle-pip psycopg2`.
        /// Acepta version pin nativa de pip:
        /// `--bundle-pip "sqlalchemy==2.0.0"`. Implica `--bundle-python`
        /// automáticamente. Builder ejecuta `pip install --target` al
        /// build time, empaqueta el resultado en un tarball secundario
        /// embebido en el launcher. En primer run del binario, los
        /// paquetes se extraen a `<TMPDIR>/fitz-py-<hash>/python/Lib/
        /// site-packages/` (Windows) o equivalente Unix.
        #[arg(long = "bundle-pip", value_name = "PACKAGE")]
        bundle_pip: Vec<String>,
        /// Bundlear paquetes pip leídos desde un `requirements.txt`.
        /// Flag repetible: `--bundle-pip-requirements requirements.txt
        /// --bundle-pip-requirements requirements-dev.txt`. Implica
        /// `--bundle-python` automáticamente. El archivo se pasa
        /// directo a `pip install -r <file>`, así que toda la sintaxis
        /// nativa de pip (comentarios `#`, `-r other.txt` includes,
        /// version pins, `--hash`, etc.) funciona sin cambios.
        /// Combinable con `--bundle-pip <pkg>`: pip los acumula.
        #[arg(long = "bundle-pip-requirements", value_name = "FILE")]
        bundle_pip_requirements: Vec<PathBuf>,
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
    /// pyi-stubs (v0.9.39) — Genera `type` Fitz a partir de un
    /// archivo `.pyi` (stubs Python PEP 484/561). Parsea las
    /// `class` top-level y emite los `type` Fitz equivalentes,
    /// listos para `import`. NO requiere feature `python` — el
    /// parser .pyi vive en el binario default. Útil para integrar
    /// libs Python tipadas con stubs (e.g. `requests.pyi`).
    PyStubs {
        /// Archivo `.pyi` a parsear
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
    /// Fase 9.z.1 (a + b CERRADAS) — Formatea código Fitz a su
    /// estilo canónico (cero config). 4 espacios indent, comillas
    /// dobles, trailing comma solo multi-línea. **Preserva
    /// comentarios y blank lines del usuario** (9.z.1.b).
    ///
    /// Sin argumentos, formatea todos los `.fitz` del proyecto
    /// actual (vía manifest). Con archivos explícitos, formatea
    /// solo esos. `--check` no escribe, exit 1 si hay diffs.
    Fmt {
        /// Archivos `.fitz` a formatear. Si se omiten, formatea todo
        /// el proyecto (requiere `fitz.toml`).
        files: Vec<PathBuf>,
        /// Modo CI: no escribe, exit 1 si hay diffs.
        #[arg(long)]
        check: bool,
    },
    /// Fase 9.z.2.b — Corre todas las fns marcadas con `@test` del
    /// proyecto. En manifest mode, descubre desde `[lib].entry` (o
    /// `[bin].main`) + `tests/*.fitz` top-level del directorio del
    /// manifest. En single-file mode (`fitz test archivo.fitz`),
    /// carga ese archivo y corre sus `@test`. Filtra por substring
    /// del nombre del test si se pasa `[filter]`. Exit code 0 si
    /// todos pasan, 1 si alguno falla.
    Test {
        /// Substring del nombre del test para filtrar. Sin filter,
        /// corre todos los descubiertos.
        filter: Option<String>,
        /// Archivo `.fitz` específico. Si se omite, busca
        /// `fitz.toml` (manifest mode) y descubre desde el proyecto.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Fase 9.z.3 — Modo desarrollo con hot reload. Corre tu programa
    /// y lo re-arranca automáticamente cuando un archivo `.fitz` (o
    /// `fitz.toml`) cambia. Sin args, busca `fitz.toml` y corre el
    /// `[bin].main`. Con `--file`, corre ese archivo (single-file mode).
    ///
    /// Estrategia: kill+respawn del proceso (incremental rebuild es
    /// deuda). Excluye `target/`, `.git/`, `node_modules/`, archivos
    /// ocultos. Debounce 100ms para colapsar saves múltiples del
    /// editor. Ctrl+C mata el child antes de salir.
    Dev {
        /// Archivo `.fitz` específico. Si se omite, busca
        /// `fitz.toml` (manifest mode) y corre `[bin].main`.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Fase 9.z.4 — REPL interactivo. Abre un prompt `fitz> ` donde
    /// podés ingresar expresiones y statements línea por línea. El
    /// env persiste entre líneas: `let x = 1` queda definida para
    /// las siguientes. Multi-line automático (`... `) cuando un
    /// `{` o `(` quedan abiertos. History persistente en
    /// `~/.fitz/history`. Comandos especiales: `:help`, `:quit`,
    /// `:type <expr>`, `:env`, `:reset`, `:load <archivo>`.
    /// Ctrl+D sale. Async funciona (`sleep(100).await` y similares).
    Repl,
    /// Fase 9.z.5 — Linter de patrones más allá de tipos. Detecta
    /// `unused_variable`, `unused_import`, `useless_match`,
    /// `string_concat`. Default: warnings (exit 0). `--deny <lint>`
    /// trata ese lint como error (exit 1). Supresión por
    /// `// @allow(<lint>)` en la línea anterior. Sin args, busca
    /// `fitz.toml` (manifest mode) y lintea todos los `.fitz`.
    Lint {
        /// Archivos `.fitz` a lintear. Si se omiten, lintea todo
        /// el proyecto (requiere `fitz.toml`).
        files: Vec<PathBuf>,
        /// Trata el lint nombrado como error (exit 1 si aparece).
        /// Se puede pasar múltiples veces: `--deny unused_variable
        /// --deny string_concat`.
        #[arg(long)]
        deny: Vec<String>,
    },

    /// Fase 10.6 — Migraciones automáticas del ORM. Compara el
    /// schema declarado en los `@table` types con el schema real de
    /// la DB, genera SQL DDL para sincronizar, y trackea qué
    /// migraciones corrieron.
    #[command(subcommand)]
    Db(DbCmd),
}

#[derive(Subcommand)]
enum DbCmd {
    /// Compara el schema actual de la DB con el declarado en los
    /// `@table` types del programa Fitz y emite SQL DDL para
    /// sincronizar (CREATE TABLE / ADD COLUMN / etc.).
    Diff {
        /// Programa Fitz con los `@table` types. Si se omite,
        /// usa el `[bin].main` del `fitz.toml` del cwd/ancestros.
        file: Option<PathBuf>,
        /// URL Postgres. Si se omite, lee `DATABASE_URL` del env.
        #[arg(long)]
        url: Option<String>,
        /// Archivo de salida `.sql`. Si se omite, escribe a stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Corre todas las migraciones pendientes en `migrations/`
    /// (o el dir custom de `--dir`). Idempotente — skipea las
    /// ya aplicadas según `_fitz_migrations`.
    Migrate {
        /// URL Postgres. Si se omite, lee `DATABASE_URL` del env.
        #[arg(long)]
        url: Option<String>,
        /// Directorio con archivos `.sql`. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Solo imprime qué se aplicaría, sin tocar la DB.
        #[arg(long)]
        dry_run: bool,
    },
    /// Lista las migraciones del dir + estado (applied/pending).
    Status {
        /// URL Postgres. Si se omite, lee `DATABASE_URL` del env.
        #[arg(long)]
        url: Option<String>,
        /// Directorio con archivos `.sql`. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Crea un archivo de migración vacío con prefijo timestamp
    /// `YYYYMMDDHHMMSS_<name>.sql` en `migrations/`. v0.10.17 —
    /// el stub incluye secciones `-- UP` / `-- DOWN` por convención.
    New {
        /// Nombre descriptivo de la migración (snake_case).
        name: String,
        /// Directorio destino. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// v0.10.17 — Revierte las últimas N migraciones aplicadas
    /// ejecutando su sección `-- DOWN` adentro de tx + borrando
    /// el registro de `_fitz_migrations`. Default `N=1`. Aborta
    /// fast si alguna migration target no tiene `-- DOWN`.
    Rollback {
        /// URL Postgres. Si se omite, lee `DATABASE_URL` del env.
        #[arg(long)]
        url: Option<String>,
        /// Directorio con archivos `.sql`. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Cantidad de migraciones a revertir (más reciente
        /// primero). Default: 1.
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    /// v0.10.18 — Drift check: corre el diff y devuelve exit 0
    /// si el schema declarado matchea la DB, exit 1 con el SQL
    /// pendiente al stderr si hay diferencias. Hook para CI
    /// bloqueante ("no merge si el schema diverge").
    Check {
        /// Programa Fitz con los `@table` types. Si se omite,
        /// usa el `[bin].main` del `fitz.toml`.
        file: Option<PathBuf>,
        /// URL Postgres. Si se omite, lee `DATABASE_URL` del env.
        #[arg(long)]
        url: Option<String>,
    },
    /// v0.10.18 — Marca una migration como aplicada SIN ejecutar
    /// su SQL. Útil para adoptar Fitz en una DB legacy donde el
    /// schema ya está aplicado manualmente. `--all` marca todas
    /// las pending del dir como aplicadas.
    Stamp {
        /// Version a marcar (timestamp prefix del filename, p.ej.
        /// `20260530120000`). Mutuamente excluyente con `--all`.
        #[arg(conflicts_with = "all")]
        version: Option<String>,
        /// Marca todas las pending del dir como aplicadas.
        /// Mutuamente excluyente con `<version>`.
        #[arg(long)]
        all: bool,
        /// URL Postgres. Si se omite, lee `DATABASE_URL` del env.
        #[arg(long)]
        url: Option<String>,
        /// Directorio con archivos `.sql`. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
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
        Commands::Build {
            file,
            bundle_python,
            bundle_pip,
            bundle_pip_requirements,
        } => {
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
            // Fase 8.c: --bundle-pip implica --bundle-python.
            // Fase 8.c (cosecha): --bundle-pip-requirements también.
            // Cualquiera de los tres rutea al pipeline de bundling.
            if bundle_python || !bundle_pip.is_empty() || !bundle_pip_requirements.is_empty() {
                build_file_with_bundle(
                    &resolved.entry,
                    override_dest.as_deref(),
                    dep_registry,
                    bundle_pip,
                    bundle_pip_requirements,
                );
            } else {
                build_file(&resolved.entry, override_dest.as_deref(), dep_registry);
            }
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
        Commands::PyStubs { source, out } => {
            py_stubs_file(&source, out.as_deref());
        }
        Commands::New { name, http, no_git } => {
            new_project(&name, http, no_git);
        }
        Commands::Init { name, http, no_git } => {
            init_project(name.as_deref(), http, no_git);
        }
        Commands::Add {
            name,
            path,
            git,
            tag,
            rev,
        } => {
            add_dep_cmd(
                &name,
                path.as_deref(),
                git.as_deref(),
                tag.as_deref(),
                rev.as_deref(),
            );
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
        Commands::Test { filter, file } => {
            test_cmd(filter, file);
        }
        Commands::Dev { file } => {
            dev_cmd(file);
        }
        Commands::Repl => {
            repl_cmd();
        }
        Commands::Lint { files, deny } => {
            lint_cmd(files, deny);
        }
        Commands::Db(sub) => match sub {
            DbCmd::Diff { file, url, out } => db_diff_cmd(file, url, out),
            DbCmd::Migrate { url, dir, dry_run } => db_migrate_cmd(url, dir, dry_run),
            DbCmd::Status { url, dir } => db_status_cmd(url, dir),
            DbCmd::New { name, dir } => db_new_cmd(name, dir),
            DbCmd::Rollback { url, dir, count } => db_rollback_cmd(url, dir, count),
            DbCmd::Check { file, url } => db_check_cmd(file, url),
            DbCmd::Stamp {
                version,
                all,
                url,
                dir,
            } => db_stamp_cmd(version, all, url, dir),
        },
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
        eprintln!("✗ no se pudo leer `{}`: {e}", manifest_path.display());
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
    let resolved_deps =
        manifest::resolve_dependencies(&manifest, &manifest_dir).unwrap_or_else(|e| {
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
                eprintln!(
                    "✗ py-types: no se pudo escribir `{}`: {}",
                    path.display(),
                    e
                );
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

/// `fitz py-stubs <archivo.pyi> [--out <archivo.fitz>]` — pyi-stubs
/// (v0.9.39). Parsea un archivo `.pyi` (PEP 484/561) y emite los
/// `type` Fitz equivalentes para cada `class` top-level del stub.
/// Disponible sin feature `python` (el parser es ad-hoc, no usa PyO3).
///
/// Output: archivo Fitz commiteable que el user puede importar normal
/// (`from <archivo> import User`). El checker entonces ve tipos
/// reales, no `PyAny` opaco. Trade-off: pierde sincronía automática
/// con el .pyi (regenerar si el stub cambia).
fn py_stubs_file(source: &std::path::Path, out: Option<&std::path::Path>) {
    let raw = match fs::read_to_string(source) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ py-stubs: no se pudo leer `{}`: {}", source.display(), e);
            std::process::exit(1);
        }
    };
    let items = match fitz::pyi_stub::parse_stub(&raw) {
        Ok(items) => items,
        Err(e) => {
            eprintln!("✗ py-stubs: {}", e);
            std::process::exit(1);
        }
    };
    let output = render_stub_items_as_fitz(&items, source);
    match out {
        Some(path) => match fs::write(path, &output) {
            Ok(_) => println!("✓ types Fitz emitidos a {}", path.display()),
            Err(e) => {
                eprintln!(
                    "✗ py-stubs: no se pudo escribir `{}`: {}",
                    path.display(),
                    e
                );
                std::process::exit(1);
            }
        },
        None => print!("{}", output),
    }
}

/// Convierte los items del stub a código Fitz commiteable. Por ahora
/// emite solo `class` → `type` (fns/vars top-level del stub quedan
/// como deuda menor — el `.py` real expone esas vías field access
/// con `PyAny` opaco hoy y eso ya funciona).
fn render_stub_items_as_fitz(
    items: &[fitz::pyi_stub::StubItem],
    source: &std::path::Path,
) -> String {
    use fitz::pyi_stub::StubItem;
    let mut out = String::new();
    out.push_str(&format!(
        "// Generado por `fitz py-stubs` desde `{}`.\n",
        source.display()
    ));
    out.push_str("// No editar a mano — regenerar si el stub cambia.\n\n");
    for item in items {
        if let StubItem::Class(cls) = item {
            if cls.fields.is_empty() {
                // class sin fields → no aporta info útil al checker.
                continue;
            }
            out.push_str(&format!("type {} {{\n", cls.name));
            for (i, f) in cls.fields.iter().enumerate() {
                let fitz_ty = render_stub_type_as_fitz(&f.ty);
                let comma = if i + 1 < cls.fields.len() { "," } else { "" };
                out.push_str(&format!("    {}: {}{}\n", f.name, fitz_ty, comma));
            }
            out.push_str("}\n\n");
        }
    }
    out
}

fn render_stub_type_as_fitz(ty: &fitz::pyi_stub::StubType) -> String {
    use fitz::pyi_stub::StubType;
    match ty {
        StubType::Named(name) => match name.as_str() {
            "int" => "Int".into(),
            "float" => "Float".into(),
            "str" => "Str".into(),
            "bool" => "Bool".into(),
            "None" | "NoneType" => "Null".into(),
            "bytes" | "bytearray" => "Bytes".into(),
            "Any" | "object" => "Any".into(),
            // Custom name — el user debería tener el tipo declarado
            // adyacente o lo importará separado.
            other => other.to_string(),
        },
        StubType::Generic(name, args) => match (name.as_str(), args.len()) {
            ("list" | "List", 1) => {
                format!("List<{}>", render_stub_type_as_fitz(&args[0]))
            }
            ("dict" | "Dict", 2) => format!(
                "Map<{}, {}>",
                render_stub_type_as_fitz(&args[0]),
                render_stub_type_as_fitz(&args[1])
            ),
            ("Optional", 1) => format!("{}?", render_stub_type_as_fitz(&args[0])),
            _ => "Any".into(),
        },
        StubType::Union(alts) => {
            // `T | None` → `T?`. Resto → `Any`.
            let mut non_null = Vec::new();
            let mut has_null = false;
            for alt in alts {
                match alt {
                    StubType::Named(n) if n == "None" || n == "NoneType" => has_null = true,
                    _ => non_null.push(alt),
                }
            }
            if non_null.len() == 1 && has_null {
                format!("{}?", render_stub_type_as_fitz(non_null[0]))
            } else if non_null.len() == 1 {
                render_stub_type_as_fitz(non_null[0])
            } else {
                "Any".into()
            }
        }
        StubType::Any => "Any".into(),
    }
}

/// 8-pyi.B (v0.9.57) — Computa el `base_dir` para el lookup de stubs
/// `.pyi` adyacentes. Es el directorio padre del `path` del `.fitz`;
/// si el path no tiene padre claro (e.g. archivo en cwd sin prefix),
/// fallback a `current_dir()`.
fn base_dir_for_stub_lookup(path: &std::path::Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// 8-pyi.B (v0.9.57) — Wrapper de `check_program` que ANTES de invocar
/// al checker carga stubs `.pyi` adyacentes al `path` y registra sus
/// nominales en el TypeEnv. Política silent fallback: si el `.pyi` no
/// existe o falla al parsear, sigue como si no estuviera (el binding
/// del `from python import` queda como `Type::PyAny` opaco).
///
/// **Por qué main.rs y no types.rs**: el checker no conoce el
/// filesystem; el `path` solo lo tiene el caller que arrancó el
/// pipeline. Mantenemos `types::check_program(program)` puro para los
/// call sites sin contexto de archivo (tests, REPL, openapi de
/// programas sin path conocido).
fn check_program_with_pyi_stubs(
    program: &ast::Program,
    path: &std::path::Path,
) -> (
    types::TypeEnv,
    types::TypeInfo,
    types::DefinitionInfo,
    Vec<error::FitzError>,
) {
    check_program_with_pyi_stubs_and_deps(program, path, &manifest::DepRegistry::new())
}

/// W12 (v0.10.8) — variante de `check_program_with_pyi_stubs` que
/// además recibe el `dep_registry` resuelto del `fitz.toml` (cuando
/// `fitz check`/`build`/`run` corre adentro de un proyecto Fitz con
/// `[dependencies]`). Lo usa el pre-scan de `@auth_provider`
/// cross-module para resolver `from <dep-name> import` al `lib_entry`
/// absoluto de la dep en lugar de fallback a path relativo al
/// importer.
///
/// **Pasos coordinados** (orden importa):
/// 1. `pyi_loader::load_stubs` — pre-llena el TypeEnv con nominales
///    declarados en `.pyi` adyacentes (los call sites posteriores
///    pueden referenciar `User` del stub).
/// 2. `resolve_program_with_env` — registra nominales locales del
///    `.fitz` + nominales traídos por `from <mod> import <T>` (vuelta
///    1b) en el TypeEnv del importer.
/// 3. `pyi_loader::load_callables` — pre-llena callables de stubs
///    (depende de tipos resueltos en paso 2 para los rets).
/// 4. **`pre_scan_imported_auth_provider`** — escanea cada
///    `Stmt::Import` / `Stmt::FromImport`, resuelve el archivo .fitz,
///    parsea, extrae `@auth_provider` con
///    `types::extract_auth_provider_signature`. El primero que
///    aparezca gana (el caller, no el orden de imports). Si hay
///    provider, se registra en el TypeEnv con
///    `set_imported_auth_provider`. Errores de lectura/parse del
///    módulo se silencian — el loader real del runtime/codegen
///    reportará los suyos cuando corra de verdad.
/// 5. `check_with_env` — el checker corre con el env enriquecido;
///    `collect_auth_provider` hace fallback al provider importado
///    cuando no encuentra uno local.
fn check_program_with_pyi_stubs_and_deps(
    program: &ast::Program,
    path: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> (
    types::TypeEnv,
    types::TypeInfo,
    types::DefinitionInfo,
    Vec<error::FitzError>,
) {
    let base_dir = base_dir_for_stub_lookup(path);
    let mut env = types::TypeEnv::new();
    let stubs = pyi_loader::load_stubs(program, &base_dir, &mut env);
    let (mut env, errors) = types::resolve_program_with_env(program, env, Vec::new());
    pyi_loader::load_callables(&stubs, &mut env);
    // W12 — pre-scan de `@auth_provider` cross-module.
    if let Some(provider) = pre_scan_imported_auth_provider(program, &base_dir, dep_registry) {
        env.set_imported_auth_provider(provider);
    }
    types::check_with_env(program, env, errors)
}

/// W12 (v0.10.8) — Escanea los `Stmt::Import` / `Stmt::FromImport` del
/// importer. Por cada módulo importado, resuelve su archivo `.fitz`,
/// lo lee + lexea + parsea, e invoca
/// `types::extract_auth_provider_signature` sobre el AST. Devuelve el
/// primer provider encontrado (siguiendo el orden top-down de los
/// imports en el archivo).
///
/// **Política de errores**: errores de lectura/parse del módulo se
/// silencian (silent fallback — paralelo a la política del
/// `pyi_loader::load_stubs`). El loader real del runtime
/// (`evaluator::eval_with_base_and_deps`) y del codegen
/// (`ModuleLoader` en `codegen.rs`) cargan los módulos por su cuenta
/// y reportan errores claros si fallan. Acá NO queremos doble-reporte
/// — el objetivo es enriquecer el TypeEnv con info estática, no
/// validar imports.
///
/// **Alcance MVP**: un solo nivel de profundidad. El importer ve
/// `@auth_provider` de sus imports directos; no recursa transitivos.
/// Caso típico cubierto: `main.fitz` importa `auth.fitz`+`posts.fitz`
/// (provider en `auth.fitz`) — funciona. Caso fuera del MVP:
/// `posts.fitz` importa `lib.fitz` que a su vez importa el provider
/// desde `auth.fitz` — requeriría recursión. Si aparece presión,
/// extender en sub-paso futuro.
fn pre_scan_imported_auth_provider(
    program: &ast::Program,
    base_dir: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Option<types::ImportedAuthProvider> {
    for stmt in program {
        let (path_segments, module_binding_name) = match stmt {
            ast::Stmt::Import { path, alias, .. } => {
                // Skip Python imports — no tienen archivo .fitz en disk.
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                let binding = alias
                    .clone()
                    .or_else(|| path.last().cloned())
                    .unwrap_or_default();
                (path.clone(), binding)
            }
            ast::Stmt::FromImport { path, .. } => {
                // Skip Python imports.
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                // El "module_binding_name" para `from auth import User` es
                // `auth` (el último segmento del path). Codegen lo usa
                // como nombre de mod en el crate generado.
                let binding = path.last().cloned().unwrap_or_default();
                (path.clone(), binding)
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path(&path_segments, base_dir, dep_registry)
        else {
            continue;
        };
        let Ok(source) = fs::read_to_string(&file_path) else {
            continue;
        };
        let Ok(tokens) = lexer::tokenize(&source) else {
            continue;
        };
        let Ok(module_program) = parser::parse(tokens) else {
            continue;
        };
        if let Some(provider) =
            types::extract_auth_provider_signature(&module_program, &module_binding_name)
        {
            return Some(provider);
        }
    }
    None
}

/// W12 (v0.10.8) — resuelve `path` de un `Stmt::Import` /
/// `Stmt::FromImport` al archivo `.fitz` correspondiente. Mirror
/// liviano de `evaluator::resolve_module_path`:
/// 1. Single-segment matchea key del `dep_registry` → `lib_entry` de
///    la dep.
/// 2. Fallback a path relativo a `base_dir`: `["foo"]` →
///    `<base>/foo.fitz`; `["sub", "foo"]` → `<base>/sub/foo.fitz`.
///
/// Devuelve `None` si el archivo no existe (verificamos con `exists()`
/// para que el silent fallback funcione sin tirar warnings espurios).
fn resolve_import_file_path(
    segments: &[String],
    base_dir: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Option<PathBuf> {
    if segments.len() == 1 {
        if let Some(lib_entry) = dep_registry.get(&segments[0]) {
            if lib_entry.exists() {
                return Some(lib_entry.clone());
            }
        }
    }
    let mut candidate = base_dir.to_path_buf();
    for seg in &segments[..segments.len().saturating_sub(1)] {
        candidate.push(seg);
    }
    if let Some(last) = segments.last() {
        candidate.push(format!("{}.fitz", last));
    }
    if candidate.exists() {
        Some(candidate)
    } else {
        None
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
    // criterio que `fitz build`. 8-pyi.B: carga stubs .pyi adyacentes.
    let (_env, _types, _defs, type_errors) = check_program_with_pyi_stubs(&program, path);
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

    let base_dir = base_dir_for_stub_lookup(path);

    let (eval_result, registry) =
        http::with_active_registry(|| evaluator::eval_with_base_sync(program.clone(), base_dir));
    if let Err(e) = eval_result {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    let routes = openapi::routes_from_registry(&registry, &program);
    // Q.2: `@server(api_version=...)` override.
    let api_version = registry
        .server_config
        .as_ref()
        .and_then(|c| c.api_version.clone());
    let schema = openapi::generate_openapi_with_version(&routes, &program, api_version.as_deref());
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
    // 8-pyi.B: carga stubs .pyi adyacentes antes del check.
    let (_env, _types, _defs, errors) = check_program_with_pyi_stubs(&program, path);
    if errors.is_empty() {
        println!("✓ {} — sin errores de tipo", path.display());
    } else {
        eprintln!("✗ {} — {} error(es) de tipo:", path.display(), errors.len());
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
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Checker en modo strict — no hay `--no-typecheck` en build.
    // 8-pyi.B: carga stubs .pyi adyacentes antes del check.
    let (env, types, _defs, type_errors) = check_program_with_pyi_stubs(&program, path);
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

    // Mini-tanda P2 — 5b.1/Hpx.2 chained fix. Si hay fns con params
    // sin anotar (5b.1) Y return type sin anotar (Hpx.2), la primera
    // pasada del checker tipa el body asumiendo params como Any, y
    // Hpx.2 falla porque el body retorna Any. Estrategia: inferir
    // params via call sites (codegen::infer_param_type_from_call_sites)
    // y mutar el AST en-place fillingo Param.type_, después re-correr
    // el checker para refinar TypeInfo. Cost extra: ~1 check pass para
    // programas con unannotated fns; gratis para programas anotados.
    let (env, types) = if codegen::has_unannotated_fn_params(&program) {
        codegen::fill_inferred_param_types(&mut program, &types, &env);
        // 8-pyi.B: re-check también carga stubs (idempotente).
        let (env2, types2, _defs2, errs2) = check_program_with_pyi_stubs(&program, path);
        if !errs2.is_empty() {
            // Si el re-check genera nuevos errores con los tipos
            // inferidos, surfacearlos.
            eprintln!(
                "✗ {} — {} error(es) de tipo tras inferencia de params (5b.1):",
                path.display(),
                errs2.len()
            );
            for e in &errs2 {
                eprintln!("  {}", e);
            }
            std::process::exit(1);
        }
        (env2, types2)
    } else {
        (env, types)
    };

    // Codegen a Cargo project. Mini-tanda Hpx.2 — TypeInfo del checker
    // se pasa al codegen para inferir return types de fns sin anotar.
    let project = match codegen::generate_project(path, &program, &env, &types, dep_registry) {
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
        eprintln!(
            "   (revisá {} para ver qué se intentó compilar.)",
            src_dir.display()
        );
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

/// Fase 8.b — Variante de `build_file` que produce un binario standalone
/// Hash determinístico de los inputs del pip install (positionals
/// `--bundle-pip` + bytes de los `--bundle-pip-requirements`).
///
/// Usado como cache key del pip_packages tarball para evitar re-correr
/// pip install en cada `fitz build` cuando los paquetes no cambiaron.
///
/// El sidecar `<bin>_pip_packages.inputs_hash` adyacente al tarball
/// guarda el resultado; en el próximo build, si matchea el hash actual,
/// se reusa el tarball existente.
///
/// Reglas del hash:
/// - Positionals de `--bundle-pip` se ordenan alfabéticamente
///   (reordenar args no debe invalidar cache).
/// - Bytes de cada requirements file en orden CLI (reordenar archivos
///   sí invalida; pip los procesa en orden y puede dar resultados
///   diferentes con el mismo set de paquetes).
/// - Separador `\n---\n` entre las dos secciones para que
///   `["foo", "bar"]` positionals ≠ `"foo\nbar"` en requirements.
fn pip_inputs_hash(bundle_pip: &[String], requirements_contents: &[Vec<u8>]) -> String {
    let mut sorted_pkgs: Vec<&str> = bundle_pip.iter().map(|s| s.as_str()).collect();
    sorted_pkgs.sort();
    let mut buf: Vec<u8> = Vec::new();
    for p in &sorted_pkgs {
        buf.extend_from_slice(p.as_bytes());
        buf.push(b'\n');
    }
    buf.extend_from_slice(b"---\n");
    for content in requirements_contents {
        buf.extend_from_slice(content);
        buf.push(b'\n');
    }
    launcher_template::tarball_hash_short(&buf)
}

/// con CPython embebido. El output es UN solo archivo que internamente
/// lleva: tarball PBS (install_only_stripped 3.14.x) + real binary +
/// launcher Rust standalone. Primer run extrae a `$TMPDIR/fitz-py-<hash>/`,
/// setea PYTHONHOME + LD_LIBRARY_PATH/DYLD/PATH según OS, exec del real
/// binary. Subsecuentes runs son instantáneos (cache TMP).
///
/// Validaciones tempranas: host triple soportado por PBS, programa usa
/// `from python import` (sin interop no tiene sentido bundlear).
///
/// Constraint conocido del Linux (deuda R.bug-pyo3-abi3-portable-link):
/// el real binary linkea contra `libpython3.X.so.1.0` específica del
/// builder, no contra `libpython3.so` stable ABI. PBS 3.14.5 exige que
/// el builder tenga Python 3.14.x disponible al momento de `cargo build`
/// para que la versión del symlink linkeado coincida con la libpython
/// del bundle. En Windows este problema no existe (linkea contra
/// `python3.dll` stable ABI shim).
fn build_file_with_bundle(
    path: &PathBuf,
    override_dest: Option<&std::path::Path>,
    dep_registry: manifest::DepRegistry,
    bundle_pip: Vec<String>,
    bundle_pip_requirements: Vec<PathBuf>,
) {
    // --- Validación temprana: host triple soportado ---
    let triple = match pbs::host_triple() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("✗ {}", e);
            std::process::exit(1);
        }
    };

    // --- Validación temprana: requirements files existen y son legibles ---
    //
    // Lo hacemos ANTES de tocar lex/parse/PBS para fail-fast sobre el
    // input del usuario (caso típico: typo en el path del archivo).
    let mut requirements_abs_paths: Vec<PathBuf> = Vec::new();
    let mut requirements_pkg_count: usize = 0;
    for req_path in &bundle_pip_requirements {
        let abs = match req_path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "✗ no se pudo leer requirements file `{}`: {}",
                    req_path.display(),
                    e
                );
                std::process::exit(1);
            }
        };
        let content = match fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "✗ no se pudo leer requirements file `{}`: {}",
                    abs.display(),
                    e
                );
                std::process::exit(1);
            }
        };
        // Conteo aproximado solo para el summary: líneas no-blank que no
        // empiezan con `#`. pip se encarga del parsing real (includes
        // `-r other.txt`, opciones como `--hash`, etc.).
        requirements_pkg_count += content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count();
        requirements_abs_paths.push(abs);
    }

    // --- Lex + parse para detectar from python import ---
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
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    if !program_uses_from_python_import(&program) {
        eprintln!("✗ `--bundle-python` solo aplica a programas que usan `from python import ...`.");
        eprintln!(
            "  Si tu programa no usa interop Python, usá `fitz build` sin el flag — el binario \
             resultante ya es standalone (no requiere ningún runtime externo)."
        );
        std::process::exit(1);
    }

    // --- Checker strict (sin `--no-typecheck` en build) ---
    // 8-pyi.B: carga stubs .pyi adyacentes antes del check.
    let (env, types, _defs, type_errors) = check_program_with_pyi_stubs(&program, path);
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

    let (env, types) = if codegen::has_unannotated_fn_params(&program) {
        codegen::fill_inferred_param_types(&mut program, &types, &env);
        // 8-pyi.B: re-check también carga stubs (idempotente).
        let (env2, types2, _defs2, errs2) = check_program_with_pyi_stubs(&program, path);
        if !errs2.is_empty() {
            eprintln!(
                "✗ {} — {} error(es) de tipo tras inferencia de params:",
                path.display(),
                errs2.len()
            );
            for e in &errs2 {
                eprintln!("  {}", e);
            }
            std::process::exit(1);
        }
        (env2, types2)
    } else {
        (env, types)
    };

    // --- Codegen + escribir Cargo project del real binary ---
    let project = match codegen::generate_project(path, &program, &env, &types, dep_registry) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ codegen: {}", e);
            std::process::exit(1);
        }
    };

    let build_dir = PathBuf::from("target")
        .join("fitz-build")
        .join(&project.bin_name);
    let src_dir = build_dir.join("src");
    if let Err(e) = fs::create_dir_all(&src_dir) {
        eprintln!("Error creando {}: {}", src_dir.display(), e);
        std::process::exit(1);
    }

    let cargo_toml_path = build_dir.join("Cargo.toml");
    if let Err(e) = fs::write(&cargo_toml_path, &project.cargo_toml) {
        eprintln!("Error escribiendo {}: {}", cargo_toml_path.display(), e);
        std::process::exit(1);
    }
    let main_rs_path = src_dir.join("main.rs");
    if let Err(e) = fs::write(&main_rs_path, &project.main_rs) {
        eprintln!("Error escribiendo {}: {}", main_rs_path.display(), e);
        std::process::exit(1);
    }
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

    // --- Build del real binary ---
    println!("→ compilando real binary…");
    let output = std::process::Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&cargo_toml_path)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error invocando cargo (real binary): {}", e);
            std::process::exit(1);
        }
    };
    if !output.status.success() {
        eprintln!("✗ cargo build del real binary falló:");
        eprintln!("--- stderr ---");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }

    let real_bin_filename = if cfg!(windows) {
        format!("{}.exe", project.bin_name)
    } else {
        project.bin_name.clone()
    };
    let real_bin_path = build_dir
        .join("target")
        .join("release")
        .join(&real_bin_filename);
    let real_bin_abs = match real_bin_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "✗ no se encontró el real binary tras cargo build ({}): {}",
                real_bin_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    // --- PBS tarball + hash ---
    println!(
        "→ asegurando PBS tarball (cpython {} / {})…",
        pbs::PYTHON_VERSION,
        triple
    );
    let tarball_path = match pbs::ensure_tarball(triple) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ {}", e);
            std::process::exit(1);
        }
    };
    let tarball_abs = match tarball_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ canonicalize del tarball PBS falló: {}", e);
            std::process::exit(1);
        }
    };
    let tarball_bytes = match fs::read(&tarball_abs) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("✗ leer tarball PBS para hash: {}", e);
            std::process::exit(1);
        }
    };

    // --- Fase 8.c — Pip install + tarball secundario (si --bundle-pip) ---
    //
    // Si el usuario pasó `--bundle-pip <pkg>` (uno o más veces), instalamos
    // esos paquetes con pip al build time adentro de un dir local del
    // proyecto y los empacamos en un tarball secundario que el launcher
    // va a embeber como segundo `include_bytes!`. Al primer run del
    // binario, el launcher extrae los paquetes adentro de
    // `python/Lib/site-packages/` (Windows) o `python/lib/python3.X/
    // site-packages/` (Unix) del extract dir TMP, automáticamente
    // accesibles via `import` desde el código Python del usuario.
    //
    // Para correr `pip install` necesitamos un Python ejecutable; usamos
    // el python embebido del PBS tarball, extraído al cache local del
    // proyecto. El extract es ~60 MB pero se reusa entre builds.
    let pip_total_count = bundle_pip.len() + requirements_pkg_count;
    let pip_tarball_abs: Option<PathBuf> = if !bundle_pip.is_empty()
        || !requirements_abs_paths.is_empty()
    {
        // --- Cache key del pip_packages tarball ---
        //
        // Helper `pip_inputs_hash` calcula el hash determinístico sobre
        // los inputs del pip install (positionals --bundle-pip ordenados
        // + bytes de los requirements files). Sidecar vive adentro de
        // `target/fitz-build/<bin>_*` → scoped por proyecto.
        let mut requirements_contents: Vec<Vec<u8>> =
            Vec::with_capacity(requirements_abs_paths.len());
        for req_path in &requirements_abs_paths {
            let content = match fs::read(req_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "✗ no se pudo releer requirements file `{}` para cache key: {}",
                        req_path.display(),
                        e
                    );
                    std::process::exit(1);
                }
            };
            requirements_contents.push(content);
        }
        let pip_inputs_hash = pip_inputs_hash(&bundle_pip, &requirements_contents);

        let pip_tarball_path = PathBuf::from("target")
            .join("fitz-build")
            .join(format!("{}_pip_packages.tar.gz", project.bin_name));
        let pip_hash_sidecar = PathBuf::from("target")
            .join("fitz-build")
            .join(format!("{}_pip_packages.inputs_hash", project.bin_name));

        // Cache hit: tarball + sidecar existen y el hash matchea.
        // Saltamos extracción del PBS, pip install y tar.
        let cache_hit = pip_tarball_path.exists()
            && pip_hash_sidecar
                .exists()
                .then(|| fs::read_to_string(&pip_hash_sidecar).ok())
                .flatten()
                .map(|s| s.trim() == pip_inputs_hash)
                .unwrap_or(false);

        if cache_hit {
            println!(
                "→ pip cache hit ({} paquete(s), hash {}…) — reusando tarball",
                pip_total_count,
                &pip_inputs_hash[..8]
            );
            match pip_tarball_path.canonicalize() {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("✗ canonicalize del pip tarball cacheado falló: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            println!(
                "→ extrayendo PBS al cache local para correr pip ({} paquete(s))…",
                pip_total_count
            );
            let pbs_extract_dir = PathBuf::from("target")
                .join("fitz-build")
                .join(format!("{}_pbs_extract", project.bin_name));
            if !pbs_extract_dir.join("python").exists() {
                if let Err(e) = fs::create_dir_all(&pbs_extract_dir) {
                    eprintln!("Error creando {}: {}", pbs_extract_dir.display(), e);
                    std::process::exit(1);
                }
                // Extraer PBS tarball usando `tar -xzf` (mismo subprocess que
                // usa el launcher en runtime).
                let tar_status = std::process::Command::new("tar")
                    .args([
                        "-xzf",
                        &tarball_abs.to_string_lossy(),
                        "-C",
                        &pbs_extract_dir.to_string_lossy(),
                    ])
                    .status();
                match tar_status {
                    Ok(s) if s.success() => {}
                    Ok(s) => {
                        eprintln!(
                            "✗ tar -xzf del PBS tarball falló (exit code: {:?})",
                            s.code()
                        );
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("✗ no se pudo invocar `tar` para extraer PBS: {}", e);
                        eprintln!(
                            "  Necesitás `tar` en el PATH (bsdtar en Win11/macOS, GNU tar en Linux)."
                        );
                        std::process::exit(1);
                    }
                }
            }
            // Path del ejecutable Python adentro del PBS extract.
            let python_exe = if cfg!(windows) {
                pbs_extract_dir.join("python").join("python.exe")
            } else {
                pbs_extract_dir.join("python").join("bin").join("python3")
            };
            if !python_exe.exists() {
                eprintln!(
                    "✗ no se encontró python en el PBS extract: {}",
                    python_exe.display()
                );
                std::process::exit(1);
            }

            // Dir del pip install. Limpiar previo si existe para builds
            // reproducibles (pip install --target es additive, sin clean
            // los paquetes acumulan entre builds).
            let pip_install_dir = PathBuf::from("target")
                .join("fitz-build")
                .join(format!("{}_pip_packages", project.bin_name));
            if pip_install_dir.exists() {
                let _ = fs::remove_dir_all(&pip_install_dir);
            }
            if let Err(e) = fs::create_dir_all(&pip_install_dir) {
                eprintln!("Error creando {}: {}", pip_install_dir.display(), e);
                std::process::exit(1);
            }

            println!("→ pip install --target ({} paquete(s))…", pip_total_count);
            let mut pip_args = vec![
                "-m".to_string(),
                "pip".to_string(),
                "install".to_string(),
                "--target".to_string(),
                pip_install_dir.to_string_lossy().to_string(),
                "--no-warn-script-location".to_string(),
                // Quiet para evitar ruido en CI; los errores siguen
                // visibles en stderr.
                "--quiet".to_string(),
            ];
            // `-r <file>` por cada requirements file. pip los acumula con
            // los positionals que vienen después; toda la sintaxis nativa
            // del archivo (comments, includes, version pins, --hash) la
            // procesa pip directamente, sin parsing del lado de Fitz.
            for req_path in &requirements_abs_paths {
                pip_args.push("-r".to_string());
                pip_args.push(req_path.to_string_lossy().into_owned());
            }
            pip_args.extend(bundle_pip.iter().cloned());
            let pip_out = std::process::Command::new(&python_exe)
                .args(&pip_args)
                .output();
            let pip_out = match pip_out {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("✗ no se pudo invocar pip: {}", e);
                    std::process::exit(1);
                }
            };
            if !pip_out.status.success() {
                eprintln!("✗ pip install falló:");
                eprintln!("--- stderr ---");
                eprintln!("{}", String::from_utf8_lossy(&pip_out.stderr));
                std::process::exit(1);
            }

            // Crear tarball secundario desde el pip_install_dir. Usamos
            // `tar -czf` con `-C <dir>` para que las paths internas sean
            // relativas (sin el prefijo del cwd).
            println!("→ empacando pip_packages.tar.gz…");
            let tar_status = std::process::Command::new("tar")
                .args([
                    "-czf",
                    &pip_tarball_path.to_string_lossy(),
                    "-C",
                    &pip_install_dir.to_string_lossy(),
                    ".",
                ])
                .status();
            match tar_status {
                Ok(s) if s.success() => {}
                Ok(s) => {
                    eprintln!(
                        "✗ tar -czf del pip_packages tarball falló (exit code: {:?})",
                        s.code()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("✗ no se pudo invocar `tar` para empacar pip: {}", e);
                    std::process::exit(1);
                }
            }
            // Write sidecar con el hash de los inputs. El sidecar vive
            // adyacente al tarball para que `rm target/fitz-build/<bin>_*`
            // limpie todo junto. Si la escritura falla no abortamos el
            // build — el cache simplemente no funcionará la próxima vez.
            if let Err(e) = fs::write(&pip_hash_sidecar, &pip_inputs_hash) {
                eprintln!(
                    "Warning: no se pudo escribir cache sidecar `{}`: {}",
                    pip_hash_sidecar.display(),
                    e
                );
            }
            match pip_tarball_path.canonicalize() {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("✗ canonicalize del pip tarball falló: {}", e);
                    std::process::exit(1);
                }
            }
        }
    } else {
        None
    };

    // Hash combinado: si hay pip, incluir los bytes del pip tarball en
    // el hash para que dos proyectos con distintos paquetes tengan
    // distintos extract dirs en TMP (cache hit-or-miss correcto).
    let tarball_hash = if let Some(ref pip_path) = pip_tarball_abs {
        let pip_bytes = match fs::read(pip_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("✗ leer pip tarball para hash combinado: {}", e);
                std::process::exit(1);
            }
        };
        let mut combined = tarball_bytes.clone();
        combined.extend_from_slice(&pip_bytes);
        launcher_template::tarball_hash_short(&combined)
    } else {
        launcher_template::tarball_hash_short(&tarball_bytes)
    };

    // --- Generar + buildear launcher ---
    println!("→ compilando launcher…");
    let launcher_bin_name = format!("{}_launcher", project.bin_name);
    let launcher_dir = PathBuf::from("target")
        .join("fitz-build")
        .join(&launcher_bin_name);
    let launcher_src = launcher_dir.join("src");
    if let Err(e) = fs::create_dir_all(&launcher_src) {
        eprintln!("Error creando {}: {}", launcher_src.display(), e);
        std::process::exit(1);
    }

    let pip_tarball_str = pip_tarball_abs
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    let launcher_main_rs = launcher_template::gen_launcher_main_rs(
        &tarball_abs.to_string_lossy(),
        &real_bin_abs.to_string_lossy(),
        &tarball_hash,
        pip_tarball_str.as_deref(),
    );
    let launcher_cargo_toml = launcher_template::gen_launcher_cargo_toml(&launcher_bin_name);

    let launcher_cargo_toml_path = launcher_dir.join("Cargo.toml");
    if let Err(e) = fs::write(&launcher_cargo_toml_path, &launcher_cargo_toml) {
        eprintln!(
            "Error escribiendo {}: {}",
            launcher_cargo_toml_path.display(),
            e
        );
        std::process::exit(1);
    }
    let launcher_main_rs_path = launcher_src.join("main.rs");
    if let Err(e) = fs::write(&launcher_main_rs_path, &launcher_main_rs) {
        eprintln!(
            "Error escribiendo {}: {}",
            launcher_main_rs_path.display(),
            e
        );
        std::process::exit(1);
    }

    let output = std::process::Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&launcher_cargo_toml_path)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error invocando cargo (launcher): {}", e);
            std::process::exit(1);
        }
    };
    if !output.status.success() {
        eprintln!("✗ cargo build del launcher falló:");
        eprintln!(
            "   (revisá {} para ver el código generado.)",
            launcher_src.display()
        );
        eprintln!("--- stderr ---");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }

    // --- Copiar launcher al destino del usuario ---
    let launcher_release_filename = if cfg!(windows) {
        format!("{}.exe", launcher_bin_name)
    } else {
        launcher_bin_name.clone()
    };
    let output_filename = if cfg!(windows) {
        format!("{}.exe", project.output_basename)
    } else {
        project.output_basename.clone()
    };
    let launcher_release_path = launcher_dir
        .join("target")
        .join("release")
        .join(&launcher_release_filename);

    let bin_out = match override_dest {
        Some(p) => p.to_path_buf(),
        None => path
            .parent()
            .map(|p| p.join(&output_filename))
            .unwrap_or_else(|| PathBuf::from(&output_filename)),
    };
    if let Some(parent) = bin_out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("Error creando {}: {}", parent.display(), e);
                std::process::exit(1);
            }
        }
    }
    if let Err(e) = fs::copy(&launcher_release_path, &bin_out) {
        eprintln!(
            "Error copiando {} a {}: {}",
            launcher_release_path.display(),
            bin_out.display(),
            e
        );
        std::process::exit(1);
    }

    let bin_size_mb = fs::metadata(&bin_out)
        .map(|m| m.len() as f64 / 1024.0 / 1024.0)
        .unwrap_or(0.0);
    let bundle_summary = if pip_total_count == 0 {
        format!("CPython {} embebido", pbs::PYTHON_VERSION)
    } else {
        format!(
            "CPython {} + {} pip pkg(s) embebidos",
            pbs::PYTHON_VERSION,
            pip_total_count
        )
    };
    println!(
        "✓ binario standalone ({}): {} ({:.1} MB)",
        bundle_summary,
        bin_out.display(),
        bin_size_mb
    );
    println!(
        "  Primer arranque en el destino extrae a $TMPDIR/fitz-py-{}/ \
         (~3-5s sobre SSD, depende del OS). Runs subsecuentes ~50-100ms (cache TMP).",
        &tarball_hash[..8]
    );
}

/// Detecta `from python import X` en el AST. Devuelve true si al menos
/// un `Stmt::FromImport` tiene `path[0] == "python"`. Usado por
/// `build_file_with_bundle` para validar el uso de `--bundle-python`.
fn program_uses_from_python_import(program: &fitz::ast::Program) -> bool {
    use fitz::ast::Stmt;
    program.iter().any(|s| {
        matches!(
            s,
            Stmt::FromImport { path, .. }
                if path.first().map(|s| s.as_str()) == Some("python")
        )
    })
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
    // 8-pyi.B: carga stubs .pyi adyacentes antes del check.
    let (_type_env, _types, _defs, type_errors) = check_program_with_pyi_stubs(&program, path);
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
    } else if registry.cron_registry.has_jobs() {
        // Fase 9.w.3 — cron-only mode: el programa NO tiene rutas
        // HTTP pero SÍ tiene jobs `@cron`. Arrancamos el scheduler
        // standalone y bloqueamos hasta SIGINT/Ctrl+C (decisión
        // confirmada con el autor: vivo bloqueante, modo
        // systemd-friendly).
        let cron_registry = registry.cron_registry.clone();
        if let Err(e) = cron_jobs::run_scheduler_only(cron_registry) {
            eprintln!("Error del cron scheduler: {}", e);
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
        eprintln!(
            "✗ `{}` ya existe — borralo o elegí otro nombre.",
            target.display()
        );
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
        (Some(p), None) => manifest::AddDepSpec::Path {
            path: p.to_string(),
        },
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
        eprintln!(
            "✗ la dep `{name}` no estaba en `[dependencies]` de `{}`.",
            manifest_path.display()
        );
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
                    eprintln!(
                        "  (aviso: no se pudo borrar `{}`: {e})",
                        lock_path.display()
                    );
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
            eprintln!(
                "✗ la dep `{filter}` no está en `[dependencies]` de `{}`.",
                manifest_path.display()
            );
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

    // (Fase 9.z.1.b: el warning loud de 9.z.1.a se removió porque
    // ya preservamos comments + blank lines del usuario.)

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

// ---- Fase 9.z.2.b — `fitz test` (testing built-in) ----

/// Una fuente de tests para el runner. `path` es absoluto (la
/// invocación con `fitz test archivo.fitz` lo canonicaliza). `label`
/// es el nombre amigable que se usa para prefijar los nombres en el
/// output (`<label>::<test>`); `None` significa no prefijar — caso
/// típico de single-file mode.
struct TestSource {
    path: PathBuf,
    label: Option<String>,
}

/// Entry point del sub-comando `fitz test` (Fase 9.z.2.b).
///
/// - **Single-file mode** (`fitz test --file archivo.fitz [filter]`):
///   evalúa `archivo.fitz` con un `TestRegistry` activo, después
///   corre los tests descubiertos.
/// - **Manifest mode** (`fitz test [filter]`): busca `fitz.toml`,
///   evalúa el entry de la lib (o el bin si no hay lib) + cada
///   `tests/*.fitz` top-level del directorio del manifest. Cada
///   archivo se evalúa con su path como `source_label` para que el
///   output prefije los nombres.
///
/// El filtro es substring case-sensitive sobre el nombre del test
/// (sin prefijo de file). Cargo style.
fn test_cmd(filter: Option<String>, file_arg: Option<PathBuf>) {
    let (sources, dep_registry) = match file_arg {
        Some(p) => {
            // Single-file: el path tal cual; sin label en el output.
            // Dep registry vacío (single-file no toca `fitz.toml`).
            (
                vec![TestSource {
                    path: p,
                    label: None,
                }],
                manifest::DepRegistry::new(),
            )
        }
        None => discover_test_sources_from_manifest(),
    };

    if sources.is_empty() {
        eprintln!(
            "✗ no se encontraron archivos con tests.\n\
             En manifest mode, descubrimos `[lib].entry` (o `[bin].main`) + \
             `tests/*.fitz` top-level. En single-file, pasá `--file <archivo.fitz>`."
        );
        std::process::exit(1);
    }

    // Build runtime tokio current_thread + bloquear sobre toda la
    // operación: descubrimiento (evaluar cada archivo con registry
    // activo) + run de los tests. Una sola invocación del runtime
    // para todo, así los TestSpec acumulan en el mismo registry.
    let runtime = evaluator::build_runtime();
    let registry = runtime.block_on(async {
        let ((), reg) = testing::with_active_test_registry_async(|| async {
            for src in &sources {
                let res = match &src.label {
                    Some(label) => {
                        testing::with_test_source_async(label.clone(), || async {
                            eval_test_source(&src.path, &dep_registry).await
                        })
                        .await
                    }
                    None => eval_test_source(&src.path, &dep_registry).await,
                };
                if let Err(e) = res {
                    eprintln!("✗ error cargando {}: {}", src.path.display(), e);
                    std::process::exit(1);
                }
            }
        })
        .await;
        reg
    });

    let total_failed = run_test_registry(&registry, filter.as_deref());
    if total_failed > 0 {
        std::process::exit(1);
    }
}

/// Descubre las fuentes de tests en manifest mode. Lee el manifest
/// (debe existir), arma el `dep_registry` (resolución de deps
/// path/git), después devuelve:
///
/// 1. `[lib].entry` si existe; si no, `[bin].main` si existe; si no,
///    ninguna fuente del proyecto (solo `tests/*.fitz`).
/// 2. Todos los `tests/<nombre>.fitz` top-level del directorio del
///    manifest (no recursivo — alineado con cómo Cargo descubre
///    integration tests).
///
/// A diferencia de `resolve_entry`, NO exigimos `[bin]` — un
/// proyecto solo-lib es válido (caso 90% de las librerías). Si no
/// hay ni lib ni bin ni `tests/`, devolvemos lista vacía y el caller
/// (`test_cmd`) emite el mensaje "no se encontraron archivos con
/// tests".
///
/// Los `label` son paths relativos al `manifest_dir`
/// (`"src/lib.fitz"`, `"tests/math.fitz"`) para que el output sea
/// legible y portable entre máquinas.
fn discover_test_sources_from_manifest() -> (Vec<TestSource>, manifest::DepRegistry) {
    let manifest_path = find_local_manifest_or_exit();
    let manifest_text = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    });
    let parsed_manifest = manifest::Manifest::parse(&manifest_text).unwrap_or_else(|e| {
        eprintln!("✗ `{}`: {e}", manifest_path.display());
        std::process::exit(1);
    });
    let manifest_dir = manifest_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Resolver deps eager (fail-fast con mensaje del resolver). Sin
    // deps, el dep_registry queda vacío y el loader solo usará paths
    // relativos.
    let resolved_deps = manifest::resolve_dependencies(&parsed_manifest, &manifest_dir)
        .unwrap_or_else(|e| {
            eprintln!("✗ no se pudieron resolver las dependencias: {e}");
            std::process::exit(1);
        });

    // Sync lockfile (no-op si no hay deps o ya está sincronizado).
    if !resolved_deps.is_empty() {
        let lock = lockfile::Lockfile::from_resolved(&resolved_deps);
        let lock_path = lockfile::lockfile_path(&manifest_dir);
        if let Err(e) = lockfile::write_lockfile_if_changed(&lock_path, &lock) {
            eprintln!("✗ no se pudo escribir `{}`: {e}", lock_path.display());
            std::process::exit(1);
        }
    }
    let mut dep_registry = manifest::build_dep_registry(&resolved_deps);

    // Auto-self-import: si el proyecto declara `[lib].entry`,
    // registramos el lib bajo el nombre del paquete en el
    // `dep_registry`. Esto permite que `tests/*.fitz` haga
    // `from <pkg-name> import X` para acceder al código de la lib —
    // paralelo a `use my_crate::*` de Rust en tests integration.
    // Sin esto, los tests tendrían que escribir paths fragmentados
    // (`from ../src/lib import X`) que el loader actual no soporta.
    if let Some(lib) = &parsed_manifest.lib {
        let lib_path = manifest_dir.join(&lib.entry);
        if lib_path.exists() {
            dep_registry.insert(parsed_manifest.package.name.clone(), lib_path);
        }
    }

    // Primero coleccionamos los `tests/*.fitz` top-level del manifest dir
    // (no recursivo). Orden alfabético para reproducibilidad.
    let mut integration_sources: Vec<TestSource> = Vec::new();
    let tests_dir = manifest_dir.join("tests");
    if tests_dir.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&tests_dir)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for path in entries {
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("fitz") {
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("test.fitz")
                    .to_string();
                integration_sources.push(TestSource {
                    path,
                    label: Some(format!("tests/{}", file_name)),
                });
            }
        }
    }

    let mut sources: Vec<TestSource> = Vec::new();

    if !integration_sources.is_empty() {
        // **Modo "tests integration"**: SOLO cargamos los `tests/*.fitz`.
        // El `[lib]` (o `[bin]`) se carga indirectamente cuando un test
        // hace `from <pkg> import X` — el dep_registry tiene el auto-self
        // registrado, y el loader cachea por path canonical, así un
        // `@test` declarado en el lib se descubre UNA VEZ aunque varios
        // tests importen la lib. Si no lo importa nadie, no se descubre
        // (deuda visible para el caso degenerado).
        sources.extend(integration_sources);
    } else {
        // **Modo "tests inline only"**: cargamos el `[lib]` (o `[bin]`)
        // directamente porque es el único lugar donde puede haber `@test`.
        let entry_rel: Option<String> = match (&parsed_manifest.lib, &parsed_manifest.bin) {
            (Some(lib), _) => Some(lib.entry.clone()),
            (None, Some(bin)) => Some(bin.main.clone()),
            (None, None) => None,
        };
        if let Some(rel) = entry_rel {
            let path = manifest_dir.join(&rel);
            if path.exists() {
                sources.push(TestSource {
                    path,
                    label: Some(rel),
                });
            }
        }
    }

    (sources, dep_registry)
}

/// Evalúa un archivo con el `TestRegistry` (y posiblemente el
/// `CURRENT_TEST_SOURCE`) ya activos por el caller. Hace
/// lexer + parser + checker strict + eval. Si el checker reporta
/// errores, los formatea y devuelve `Err` (el caller decide
/// abortar).
///
/// `base_dir` se deriva del directorio del archivo — así los
/// imports relativos (`from utils import X`) resuelven al sibling
/// del archivo, paralelo a `fitz run` single-file.
async fn eval_test_source(
    path: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Result<(), String> {
    let source = fs::read_to_string(path).map_err(|e| format!("no se pudo leer: {e}"))?;

    let tokens = lexer::tokenize(&source).map_err(|e| format!("{e}"))?;
    let program = parser::parse(tokens).map_err(|e| format!("{e}"))?;

    // 8-pyi.B: carga stubs .pyi adyacentes antes del check (también en
    // tests — los `tests/*.fitz` que usan `from python import foo` ven
    // los tipos del `foo.pyi` adyacente).
    let (_env, _types, _defs, type_errors) = check_program_with_pyi_stubs(&program, path);
    if !type_errors.is_empty() {
        let mut msg = format!("{} error(es) de tipo:", type_errors.len());
        for e in &type_errors {
            msg.push_str(&format!("\n  {}", e));
        }
        return Err(msg);
    }

    let base_dir = base_dir_for_stub_lookup(path);

    evaluator::eval_with_base_and_deps(program, base_dir, dep_registry.clone())
        .await
        .map_err(|e| format!("{e}"))
}

/// Corre todos los tests del registry, aplica `filter` opcional,
/// reporta estilo cargo (`test <name> ... ok/FAILED`), summary
/// final, y devuelve la cantidad de tests fallidos. El caller usa
/// ese número para decidir el exit code (`>0` → 1).
///
/// Output:
/// - `running N tests` (o `N (M filtered out)` si hay filter).
/// - Por cada test: `test <full_name> ... <result>` con result
///   coloreado (ok verde, FAILED rojo) si stdout es TTY.
/// - Si hay failures, sección `failures:` con detalle de cada uno
///   (mensaje del FitzError o EvalSignal).
/// - Summary: `test result: ok|FAILED. P passed; F failed; finished in Ts`.
fn run_test_registry(registry: &testing::TestRegistry, filter: Option<&str>) -> usize {
    use std::io::IsTerminal;

    // ANSI raw — usar colors solo si stdout es TTY (no redirigido).
    let use_color = std::io::stdout().is_terminal();
    let green = |s: &str| {
        if use_color {
            format!("\x1b[32m{s}\x1b[0m")
        } else {
            s.into()
        }
    };
    let red = |s: &str| {
        if use_color {
            format!("\x1b[31m{s}\x1b[0m")
        } else {
            s.into()
        }
    };
    let bold = |s: &str| {
        if use_color {
            format!("\x1b[1m{s}\x1b[0m")
        } else {
            s.into()
        }
    };

    let all = registry.tests();
    let total_discovered = all.len();

    // Aplicar filtro. Tests excluidos se cuentan como "filtered out"
    // en el output (cargo style).
    let selected: Vec<&testing::TestSpec> = match filter {
        Some(needle) => all.iter().filter(|t| t.name.contains(needle)).collect(),
        None => all.iter().collect(),
    };
    let filtered_out = total_discovered - selected.len();

    let plural = |n: usize| if n == 1 { "test" } else { "tests" };
    if filtered_out > 0 {
        println!(
            "\nrunning {} {} ({} filtered out)",
            selected.len(),
            plural(selected.len()),
            filtered_out
        );
    } else {
        println!("\nrunning {} {}", selected.len(), plural(selected.len()));
    }

    if selected.is_empty() {
        println!("\ntest result: {}. 0 passed; 0 failed", green("ok"));
        return 0;
    }

    let start = std::time::Instant::now();
    let mut failures: Vec<(String, String)> = Vec::new(); // (full_name, error_msg)
    let runtime = evaluator::build_runtime();

    for test in &selected {
        let full_name = match &test.source_file {
            Some(src) => format!("{}::{}", src, test.name),
            None => test.name.clone(),
        };
        // Imprimimos "test <name> ..." y dejamos pendiente el OK/FAILED
        // para imprimir después de ejecutar (cargo lo hace en la misma
        // línea con un buffer — acá usamos print! + flush).
        print!("test {} ... ", full_name);
        std::io::Write::flush(&mut std::io::stdout()).ok();

        let outcome = runtime.block_on(invoke_one_test(test));
        match outcome {
            Ok(()) => println!("{}", green("ok")),
            Err(msg) => {
                println!("{}", red("FAILED"));
                failures.push((full_name, msg));
            }
        }
    }

    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();

    if !failures.is_empty() {
        println!("\nfailures:");
        for (name, msg) in &failures {
            println!("\n---- {} stdout ----\n{}", name, msg);
        }
        println!("\nfailures:");
        for (name, _) in &failures {
            println!("    {}", name);
        }
    }

    let passed = selected.len() - failures.len();
    let result_label = if failures.is_empty() {
        green("ok")
    } else {
        red("FAILED")
    };
    println!(
        "\ntest result: {}. {} passed; {} failed; finished in {:.2}s",
        result_label,
        bold(&passed.to_string()),
        bold(&failures.len().to_string()),
        secs,
    );

    failures.len()
}

/// Invoca un test individual via `evaluator::run_test_handler`.
/// Cualquier `FitzError` se devuelve como `Err(formatted_string)` —
/// el runner lo registra en la sección `failures:` del output.
async fn invoke_one_test(test: &testing::TestSpec) -> Result<(), String> {
    evaluator::run_test_handler(test.handler.clone(), test.is_async, &test.name)
        .await
        .map_err(|e| format!("{e}"))
}

// ---- Fase 9.z.3 — `fitz dev` (hot reload) ----

/// Resuelto al inicio de `dev_cmd`: qué directorio watcheamos y qué
/// argumentos le pasamos al child `fitz run`. Single-file mode usa el
/// parent del archivo como watch root + `fitz run <file>`; manifest
/// mode usa `manifest_dir` como root + `fitz run` (sin args, así el
/// child re-descubre el manifest cada arranque y respeta cambios de
/// `[bin].main` en `fitz.toml`).
struct DevTarget {
    /// Directorio que el watcher monitorea recursivamente.
    watch_dir: PathBuf,
    /// Args adicionales para el child `fitz run ...`.
    child_args: Vec<String>,
    /// String corta para el banner UX ("`./mi_app.fitz`" o
    /// "proyecto `miapp`").
    display: String,
}

/// Entry point del sub-comando `fitz dev` (Fase 9.z.3).
///
/// Loop principal: spawn child `fitz run <entry>`, escucha cambios en
/// el filesystem, y al detectar uno relevante (archivo `.fitz` o
/// `fitz.toml`, no excluido), mata el child y respawnea. Ctrl+C mata
/// el child antes de salir para evitar procesos zombie.
///
/// Toda la lógica corre adentro de un runtime tokio current_thread
/// porque combina `tokio::process` (kill async del child),
/// `tokio::signal::ctrl_c`, y un canal async para los eventos de
/// `notify` (que es sync; lo reenviamos vía `std::thread::spawn` +
/// `tokio::sync::mpsc::UnboundedSender`).
fn dev_cmd(file_arg: Option<PathBuf>) {
    let target = resolve_dev_target(file_arg);

    eprintln!("🔄 fitz dev — watching {}", target.watch_dir.display());
    eprintln!("   ejecutando: {}", target.display);
    eprintln!("   (Ctrl+C para salir)\n");

    let runtime = evaluator::build_runtime();
    runtime.block_on(async move {
        if let Err(e) = run_dev_loop(target).await {
            eprintln!("✗ fitz dev: {e}");
            std::process::exit(1);
        }
    });
}

/// Decide qué directorio watchear + qué args pasarle al child.
fn resolve_dev_target(file_arg: Option<PathBuf>) -> DevTarget {
    if let Some(path) = file_arg {
        // Single-file mode: watch el parent del archivo, child con
        // `fitz run <file>` (path absolute para evitar problemas si
        // el cwd cambia).
        let abs = std::fs::canonicalize(&path).unwrap_or_else(|e| {
            eprintln!("✗ no se pudo resolver `{}`: {e}", path.display());
            std::process::exit(1);
        });
        let watch_dir = abs
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let display = abs.display().to_string();
        return DevTarget {
            watch_dir,
            child_args: vec!["run".into(), abs.to_string_lossy().into()],
            display,
        };
    }

    // Manifest mode: encontrar fitz.toml, watch su directorio. Child
    // `fitz run` sin args para que re-descubra el manifest cada
    // arranque (si el user edita `[bin].main`, se respeta).
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("✗ no se pudo leer el directorio actual: {e}");
        std::process::exit(1);
    });
    let manifest_path = match manifest::find_manifest(&cwd) {
        Some(p) => p,
        None => {
            eprintln!(
                "✗ no se encontró `{}` en `{}` ni en directorios padre.\n   \
                 Pasá un archivo explícito (`fitz dev --file archivo.fitz`) o creá \
                 un proyecto con `fitz new <nombre>` / `fitz init`.",
                manifest::MANIFEST_FILE,
                cwd.display()
            );
            std::process::exit(1);
        }
    };
    let manifest_dir = manifest_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.clone());
    // Parsear nombre del paquete para el banner.
    let display = match fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|t| manifest::Manifest::parse(&t).ok())
    {
        Some(m) => format!("proyecto `{}`", m.package.name),
        None => format!("proyecto en `{}`", manifest_dir.display()),
    };
    DevTarget {
        watch_dir: manifest_dir,
        child_args: vec!["run".into()],
        display,
    }
}

/// Loop principal del dev: spawnea child + escucha cambios + Ctrl+C.
/// Cada iteración del outer loop = un "run" del programa. Cuando un
/// archivo relevante cambia, kill+respawn. Cuando Ctrl+C llega,
/// kill child y return Ok.
async fn run_dev_loop(target: DevTarget) -> Result<(), String> {
    // Canal sync → async para los eventos del watcher. notify es sync;
    // un std::thread re-envía cada evento al canal tokio.
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(notify_tx)
        .map_err(|e| format!("no se pudo crear el file watcher: {e}"))?;
    use notify::Watcher;
    watcher
        .watch(&target.watch_dir, notify::RecursiveMode::Recursive)
        .map_err(|e| format!("no se pudo watch-ear `{}`: {e}", target.watch_dir.display()))?;

    let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::unbounded_channel::<notify::Event>();
    std::thread::spawn(move || {
        // Ignoramos errores del watcher (`Err`): el SO a veces emite
        // ruido (paths efímeros, permisos transitorios) que no nos
        // afecta. Si el canal tokio cierra (`send().is_err()`), el
        // consumer murió y salimos.
        for event in notify_rx.into_iter().flatten() {
            if tokio_tx.send(event).is_err() {
                break;
            }
        }
    });

    let bin = std::env::current_exe()
        .map_err(|e| format!("no se pudo encontrar el binario `fitz` actual: {e}"))?;

    let mut run_count: u32 = 1;
    loop {
        clear_screen_and_banner(&target, run_count);

        // Spawn child con working dir = el watch_dir para que `fitz run`
        // (sin args, manifest mode) encuentre el manifest. Single-file
        // mode usa el path absoluto del archivo, así que el cwd no importa.
        let mut child = match tokio::process::Command::new(&bin)
            .args(&target.child_args)
            .current_dir(&target.watch_dir)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("✗ no se pudo spawnear el child: {e}");
                // Si no podemos spawnear, igual queremos seguir escuchando
                // por si el user fixea (path inexistente, permisos, etc.).
                // Esperamos un cambio + retry.
                drain_until_change(&mut tokio_rx, &target.watch_dir).await;
                continue;
            }
        };

        // Inner loop: esperamos cambio en filesystem, Ctrl+C, o child exit.
        let restart = tokio::select! {
            change = wait_for_relevant_change(&mut tokio_rx, &target.watch_dir) => {
                let path = change;
                // Debounce: 100ms drain del canal para colapsar saves múltiples.
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    drain_pending(&mut tokio_rx),
                )
                .await;
                eprintln!(
                    "\n↻ cambio detectado en {} — reiniciando ...",
                    relative_to(&path, &target.watch_dir)
                );
                true
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\n👋 Ctrl+C recibido — matando child y saliendo");
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Ok(());
            }
            status = child.wait() => {
                // El child terminó solo (programa CLI corto, error de tipo, etc.).
                // Mostramos el status y esperamos un cambio para reiniciar.
                match status {
                    Ok(s) if s.success() => {
                        eprintln!("\n✓ programa terminó OK (exit 0) — esperando cambios ...");
                    }
                    Ok(s) => {
                        eprintln!(
                            "\n✗ programa terminó con error (exit {}) — esperando cambios ...",
                            s.code().unwrap_or(-1)
                        );
                    }
                    Err(e) => {
                        eprintln!("\n✗ error esperando al child: {e}");
                    }
                }
                drain_until_change(&mut tokio_rx, &target.watch_dir).await;
                eprintln!("\n↻ reiniciando ...");
                false
            }
        };

        // Kill del child si seguía vivo (case "restart por cambio").
        if restart {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        run_count += 1;
    }
}

/// Espera el próximo evento del watcher que toque un archivo relevante
/// (`.fitz` o `fitz.toml`, no excluido). Eventos irrelevantes se
/// drenan silenciosamente.
async fn wait_for_relevant_change(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<notify::Event>,
    watch_dir: &std::path::Path,
) -> PathBuf {
    loop {
        let Some(ev) = rx.recv().await else {
            // El canal cerró (el thread del watcher murió). Esto NO debería
            // pasar en uso normal; tratamos como cambio sintético para que
            // el loop salga.
            return watch_dir.to_path_buf();
        };
        for p in &ev.paths {
            if path_is_relevant(p, watch_dir) {
                return p.clone();
            }
        }
    }
}

/// Drena eventos en el canal sin bloquear (poll). Usado para el
/// debounce: tras detectar UN evento, drenamos los que llegan en los
/// próximos 100ms para colapsar saves múltiples.
async fn drain_pending(rx: &mut tokio::sync::mpsc::UnboundedReceiver<notify::Event>) {
    loop {
        match rx.try_recv() {
            Ok(_) => continue,
            Err(_) => {
                // Esperamos un poco para que lleguen los próximos eventos
                // del save múltiple (típico en VSCode: write tmp, rename,
                // chmod). El timeout exterior corta a 100ms total.
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                if rx.try_recv().is_err() {
                    return;
                }
            }
        }
    }
}

/// Bloquea hasta que llegue un cambio relevante. Versión "loop hasta
/// que algo pase" usada cuando el child terminó solo y esperamos al
/// próximo save del user.
async fn drain_until_change(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<notify::Event>,
    watch_dir: &std::path::Path,
) {
    let _ = wait_for_relevant_change(rx, watch_dir).await;
    // Debounce post-cambio.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), drain_pending(rx)).await;
}

/// Decide si un path del evento merece restart. Reglas:
///
/// - Sólo `.fitz` o `fitz.toml` (otras extensiones se ignoran).
/// - Excluye paths bajo `target/`, `.git/`, `node_modules/`,
///   `.fitz/`, archivos ocultos (`.algo`).
fn path_is_relevant(path: &std::path::Path, watch_dir: &std::path::Path) -> bool {
    // Filename check primero (más barato).
    let is_fitz_file = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| ext == "fitz")
        .unwrap_or(false);
    let is_manifest = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == manifest::MANIFEST_FILE)
        .unwrap_or(false);
    if !is_fitz_file && !is_manifest {
        return false;
    }

    // Componentes excluidos en cualquier nivel.
    let rel = path.strip_prefix(watch_dir).unwrap_or(path);
    for component in rel.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let s = name.to_string_lossy();
        if matches!(
            s.as_ref(),
            "target" | ".git" | "node_modules" | ".fitz" | "dist" | "build"
        ) {
            return false;
        }
        // Cualquier otro componente que arranca con `.` es oculto.
        // Excepto el archivo final si es `.fitz` literal — pero ya
        // chequeamos extensión, así que un archivo `.algo.fitz`
        // (hidden con extensión fitz) sí dispara. Razonable.
        if s.starts_with('.') && s != "." && s != ".." && !s.ends_with(".fitz") {
            return false;
        }
    }
    true
}

/// Para mensajes UX: muestra el path como relativo al watch_dir si
/// está adentro, o tal cual si está afuera.
fn relative_to(path: &std::path::Path, base: &std::path::Path) -> String {
    path.strip_prefix(base)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

// ---- Fase 9.z.4 — `fitz repl` (REPL interactivo) ----

/// Entry point del sub-comando `fitz repl` (Fase 9.z.4). Abre un
/// prompt interactivo donde cada línea se evalúa contra un env
/// compartido. Soporta multi-line continuation cuando hay
/// `{`/`(`/`[` abierto, comandos especiales con prefijo `:`,
/// history persistente en `~/.fitz/history`, y Ctrl+D para salir.
///
/// Toda la lógica corre adentro de un runtime tokio current_thread
/// (`evaluator::build_runtime`) porque el evaluator es async desde
/// Fase 6.4 y necesitamos await-ear `Value::Future` para que
/// `sleep(100).await` y similares funcionen desde el prompt.
fn repl_cmd() {
    println!("Fitz REPL");
    println!("Tipos: `:help` para comandos disponibles. Ctrl+D para salir.\n");

    let mut editor = match rustyline::DefaultEditor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("✗ no se pudo inicializar el REPL: {e}");
            std::process::exit(1);
        }
    };
    let history_path = repl_history_path();
    if let Some(ref p) = history_path {
        // Si el archivo no existe es OK — primera sesión. Cualquier otro
        // error (permisos, fs corrupto) se ignora silencioso para no
        // ensuciar la UX del arranque; rustyline igual maneja la sesión
        // sin history persistente.
        let _ = editor.load_history(p);
    }

    let runtime = evaluator::build_runtime();
    runtime.block_on(async move {
        repl_loop(&mut editor, history_path.as_deref()).await;
    });
}

/// Path al archivo de history del REPL: `~/.fitz/history`. Si no
/// podemos resolver el home dir (caso muy raro), devolvemos `None` y
/// la sesión corre sin history persistente.
fn repl_history_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let dir = PathBuf::from(home).join(".fitz");
    // Mejor intentamos crear el directorio acá; si falla, dejamos que
    // rustyline lo gestione en el `save_history` (que también va a
    // fallar pero silencioso).
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("history"))
}

/// Loop principal del REPL. Cada iteración: read una línea (o varias
/// si está incompleta), procesar comandos especiales `:`, parsear,
/// evaluar contra el env compartido, imprimir el valor si era una
/// expresión top-level.
async fn repl_loop(editor: &mut rustyline::DefaultEditor, history_path: Option<&std::path::Path>) {
    let mut env = evaluator::new_repl_env();
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    loop {
        let buffer = match read_complete_input(editor) {
            Ok(b) => b,
            Err(ReplReadError::Interrupted) => {
                // Ctrl+C: limpio buffer multi-line si había, vuelvo al prompt.
                println!("(Ctrl+C — cancelado)");
                continue;
            }
            Err(ReplReadError::Eof) => {
                println!("\n👋 hasta luego!");
                if let Some(p) = history_path {
                    let _ = editor.save_history(p);
                }
                return;
            }
            Err(ReplReadError::Other(e)) => {
                eprintln!("✗ error leyendo input: {e}");
                return;
            }
        };

        if buffer.trim().is_empty() {
            continue;
        }
        // Lo agregamos a la history sólo si no es vacío. rustyline
        // dedupea automáticamente la línea anterior idéntica.
        let _ = editor.add_history_entry(buffer.as_str());

        // Comandos especiales: `:help`, `:quit`, `:type`, `:env`,
        // `:reset`, `:load`. Si la línea arranca con `:` (sin espacios
        // previos), la tratamos como comando.
        let trimmed = buffer.trim_start();
        if let Some(cmd) = trimmed.strip_prefix(':') {
            match handle_special_command(cmd, &mut env, &base_dir).await {
                ReplCommandResult::Continue => {}
                ReplCommandResult::Quit => {
                    println!("👋 hasta luego!");
                    if let Some(p) = history_path {
                        let _ = editor.save_history(p);
                    }
                    return;
                }
            }
            continue;
        }

        // Evaluamos como código Fitz. Errores de lexer/parser/checker
        // se muestran y volvemos al prompt sin abortar.
        eval_repl_input(&buffer, &mut env, &base_dir).await;
    }
}

/// Resultado de procesar una línea con `rustyline`: lectura OK,
/// Ctrl+C (cancela buffer multi-line), Ctrl+D (sale), o error
/// inesperado.
enum ReplReadError {
    Interrupted,
    Eof,
    Other(String),
}

/// Lee una entrada COMPLETA del usuario: una o más líneas hasta que
/// los brackets/parens/braces/strings estén balanceados. Devuelve el
/// buffer concatenado.
///
/// El prompt cambia entre líneas: `fitz> ` para la primera línea,
/// `...   ` para continuations. Mantiene el visual aligned con `fitz>`
/// (4 chars cada uno).
fn read_complete_input(editor: &mut rustyline::DefaultEditor) -> Result<String, ReplReadError> {
    use rustyline::error::ReadlineError;

    let mut buffer = String::new();
    loop {
        let prompt = if buffer.is_empty() {
            "fitz> "
        } else {
            "...   "
        };
        let line = editor.readline(prompt);
        match line {
            Ok(line) => {
                buffer.push_str(&line);
                buffer.push('\n');
                if input_is_complete(&buffer) {
                    return Ok(buffer);
                }
                // Si no está completo, seguimos pidiendo más líneas.
            }
            Err(ReadlineError::Interrupted) => return Err(ReplReadError::Interrupted),
            Err(ReadlineError::Eof) => return Err(ReplReadError::Eof),
            Err(e) => return Err(ReplReadError::Other(format!("{e}"))),
        }
    }
}

/// Heurística de "input completo": balanced `{`/`(`/`[` + sin string
/// literal abierto. Para multi-line continuation cuando el usuario
/// escribe un bloque (`fn`, `if`, `match`) o expresión compleja.
///
/// Maneja:
/// - String literals `"..."` con escapes `\"`.
/// - Comments de línea `//` (ignora resto hasta `\n`).
/// - Comments multi-línea `/* ... */`.
///
/// No es un parser real — heurística suficiente para multi-line
/// detection. El parser real puede aún fallar con un error sintáctico
/// distinto; el REPL lo muestra y vuelve al prompt.
fn input_is_complete(buf: &str) -> bool {
    let mut braces = 0i32;
    let mut parens = 0i32;
    let mut brackets = 0i32;
    let mut in_str = false;
    let mut escape = false;
    let mut chars = buf.chars().peekable();
    while let Some(c) = chars.next() {
        if escape {
            escape = false;
            continue;
        }
        if in_str {
            match c {
                '\\' => escape = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => braces += 1,
            '}' => braces -= 1,
            '(' => parens += 1,
            ')' => parens -= 1,
            '[' => brackets += 1,
            ']' => brackets -= 1,
            '/' if chars.peek() == Some(&'/') => {
                // Line comment: skip hasta \n.
                for c2 in chars.by_ref() {
                    if c2 == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                // Block comment: skip hasta `*/`.
                chars.next(); // consume el `*`
                let mut prev = ' ';
                for c2 in chars.by_ref() {
                    if prev == '*' && c2 == '/' {
                        break;
                    }
                    prev = c2;
                }
            }
            _ => {}
        }
    }
    !in_str && braces <= 0 && parens <= 0 && brackets <= 0
}

/// Resultado de un comando especial: `Continue` vuelve al prompt,
/// `Quit` sale del REPL.
enum ReplCommandResult {
    Continue,
    Quit,
}

/// Procesa un comando especial `:nombre [args]`. La línea ya viene
/// sin el `:` inicial (consumida por el caller).
async fn handle_special_command(
    cmd: &str,
    env: &mut fitz::env::EnvRef,
    base_dir: &std::path::Path,
) -> ReplCommandResult {
    let cmd = cmd.trim();
    let (name, args) = match cmd.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (cmd, ""),
    };
    match name {
        "help" | "h" => {
            print_repl_help();
        }
        "quit" | "q" | "exit" => return ReplCommandResult::Quit,
        "env" => print_repl_env(env),
        "reset" => {
            *env = evaluator::new_repl_env();
            println!("✓ scope reseteado");
        }
        "type" | "t" => {
            if args.is_empty() {
                println!("uso: `:type <expr>` — ej. `:type 1 + 2`");
            } else {
                print_repl_type(args, env);
            }
        }
        "load" => {
            if args.is_empty() {
                println!("uso: `:load <archivo.fitz>`");
            } else {
                load_into_repl_env(args, env, base_dir).await;
            }
        }
        other => {
            println!("comando desconocido `:{other}`. Tipeá `:help` para la lista.");
        }
    }
    ReplCommandResult::Continue
}

fn print_repl_help() {
    println!("Comandos del REPL:");
    println!("  :help, :h       — esta ayuda");
    println!("  :quit, :q       — salir (también Ctrl+D)");
    println!("  :env            — listar variables y fns definidas en el scope");
    println!("  :reset          — limpiar el scope (perdés todo)");
    println!("  :type <expr>    — mostrar el tipo de una expresión");
    println!("  :load <archivo> — evaluar un .fitz en el scope actual");
}

/// Imprime las variables del scope raíz, excluyendo builtins
/// (`print`/`len`/etc.) que no son interesantes para el usuario.
fn print_repl_env(env: &fitz::env::EnvRef) {
    let names = env.lock().local_names();
    let builtins: std::collections::HashSet<&str> =
        evaluator::builtin_names().iter().copied().collect();
    let user_names: Vec<String> = names
        .into_iter()
        .filter(|n| !builtins.contains(n.as_str()))
        .collect();
    if user_names.is_empty() {
        println!("(scope vacío — no definiste nada todavía)");
        return;
    }
    println!("Definido en el scope:");
    for name in user_names {
        let value = env.lock().get(&name);
        match value {
            Some(v) => println!("  {} = {}  // {}", name, v, v.type_name()),
            None => println!("  {} = ?", name),
        }
    }
}

/// Implementa `:type <expr>`. Parsea la expresión + chequea contra
/// los nombres existentes en el env del REPL, después imprime el tipo
/// sintetizado.
///
/// Pragmático: el checker corre sobre el programa entero (un solo
/// `Stmt::Expr`), no sobre la expresión aislada — eso permite que
/// `:type x + 1` con `x: Int` previo refleje que el resultado es
/// `Int`. Implementación: sintetizamos un `let __repl_type = <expr>`
/// y le preguntamos al checker el tipo del binding. El env del REPL
/// solo importa para que el ident `x` no falte; el checker reconstruye
/// los bindings desde cero al ver el programa, por eso un `let x =
/// "hola"` previo no influye en este path. Como mejora futura: feeding
/// del env del REPL al checker.
fn print_repl_type(expr_src: &str, _env: &fitz::env::EnvRef) {
    let synthesized = format!("let __repl_type = {expr_src}");
    let tokens = match fitz::lexer::tokenize(&synthesized) {
        Ok(t) => t,
        Err(e) => {
            println!("✗ {e}");
            return;
        }
    };
    let program = match fitz::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            println!("✗ {e}");
            return;
        }
    };
    let (type_env, types, _defs, _errs) = fitz::types::check_program(&program);
    // El último stmt es `Stmt::Assign` con value = la expr. Su tipo
    // sintetizado está en TypeInfo bajo el span del value.
    let last = program.last();
    if let Some(fitz::ast::Stmt::Assign { value, .. }) = last {
        let span = value.span();
        if let Some(t) = types.type_at(span) {
            println!(":: {}", t.display(&type_env));
        } else {
            println!(":: <no resoluble> (deuda: el checker no registró span)");
        }
    } else {
        println!("✗ no pude evaluar la expresión");
    }
}

/// Implementa `:load <archivo>`. Lee el archivo, parsea + chequea +
/// evalúa contra el env del REPL. Los `let`/`fn` definidos en el
/// archivo quedan disponibles para las siguientes líneas del prompt.
async fn load_into_repl_env(
    path_str: &str,
    env: &mut fitz::env::EnvRef,
    base_dir: &std::path::Path,
) {
    let path = std::path::Path::new(path_str);
    let resolved: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };
    let source = match fs::read_to_string(&resolved) {
        Ok(s) => s,
        Err(e) => {
            println!("✗ no se pudo leer `{}`: {e}", resolved.display());
            return;
        }
    };
    let tokens = match fitz::lexer::tokenize(&source) {
        Ok(t) => t,
        Err(e) => {
            println!("✗ {e}");
            return;
        }
    };
    let program = match fitz::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            println!("✗ {e}");
            return;
        }
    };
    let (_env, _types, _defs, type_errors) = fitz::types::check_program(&program);
    if !type_errors.is_empty() {
        for e in &type_errors {
            println!("✗ {e}");
        }
        return;
    }
    let load_base = resolved
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| base_dir.to_path_buf());
    match evaluator::eval_program_with_env(
        program,
        load_base,
        env.clone(),
        manifest::DepRegistry::new(),
    )
    .await
    {
        Ok(_) => println!("✓ cargado {}", resolved.display()),
        Err(e) => println!("✗ {e}"),
    }
}

/// Evalúa la entrada del usuario como código Fitz. El último stmt del
/// programa, si es `Stmt::Expr`, se evalúa devolviendo un `Value` que
/// se imprime (paralelo a Python `_`). Para los demás stmts (let,
/// fn, etc.) el output es silencioso.
async fn eval_repl_input(source: &str, env: &mut fitz::env::EnvRef, base_dir: &std::path::Path) {
    let tokens = match fitz::lexer::tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            println!("✗ {e}");
            return;
        }
    };
    let program = match fitz::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            println!("✗ {e}");
            return;
        }
    };
    // Checker en modo warning (paralelo a `fitz run --no-typecheck`):
    // el REPL es para experimentar; preferimos que el user vea el
    // resultado runtime incluso si los tipos son ambiguos. Errores
    // duros (sintaxis) ya cortaron arriba.
    //
    // Filtramos "variable desconocida" específicamente porque el
    // checker arma su scope desde cero por línea — ignora las vars
    // que el user definió en líneas anteriores. El eval contra `env`
    // sí las ve. Sin este filtro, cada `let x = 1; x + 1` emitía un
    // warning spurio del checker para `x` en la segunda línea. Si la
    // var realmente no existe, `eval_program_with_env` aborta más
    // abajo con su propio error.
    //
    // Filtramos por substring del mensaje porque todos los errores
    // del checker llevan `ErrorKind::TypeError` (el `UndefinedVariable`
    // es kind del evaluator). El string "variable desconocida" está
    // hardcoded en `types::infer_expr` y es estable.
    let (_env, _types, _defs, type_errors) = fitz::types::check_program(&program);
    for e in &type_errors {
        if e.message.contains("variable desconocida") {
            continue;
        }
        println!("⚠ {e}");
    }

    // Detectamos si el último stmt es `Stmt::Expr` para decidir si
    // imprimir el resultado (Python-style). El eval devuelve el
    // `Value` del último stmt; sólo lo mostramos cuando vino de una
    // expresión y no es Null (print/let/fn devuelven Null y no
    // queremos ruido visual).
    let last_is_expr = matches!(program.last(), Some(fitz::ast::Stmt::Expr(_, _)));
    match evaluator::eval_program_with_env(
        program,
        base_dir.to_path_buf(),
        env.clone(),
        manifest::DepRegistry::new(),
    )
    .await
    {
        Ok(value) => {
            if last_is_expr && !matches!(value, fitz::value::Value::Null) {
                println!("= {}", value);
            }
        }
        Err(e) => {
            println!("✗ {e}");
        }
    }
}

// ---- Fase 9.z.5 — `fitz lint` (linter de patrones más allá de tipos) ----

/// Entry point del sub-comando `fitz lint`. Descubre archivos
/// (single-file o manifest mode), corre el linter sobre cada uno,
/// imprime findings estilo cargo-clippy, decide exit code según
/// `--deny`.
///
/// Default: exit 0 incluso con findings (warnings no rompen build).
/// Si algún finding matchea un name listado en `--deny`, exit 1.
fn lint_cmd(files: Vec<PathBuf>, deny: Vec<String>) {
    let targets = if files.is_empty() {
        discover_project_fitz_files()
    } else {
        files
    };
    if targets.is_empty() {
        eprintln!("✗ no se encontraron archivos `.fitz` para lintear.");
        std::process::exit(1);
    }

    let deny_set: std::collections::HashSet<String> = deny.into_iter().collect();
    let mut total_findings: usize = 0;
    let mut denied_findings: usize = 0;
    let mut read_errors: usize = 0;

    use std::io::IsTerminal;
    let use_color = std::io::stdout().is_terminal();

    for path in &targets {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("✗ no se pudo leer `{}`: {e}", path.display());
                read_errors += 1;
                continue;
            }
        };
        let tokens = match lexer::tokenize(&source) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("✗ `{}`: {e}", path.display());
                read_errors += 1;
                continue;
            }
        };
        let program = match parser::parse(tokens) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("✗ `{}`: {e}", path.display());
                read_errors += 1;
                continue;
            }
        };

        let findings = lint::lint_source(&source, &program);
        for f in &findings {
            print_lint_finding(path, f, use_color, deny_set.contains(f.name));
            total_findings += 1;
            if deny_set.contains(f.name) {
                denied_findings += 1;
            }
        }
    }

    // Summary final.
    if total_findings == 0 && read_errors == 0 {
        if use_color {
            println!(
                "\n\x1b[32m✓ sin findings\x1b[0m ({} archivo(s) revisado(s))",
                targets.len()
            );
        } else {
            println!(
                "\n✓ sin findings ({} archivo(s) revisado(s))",
                targets.len()
            );
        }
    } else {
        let f_word = if total_findings == 1 {
            "finding"
        } else {
            "findings"
        };
        println!(
            "\n{} {} en {} archivo(s){}",
            total_findings,
            f_word,
            targets.len(),
            if denied_findings > 0 {
                format!(" ({} denied)", denied_findings)
            } else {
                String::new()
            }
        );
    }

    if read_errors > 0 || denied_findings > 0 {
        std::process::exit(1);
    }
}

/// Imprime un finding estilo cargo-clippy:
/// ```text
/// warning: variable `x` declarada pero no usada
///   --> src/main.fitz:3:5
///   = nota: si es intencional, prefijá con `_` ...
/// ```
/// Con `--deny <name>`, se usa "error:" rojo en lugar de "warning:"
/// amarillo.
fn print_lint_finding(
    path: &std::path::Path,
    finding: &lint::LintFinding,
    use_color: bool,
    denied: bool,
) {
    let (label, color_code) = if denied {
        ("error", "\x1b[31m")
    } else {
        ("warning", "\x1b[33m")
    };
    if use_color {
        println!(
            "\n{}{}\x1b[0m: {} \x1b[2m[{}]\x1b[0m",
            color_code, label, finding.message, finding.name
        );
        println!(
            "  \x1b[36m-->\x1b[0m {}:{}:{}",
            path.display(),
            finding.line,
            finding.column
        );
        if let Some(hint) = &finding.hint {
            println!("  \x1b[2m= nota:\x1b[0m {}", hint);
        }
    } else {
        println!("\n{}: {} [{}]", label, finding.message, finding.name);
        println!(
            "  --> {}:{}:{}",
            path.display(),
            finding.line,
            finding.column
        );
        if let Some(hint) = &finding.hint {
            println!("  = nota: {}", hint);
        }
    }
}

/// Banner UX al arrancar / re-arrancar el child. Limpia la pantalla
/// (ANSI `\x1b[2J\x1b[H`) si stdout es TTY, sino solo separa con
/// líneas. Después imprime el run number + el target.
fn clear_screen_and_banner(target: &DevTarget, run_count: u32) {
    use std::io::IsTerminal;
    let use_ansi = std::io::stdout().is_terminal();
    if use_ansi {
        // `\x1b[2J` borra la pantalla, `\x1b[H` mueve el cursor a
        // (1,1). Suficiente en terminals modernos (cmd, PowerShell,
        // Windows Terminal, bash, zsh, fish).
        print!("\x1b[2J\x1b[H");
    } else {
        println!("\n----------------------------------------");
    }
    eprintln!("▶ fitz dev (run #{}) — {}", run_count, target.display);
    eprintln!();
}

// =================================================================
// Fase 10.6 — Handlers de `fitz db <subcomando>`
// =================================================================

/// Resuelve la URL de conexión PG: explícita > `DATABASE_URL` env.
fn resolve_db_url(explicit: Option<String>) -> Result<String, String> {
    match explicit {
        Some(u) => Ok(u),
        None => std::env::var("DATABASE_URL").map_err(|_| {
            "URL Postgres no provista. Pasá `--url postgres://...` o seteá `DATABASE_URL` env."
                .to_string()
        }),
    }
}

/// Resuelve el dir de migrations: explícito > `./migrations` (cwd).
fn resolve_migrations_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| PathBuf::from("migrations"))
}

/// Resuelve el entry .fitz: explícito > `[bin].main` del manifest.
fn resolve_db_entry(file: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(f) = file {
        return Ok(f);
    }
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let manifest_path = manifest::find_manifest(&cwd).ok_or_else(|| {
        "no se pudo encontrar el entry — pasá `<archivo.fitz>` o asegurate de estar en un proyecto con `fitz.toml`".to_string()
    })?;
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("leyendo `{}`: {e}", manifest_path.display()))?;
    let m = manifest::Manifest::parse(&manifest_text).map_err(|e| format!("manifest: {e}"))?;
    let bin_main = m.bin.as_ref().map(|b| b.main.clone()).ok_or_else(|| {
        "el manifest no tiene `[bin].main` — pasá `<archivo.fitz>` explícito".to_string()
    })?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| "manifest sin parent".to_string())?
        .to_path_buf();
    Ok(manifest_dir.join(bin_main))
}

/// Lee + parsea + chequea un programa Fitz, devuelve (Program, TypeEnv).
/// Usado por `db diff` para construir el schema esperado.
fn load_program_for_db(entry: &std::path::Path) -> Result<(ast::Program, types::TypeEnv), String> {
    let src = std::fs::read_to_string(entry)
        .map_err(|e| format!("leyendo `{}`: {e}", entry.display()))?;
    let tokens = lexer::tokenize(&src).map_err(|e| format!("lexer: {e}"))?;
    let program = parser::parse(tokens).map_err(|e| format!("parser: {e}"))?;
    let (env, _type_info, _def_info, errs) = types::check_program(&program);
    if !errs.is_empty() {
        return Err(format!(
            "checker tipos ({} errores). Corré `fitz check` para detalles.",
            errs.len()
        ));
    }
    Ok((program, env))
}

fn db_diff_cmd(file: Option<PathBuf>, url: Option<String>, out: Option<PathBuf>) {
    let entry = match resolve_db_entry(file) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let url = match resolve_db_url(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let (program, env) = match load_program_for_db(&entry) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ cargando programa: {e}");
            std::process::exit(1);
        }
    };
    let target = match migrations::schema_from_program(&program, &env) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ schema_from_program: {e}");
            std::process::exit(1);
        }
    };
    // Una sola runtime tokio para connect + introspect: la connection
    // arma `tokio::spawn(health_check_task)`; si dropeamos el runtime
    // entre connect y query, esa task muere y la próxima query rompe
    // con "cstr no es UTF-8" o similar.
    let rt = evaluator::build_runtime();
    let result = rt.block_on(async {
        let conn = db::connect_url(&url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let current = migrations::introspect_schema(&conn)
            .await
            .map_err(|e| format!("introspect: {e}"))?;
        Ok::<_, String>(current)
    });
    let current = match result {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let changes = migrations::diff_schemas(&current, &target);
    if changes.is_empty() {
        eprintln!("✓ schema sincronizado — no hay cambios pendientes");
        return;
    }
    let sql = migrations::changes_to_sql(&changes);
    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &sql) {
                eprintln!("✗ escribiendo `{}`: {e}", path.display());
                std::process::exit(1);
            }
            eprintln!("✓ {} change(s) → {}", changes.len(), path.display());
        }
        None => {
            print!("{}", sql);
            eprintln!("✓ {} change(s) emitidos a stdout", changes.len());
        }
    }
}

fn db_migrate_cmd(url: Option<String>, dir: Option<PathBuf>, dry_run: bool) {
    let url = match resolve_db_url(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let dir = resolve_migrations_dir(dir);
    let migrations_list = match migrations::read_migrations_dir(&dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if migrations_list.is_empty() {
        eprintln!(
            "✓ no hay archivos `.sql` en `{}` — nada para migrar",
            dir.display()
        );
        return;
    }
    let rt = evaluator::build_runtime();
    if dry_run {
        let result = rt.block_on(async {
            let conn = db::connect_url(&url)
                .await
                .map_err(|e| format!("connect: {e}"))?;
            migrations::status(&conn, &dir)
                .await
                .map_err(|e| format!("status: {e}"))
        });
        let report = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        };
        let mut pending = 0;
        for (version, filename, status) in &report {
            let badge = match status {
                migrations::MigrationStatus::Applied => "✓ applied ",
                migrations::MigrationStatus::Pending => {
                    pending += 1;
                    "→ PENDING "
                }
            };
            eprintln!("  {badge} {version}  {filename}");
        }
        eprintln!("[dry-run] {pending} migration(s) pendiente(s) — no se aplicaron");
        return;
    }
    // v0.10.19 — dispatch per-migration por kind: .sql via
    // `apply_migration` (SQL crudo adentro de tx), .fitz via
    // `apply_fitz_migration_blocking` (parsea + invoca async fn
    // migrate(db) + INSERT en _fitz_migrations al final).
    let result = rt.block_on(async {
        let conn = db::connect_url(&url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let applied_set = migrations::applied_versions(&conn)
            .await
            .map_err(|e| format!("applied_versions: {e}"))?;
        let applied_set: std::collections::HashSet<&str> =
            applied_set.iter().map(|s| s.as_str()).collect();
        let mut new_versions: Vec<String> = Vec::new();
        for m in &migrations_list {
            if applied_set.contains(m.version.as_str()) {
                continue;
            }
            match &m.kind {
                migrations::MigrationKind::Sql { .. } => {
                    migrations::apply_migration(&conn, m)
                        .await
                        .map_err(|e| format!("migrate `{}`: {e}", m.filename))?;
                }
                migrations::MigrationKind::Fitz { path, source } => {
                    apply_fitz_migration_async(&conn, &m.version, &m.filename, path, source)
                        .await
                        .map_err(|e| format!("migrate `{}`: {e}", m.filename))?;
                }
            }
            new_versions.push(m.version.clone());
        }
        Ok::<_, String>(new_versions)
    });
    let applied = match result {
        Ok(a) => a,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if applied.is_empty() {
        eprintln!("✓ todas las migrations ya aplicadas — no hubo cambios");
    } else {
        eprintln!("✓ {} migration(s) aplicada(s):", applied.len());
        for v in &applied {
            eprintln!("    {v}");
        }
    }
}

fn db_status_cmd(url: Option<String>, dir: Option<PathBuf>) {
    let url = match resolve_db_url(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let dir = resolve_migrations_dir(dir);
    let rt = evaluator::build_runtime();
    let result = rt.block_on(async {
        let conn = db::connect_url(&url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        migrations::status(&conn, &dir)
            .await
            .map_err(|e| format!("status: {e}"))
    });
    let report = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if report.is_empty() {
        eprintln!(
            "✓ no hay archivos `.sql` en `{}` — sin migrations tracked",
            dir.display()
        );
        return;
    }
    let mut applied = 0;
    let mut pending = 0;
    for (version, filename, status) in &report {
        let badge = match status {
            migrations::MigrationStatus::Applied => {
                applied += 1;
                "✓ applied "
            }
            migrations::MigrationStatus::Pending => {
                pending += 1;
                "→ PENDING "
            }
        };
        println!("  {badge} {version}  {filename}");
    }
    println!("\n{applied} applied, {pending} pending");
}

fn db_new_cmd(name: String, dir: Option<PathBuf>) {
    let dir = resolve_migrations_dir(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("✗ creando `{}`: {e}", dir.display());
        std::process::exit(1);
    }
    // Timestamp `YYYYMMDDHHMMSS` UTC + name sanitizado.
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d%H%M%S").to_string();
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let filename = format!("{timestamp}_{sanitized}.sql");
    let path = dir.join(&filename);
    if path.exists() {
        eprintln!("✗ ya existe: `{}`", path.display());
        std::process::exit(1);
    }
    let stub = format!(
        "-- Migration: {sanitized}\n\
         -- Created: {iso}\n\
         --\n\
         -- Editá este archivo con los statements SQL necesarios.\n\
         -- Tip: usá `fitz db diff > {filename}` para generar el SQL\n\
         -- automático desde los `@table` types del programa.\n\n\
         -- UP\n\
         \n\n\
         -- DOWN\n\
         \n",
        iso = now.to_rfc3339(),
    );
    if let Err(e) = std::fs::write(&path, &stub) {
        eprintln!("✗ escribiendo `{}`: {e}", path.display());
        std::process::exit(1);
    }
    println!("✓ {}", path.display());
}

fn db_rollback_cmd(url: Option<String>, dir: Option<PathBuf>, count: usize) {
    let url = match resolve_db_url(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let dir = resolve_migrations_dir(dir);
    let migrations_list = match migrations::read_migrations_dir(&dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    // v0.10.19 — dispatch per-migration por kind (paralelo a migrate).
    let rt = evaluator::build_runtime();
    let result = rt.block_on(async {
        let conn = db::connect_url(&url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        rollback_n_dispatch(&conn, &migrations_list, count)
            .await
            .map_err(|e| format!("rollback: {e}"))
    });
    let reverted = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if reverted.is_empty() {
        eprintln!("✓ no hay migrations aplicadas para revertir");
    } else {
        eprintln!("✓ {} migration(s) revertida(s):", reverted.len());
        for v in &reverted {
            eprintln!("    {v}");
        }
    }
}

/// v0.10.19 — Variante del `rollback_n` con dispatch por kind:
/// .sql via `migrations::revert_migration` (SQL DOWN adentro de
/// tx), .fitz via `revert_fitz_migration_async` (parsea + invoca
/// async fn rollback(db) + DELETE de _fitz_migrations). Mismo
/// pre-flight que `rollback_n` (todas las target tienen archivo +
/// rollback path resoluble) antes de tocar la DB.
async fn rollback_n_dispatch(
    conn: &std::sync::Arc<db::DbConnHandle>,
    migrations_list: &[migrations::MigrationFile],
    n: usize,
) -> Result<Vec<String>, String> {
    if n == 0 {
        return Ok(Vec::new());
    }
    migrations::ensure_tracking_table(conn)
        .await
        .map_err(|e| format!("ensure_tracking_table: {e}"))?;
    let applied = migrations::applied_versions(conn)
        .await
        .map_err(|e| format!("applied_versions: {e}"))?;
    let applied_desc: Vec<&String> = {
        // Reusar applied_versions_desc requiere exponerla; en su
        // lugar leemos applied (ASC) y reverteamos.
        let mut v: Vec<&String> = applied.iter().collect();
        v.reverse();
        v
    };
    if applied_desc.is_empty() {
        return Ok(Vec::new());
    }
    let target_versions: Vec<&String> = applied_desc.into_iter().take(n).collect();
    let by_version: std::collections::HashMap<&str, &migrations::MigrationFile> = migrations_list
        .iter()
        .map(|m| (m.version.as_str(), m))
        .collect();
    // Pre-flight: cada target debe tener file + path resoluble.
    for v in &target_versions {
        let m = by_version.get(v.as_str()).ok_or_else(|| {
            format!(
                "rollback: la version `{v}` está aplicada en la DB pero \
                 NO hay archivo en el dir de migrations. Restaurá el \
                 archivo o stampealá manualmente."
            )
        })?;
        match &m.kind {
            migrations::MigrationKind::Sql { down_sql, .. } => {
                if down_sql.is_none() {
                    return Err(format!(
                        "rollback: migration `{}` no tiene sección `-- DOWN` — \
                         no se puede revertir. Editá el archivo agregando \
                         `-- DOWN` con el SQL inverso y reintentá.",
                        m.filename
                    ));
                }
            }
            migrations::MigrationKind::Fitz { source, .. } => {
                if !fitz_migration_has_rollback(source) {
                    return Err(format!(
                        "rollback: migration `{}` (`.fitz`) no declara \
                         `async fn rollback(db: DbConn) -> Result<Null>`. \
                         Agregá la fn al archivo y reintentá.",
                        m.filename
                    ));
                }
            }
        }
    }
    let mut reverted = Vec::with_capacity(target_versions.len());
    for v in target_versions {
        let m = by_version.get(v.as_str()).expect("pre-flight validó");
        match &m.kind {
            migrations::MigrationKind::Sql { .. } => {
                migrations::revert_migration(conn, m)
                    .await
                    .map_err(|e| format!("revert `{}`: {e}", m.filename))?;
            }
            migrations::MigrationKind::Fitz { path, source } => {
                revert_fitz_migration_async(conn, &m.version, &m.filename, path, source)
                    .await
                    .map_err(|e| format!("revert `{}`: {e}", m.filename))?;
            }
        }
        reverted.push(v.clone());
    }
    Ok(reverted)
}

/// v0.10.19 (10.6.d) — Runner del `.fitz` script de una migration.
/// Convención del archivo:
///
/// ```ignore
/// async fn migrate(db: DbConn) -> Result<Null> {
///     // back-fill, parseo de JSON viejo, etc.
///     return Ok(null)
/// }
///
/// // Opcional, requerido solo si querés que `fitz db rollback` ande:
/// async fn rollback(db: DbConn) -> Result<Null> {
///     return Ok(null)
/// }
/// ```
///
/// El runner:
/// 1. Parsea el archivo y verifica que declara `migrate`.
/// 2. Inyecta `db` como var pre-bindeada al env.
/// 3. Appendea un stmt sintético al programa: `let __fitz_mig_result = migrate(db).await`.
/// 4. Eval con `evaluator::eval_program_with_env`.
/// 5. Si el último valor es `Result::Ok(_)` → trackea la migration aplicada.
/// 6. Si es `Result::Err(msg)` → error sin trackear.
///
/// **Atomicidad**: la responsabilidad de envolver en tx queda al
/// usuario (típicamente con `return db.transaction(fn(tx) -> Result<Null> { ... }).await`).
/// El runner NO envuelve automático porque la `Value::DbConn` se
/// pasa cruda; el user decide granularidad.
async fn apply_fitz_migration_async(
    conn: &std::sync::Arc<db::DbConnHandle>,
    version: &str,
    filename: &str,
    path: &std::path::Path,
    source: &str,
) -> Result<(), String> {
    run_fitz_migration_callback(conn, path, source, "migrate").await?;
    // Si la callback retornó Ok, marcamos como aplicada. Esta
    // INSERT NO está en la tx del user — si el user comiteó
    // sus cambios dentro del `.fitz`, ya persistieron; este
    // tracking es separado.
    migrations::track_fitz_migration_applied(conn, version)
        .await
        .map_err(|e| format!("track aplicada: {e}"))?;
    let _ = filename; // disponible para mensajes futuros si entra demanda
    Ok(())
}

/// v0.10.19 — Análogo a `apply_fitz_migration_async` para
/// rollback: invoca `async fn rollback(db)` + borra el registro.
async fn revert_fitz_migration_async(
    conn: &std::sync::Arc<db::DbConnHandle>,
    version: &str,
    filename: &str,
    path: &std::path::Path,
    source: &str,
) -> Result<(), String> {
    run_fitz_migration_callback(conn, path, source, "rollback").await?;
    migrations::untrack_fitz_migration(conn, version)
        .await
        .map_err(|e| format!("untrack: {e}"))?;
    let _ = filename;
    Ok(())
}

/// Helper compartido por `apply_fitz_migration_async` y
/// `revert_fitz_migration_async`. Parsea el archivo, verifica que
/// `fn_name` esté declarada como `async fn(db: DbConn) -> ...`,
/// crea env con `db` pre-bindeado a `Value::DbConn(conn)`,
/// appendea `let __fitz_mig_result = <fn_name>(db).await` y eval
/// con `evaluator::eval_program_with_env`. Inspecciona el último
/// valor: `Result::Ok(_)` → Ok(()); `Result::Err(msg)` → Err(msg).
async fn run_fitz_migration_callback(
    conn: &std::sync::Arc<db::DbConnHandle>,
    path: &std::path::Path,
    source: &str,
    fn_name: &str,
) -> Result<(), String> {
    use fitz::value::Value;
    // 1. Lex + parse.
    let tokens = lexer::tokenize(source).map_err(|e| format!("lexer: {e}"))?;
    let mut program = parser::parse(tokens).map_err(|e| format!("parser: {e}"))?;
    // 2. Verificar que `fn_name` está declarada como async fn.
    let has_callback = program.iter().any(|stmt| matches!(stmt, ast::Stmt::FnDef { name, is_async, .. } if name == fn_name && *is_async));
    if !has_callback {
        return Err(format!(
            "el archivo no declara `async fn {fn_name}(db: DbConn) -> Result<Null>`. \
             El runner de `.fitz` migrations espera esa fn como entry point."
        ));
    }
    // 3. Type-check (warning-only, paralelo a `fitz run` permissive).
    let (_env, _ti, _di, _errs) = types::check_program(&program);
    // 4. Appendear stmt sintético: `let __fitz_mig_result = <fn>(db).await`.
    let call_expr = ast::Expr::Call {
        callee: Box::new(ast::Expr::Ident(fn_name.to_string(), ast::Span::ZERO)),
        args: vec![ast::Expr::Ident("db".to_string(), ast::Span::ZERO)],
        span: ast::Span::ZERO,
    };
    let await_expr = ast::Expr::Await(Box::new(call_expr), ast::Span::ZERO);
    let assign_stmt = ast::Stmt::Assign {
        target: ast::AssignTarget::Ident("__fitz_mig_result".to_string()),
        type_: None,
        value: await_expr,
        span: ast::Span::ZERO,
    };
    program.push(assign_stmt);
    // 5. Env nuevo con builtins + db pre-bindeado.
    // `new_repl_env()` arma un EnvRef con builtins registrados —
    // mismo scope que un script `fitz run` típico.
    let env = evaluator::new_repl_env();
    // Inyectar db como Value::DbConn(Arc<DbConnHandle>).
    env.lock()
        .define("db", Value::DbConn(std::sync::Arc::clone(conn)));
    // 6. Eval.
    let base_dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let last = evaluator::eval_program_with_env(
        program,
        base_dir,
        env.clone(),
        manifest::DepRegistry::new(),
    )
    .await
    .map_err(|e| format!("eval: {e}"))?;
    // 7. Inspeccionar `__fitz_mig_result`.
    // El último stmt es el `let __fitz_mig_result = ...` cuyo
    // valor retornado por eval_program_with_env es Null (asignación).
    // Leemos el binding del env directamente para obtener el Value.
    let _ = last;
    let result = env
        .lock()
        .get("__fitz_mig_result")
        .ok_or_else(|| "interno: __fitz_mig_result no quedó bindeado".to_string())?;
    match result {
        Value::Result(variant) => match variant {
            fitz::value::ResultVariant::Ok(_) => Ok(()),
            fitz::value::ResultVariant::Err(e) => Err(format!("`{fn_name}` retornó Err: {}", *e)),
        },
        other => Err(format!(
            "`async fn {fn_name}` debe retornar Result<Null>, recibió: {other}"
        )),
    }
}

/// Heurística simple (basada en parse) para detectar si el `.fitz`
/// migration declara `async fn rollback(db: DbConn)`. Usado por
/// `rollback_n_dispatch` en el pre-flight check ANTES de tocar la
/// DB, para fallar fast con mensaje claro.
fn fitz_migration_has_rollback(source: &str) -> bool {
    let Ok(tokens) = lexer::tokenize(source) else {
        return false;
    };
    let Ok(program) = parser::parse(tokens) else {
        return false;
    };
    program.iter().any(|stmt| matches!(stmt, ast::Stmt::FnDef { name, is_async, .. } if name == "rollback" && *is_async))
}

fn db_check_cmd(file: Option<PathBuf>, url: Option<String>) {
    let entry = match resolve_db_entry(file) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let url = match resolve_db_url(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let (program, env) = match load_program_for_db(&entry) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ cargando programa: {e}");
            std::process::exit(1);
        }
    };
    let target = match migrations::schema_from_program(&program, &env) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ schema_from_program: {e}");
            std::process::exit(1);
        }
    };
    let rt = evaluator::build_runtime();
    let current = match rt.block_on(async {
        let conn = db::connect_url(&url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        migrations::introspect_schema(&conn)
            .await
            .map_err(|e| format!("introspect: {e}"))
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let changes = migrations::diff_schemas(&current, &target);
    if changes.is_empty() {
        eprintln!("✓ schema sincronizado — schema declarado matchea la DB");
        std::process::exit(0);
    }
    // Drift detectado: SQL pendiente al stderr (visible en CI logs),
    // count al stdout (parseable), exit 1.
    let sql = migrations::changes_to_sql(&changes);
    eprintln!(
        "✗ drift detectado — {} change(s) pendiente(s):",
        changes.len()
    );
    eprintln!();
    eprintln!("{}", sql);
    eprintln!(
        "💡 corré `fitz db diff > migrations/<file>.sql` + `fitz db migrate` \
         para sincronizar."
    );
    std::process::exit(1);
}

fn db_stamp_cmd(version: Option<String>, all: bool, url: Option<String>, dir: Option<PathBuf>) {
    // clap garantiza version XOR all (conflicts_with), pero validamos
    // que al menos UNO esté presente.
    if version.is_none() && !all {
        eprintln!(
            "✗ `fitz db stamp` requiere `<version>` o `--all`. \
             Ejemplos:\n  fitz db stamp 20260530120000\n  fitz db stamp --all"
        );
        std::process::exit(1);
    }
    let url = match resolve_db_url(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let dir = resolve_migrations_dir(dir);
    let migrations_list = match migrations::read_migrations_dir(&dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let rt = evaluator::build_runtime();
    if all {
        let result = rt.block_on(async {
            let conn = db::connect_url(&url)
                .await
                .map_err(|e| format!("connect: {e}"))?;
            migrations::stamp_all_pending(&conn, &migrations_list)
                .await
                .map_err(|e| format!("stamp: {e}"))
        });
        let stamped = match result {
            Ok(s) => s,
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        };
        if stamped.is_empty() {
            eprintln!("✓ no hay migrations pending para stamp");
        } else {
            eprintln!("✓ {} migration(s) stamped:", stamped.len());
            for v in &stamped {
                eprintln!("    {v}");
            }
        }
        return;
    }
    let version = version.unwrap(); // garantizado por el check arriba
                                    // Warning si la version no está en el dir (puede ser intencional
                                    // — adopción de versions legacy que no existen como files — pero
                                    // típicamente es un typo).
    let in_dir = migrations_list.iter().any(|m| m.version == version);
    if !in_dir {
        eprintln!(
            "⚠ version `{version}` NO existe en `{}` — stamped igual, \
             pero verificá que no sea un typo.",
            dir.display()
        );
    }
    let result = rt.block_on(async {
        let conn = db::connect_url(&url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        migrations::stamp_version(&conn, &version)
            .await
            .map_err(|e| format!("stamp: {e}"))
    });
    let inserted = match result {
        Ok(b) => b,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if inserted {
        eprintln!("✓ stamped: {version}");
    } else {
        eprintln!("✓ no-op: version `{version}` ya estaba aplicada");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pip_inputs_hash_es_deterministico() {
        let h1 = pip_inputs_hash(&["requests".to_string()], &[b"foo\n".to_vec()]);
        let h2 = pip_inputs_hash(&["requests".to_string()], &[b"foo\n".to_vec()]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_es_insensible_al_orden_de_positionals() {
        // Reordenar `--bundle-pip` args NO debe invalidar cache:
        // `--bundle-pip a --bundle-pip b` y `--bundle-pip b --bundle-pip a`
        // instalan el mismo set de paquetes.
        let h1 = pip_inputs_hash(
            &["sqlalchemy".to_string(), "psycopg2-binary".to_string()],
            &[],
        );
        let h2 = pip_inputs_hash(
            &["psycopg2-binary".to_string(), "sqlalchemy".to_string()],
            &[],
        );
        assert_eq!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_cambia_si_agregas_un_paquete() {
        let h1 = pip_inputs_hash(&["requests".to_string()], &[]);
        let h2 = pip_inputs_hash(&["requests".to_string(), "httpx".to_string()], &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_cambia_si_cambia_el_contenido_del_requirements() {
        let h1 = pip_inputs_hash(&[], &[b"requests>=2.0\n".to_vec()]);
        let h2 = pip_inputs_hash(&[], &[b"requests>=3.0\n".to_vec()]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_positionals_vs_requirements_son_distintos() {
        // El separador `\n---\n` garantiza que ["foo", "bar"] como
        // positionals dé hash distinto que el mismo texto en un
        // requirements file. Sin el separador, ambos producirían
        // los mismos bytes y colisionarían.
        let h1 = pip_inputs_hash(&["foo".to_string(), "bar".to_string()], &[]);
        let h2 = pip_inputs_hash(&[], &[b"bar\nfoo\n".to_vec()]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_es_16_chars_hex() {
        // Heredado de tarball_hash_short (FNV-1a 64-bit).
        let h = pip_inputs_hash(&["requests".to_string()], &[]);
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn pip_inputs_hash_vacio_devuelve_hash_estable() {
        // Sin paquetes ni requirements (caso degenerado — no debería
        // dispararse en runtime porque el bloque pip arranca solo si
        // hay algo que instalar, pero el helper sigue siendo bien
        // definido).
        let h1 = pip_inputs_hash(&[], &[]);
        let h2 = pip_inputs_hash(&[], &[]);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn pip_inputs_hash_orden_de_requirements_si_invalida_cache() {
        // Reordenar requirements files SÍ invalida cache. Razón: pip
        // los procesa en orden y dos archivos con conflicts/overrides
        // pueden producir distintos paquetes resueltos según el orden.
        // Tratamos cada permutación como input distinto, conservadora.
        let h1 = pip_inputs_hash(&[], &[b"requests\n".to_vec(), b"httpx\n".to_vec()]);
        let h2 = pip_inputs_hash(&[], &[b"httpx\n".to_vec(), b"requests\n".to_vec()]);
        assert_ne!(h1, h2);
    }
}
