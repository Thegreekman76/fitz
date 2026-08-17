// main.rs — Entry point of the Fitz compiler/interpreter.
//
// Modules live in `src/lib.rs` since Phase 9.x.1.b (lib + bin
// refactor so that `fitz-lsp` can reuse them without duplicate
// compilation). Here we only import what the CLI consumes.

use fitz::{
    ast, background_jobs, codegen, cron_jobs, db, deploy, docker, error, evaluator, fmt, http,
    launcher_template, lexer, lint, lockfile, manifest, migrations, openapi, parser, pbs,
    pyi_loader, templates, testing, types, view,
};

// `fitz py-types` sub-command (Phase 8.5) — only with the `python` feature.
#[cfg(feature = "python")]
use fitz::py_types;

use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};

// Phase 11.13 — bin-local dev server for `fitz dev`'s wasm-client
// mode (static serving + live-reload WebSocket). Not part of the
// `fitz` library — only the CLI dev loop uses it.
mod dev_server;

/// Fitz — the programming language born in Patagonia 🏔️
#[derive(Parser)]
#[command(name = "fitz")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "The Fitz programming language")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a .fitz file (or the `fitz.toml` `[bin].main` if no
    /// file is passed — Phase 9.y.2).
    Run {
        /// File to run. If omitted, looks for `fitz.toml` in the
        /// current directory or ancestors (Cargo-style) and runs
        /// its `[bin].main`.
        file: Option<PathBuf>,
        /// Phase 11.13 — Selects which `[[bin]]` to run when the
        /// manifest declares more than one (e.g. a `server` + `web`
        /// fullstack project). In single-bin projects the flag is
        /// optional (defaults to the only bin). Ignored in
        /// single-file mode.
        #[arg(long = "bin", value_name = "NAME")]
        bin: Option<String>,
        /// Skip the static type check. Without this flag, checker
        /// errors abort execution (strict mode).
        #[arg(long)]
        no_typecheck: bool,
        /// Phase 13 (v0.11.0) — Extra args passed to the Fitz
        /// program when it has `@command` decorators. The
        /// CliRegistry is populated at eval time; if it has >=1
        /// command, these args are parsed as the CLI argv
        /// (subcommand + positional + flags). Example:
        ///   `fitz run greeter.fitz -- greet Ada --loud`
        /// The `--` separates fitz args from program args.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compile to a binary (Phase 5b). With no file, reads the
    /// manifest (Phase 9.y.2) and emits the binary at
    /// `<manifest>/target/release/` with the package name.
    Build {
        /// File to compile. If omitted, looks for `fitz.toml` and
        /// compiles the selected bin (`--bin <name>` in multi-bin
        /// projects, or the only bin otherwise). Output goes to
        /// `<manifest_dir>/target/release/<pkg-name>`.
        file: Option<PathBuf>,
        /// Phase 11.5.b — Selects which `[[bin]]` to build when the
        /// manifest declares more than one. In single-bin projects
        /// the flag is optional (defaults to the only bin). Errors
        /// out listing the available bins when the requested name
        /// does not match any.
        #[arg(long = "bin", value_name = "NAME")]
        bin: Option<String>,
        /// Phase 11.5.b — Overrides the selected bin's `target`
        /// field for this build only. Accepts `native`,
        /// `wasm-client`, or `ssr` (kebab-case, matching the TOML
        /// vocabulary). Useful for one-shot experiments without
        /// editing `fitz.toml`. Cross-field validation still
        /// applies (`.fitzv` sources reject `native`, `wasm-client`
        /// requires `mount`, etc.). In single-file mode, the target
        /// is inferred from the extension (`.fitz` → `native`,
        /// `.fitzv` → `wasm-client`) and `--target` overrides.
        #[arg(long = "target", value_name = "TARGET")]
        target: Option<String>,
        /// Phase 8.b — Bundle CPython embedded into the final
        /// binary. The output is a standalone binary that does NOT
        /// require Python installed on the destination. Internally:
        /// Datasette-style launcher + PBS tarball + real binary, all
        /// embedded. Requires the program to use `from python
        /// import ...`. Adds ~30 MB (Windows) / ~45 MB (Linux x64)
        /// to the binary.
        #[arg(long = "bundle-python")]
        bundle_python: bool,
        /// Phase 8.c — Bundle pip packages along with the embedded
        /// CPython. Repeatable flag: `--bundle-pip sqlalchemy
        /// --bundle-pip psycopg2`. Accepts pip's native version
        /// pin: `--bundle-pip "sqlalchemy==2.0.0"`. Implies
        /// `--bundle-python` automatically. The builder runs
        /// `pip install --target` at build time and packs the
        /// result in a secondary tarball embedded in the launcher.
        /// On the binary's first run, packages are extracted to
        /// `<TMPDIR>/fitz-py-<hash>/python/Lib/site-packages/`
        /// (Windows) or the Unix equivalent.
        #[arg(long = "bundle-pip", value_name = "PACKAGE")]
        bundle_pip: Vec<String>,
        /// Bundle pip packages read from a `requirements.txt`.
        /// Repeatable flag: `--bundle-pip-requirements
        /// requirements.txt --bundle-pip-requirements
        /// requirements-dev.txt`. Implies `--bundle-python`
        /// automatically. The file is passed straight to
        /// `pip install -r <file>`, so all of pip's native syntax
        /// (`#` comments, `-r other.txt` includes, version pins,
        /// `--hash`, etc.) works unchanged. Combinable with
        /// `--bundle-pip <pkg>`: pip accumulates them.
        #[arg(long = "bundle-pip-requirements", value_name = "FILE")]
        bundle_pip_requirements: Vec<PathBuf>,
    },
    /// Type-check and syntax-check. With no file, reads the manifest
    /// (Phase 9.y.2) and checks the `[bin].main`.
    Check {
        /// File to check. If omitted, looks for `fitz.toml` and
        /// checks its `[bin].main`.
        file: Option<PathBuf>,
    },
    /// Emits the program's OpenAPI 3.1 schema to stdout
    Openapi {
        /// File to inspect
        file: PathBuf,
    },
    /// Phase 8.5 — Generates Fitz `type` from SQLAlchemy models
    /// defined in a Python file. Introspection uses duck typing
    /// over `__table__.columns`: any class with that shape is
    /// translated (compatible with real SQLAlchemy and equivalent
    /// mocks). Requires the `fitz` binary compiled with
    /// `--features python`.
    PyTypes {
        /// Python file with models to introspect
        source: PathBuf,
        /// Destination file. If omitted, writes to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// pyi-stubs (v0.9.39) — Generates Fitz `type` from a `.pyi`
    /// file (Python PEP 484/561 stubs). Parses top-level `class`es
    /// and emits the equivalent Fitz `type`s, ready to `import`.
    /// Does NOT require the `python` feature — the .pyi parser
    /// lives in the default binary. Useful to integrate typed
    /// Python libs with stubs (e.g. `requests.pyi`).
    PyStubs {
        /// `.pyi` file to parse
        source: PathBuf,
        /// Destination file. If omitted, writes to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Phase 9.y.1 — Creates a new Fitz project in a folder.
    ///
    /// Generates `<name>/fitz.toml`, `<name>/src/main.fitz`,
    /// `<name>/.gitignore`, and (unless `--no-git` is passed) runs
    /// `git init`. The name must match `^[a-z][a-z0-9_-]{0,63}$`.
    New {
        /// Project name (also the name of the folder to create).
        name: String,
        /// HTTP template instead of the CLI hello world.
        #[arg(long, conflicts_with = "template")]
        http: bool,
        /// Named starter template. Currently supported: `liveviews`.
        /// Overrides the built-in CLI/HTTP templates and scaffolds
        /// from a git repo instead. Per-template env var overrides
        /// (`FITZ_TEMPLATE_<NAME>_URL/SUBPATH/REF`) let tests and
        /// power users redirect the source.
        #[arg(long)]
        template: Option<String>,
        /// Don't run `git init` in the created folder.
        #[arg(long)]
        no_git: bool,
    },
    /// Phase 9.y.1 — Initializes a Fitz project in the current
    /// directory.
    ///
    /// Generates `./fitz.toml`, `./src/main.fitz`, `./.gitignore`,
    /// and (unless `--no-git` is passed) runs `git init`. The
    /// package name is derived from the current directory's name,
    /// or from `--name` if provided. Fails if a `fitz.toml` already
    /// exists.
    Init {
        /// Override the package name (default: current directory's
        /// name). Must match `^[a-z][a-z0-9_-]{0,63}$`.
        #[arg(long)]
        name: Option<String>,
        /// HTTP template instead of the CLI hello world.
        #[arg(long)]
        http: bool,
        /// Don't run `git init` in the directory.
        #[arg(long)]
        no_git: bool,
    },
    /// Phase 9.y.4 — Adds a dep to the current project's
    /// `fitz.toml` and syncs `fitz.lock`. Requires `--path` or
    /// `--git` (loose registry-style versions arrive in 9.y.5).
    ///
    /// If the dep already existed with the same name, it's
    /// overwritten (cargo-style). If subsequent resolution fails
    /// (missing path, failed git clone, etc.), the manifest
    /// persists anyway — use `fitz remove <name>` to revert.
    Add {
        /// Dep name as it will appear in `[dependencies]`.
        name: String,
        /// Path dep relative to the project's manifest.
        #[arg(long, conflicts_with = "git")]
        path: Option<String>,
        /// Git repo URL. Also requires `--tag` or `--rev`.
        #[arg(long, conflicts_with = "path")]
        git: Option<String>,
        /// Tag to check out (mutually exclusive with `--rev`).
        #[arg(long, conflicts_with = "rev", requires = "git")]
        tag: Option<String>,
        /// Commit SHA to check out (mutually exclusive with
        /// `--tag`).
        #[arg(long, conflicts_with = "tag", requires = "git")]
        rev: Option<String>,
    },
    /// Phase 9.y.4 — Removes a dep from the current project's
    /// `fitz.toml` and syncs `fitz.lock`. If the dep didn't exist,
    /// clear error.
    Remove {
        /// Name of the dep to remove (as it appears in
        /// `[dependencies]`).
        name: String,
    },
    /// Phase 9.y.4 — Re-resolves the current project's deps. For
    /// git deps, invalidates the local cache and re-clones (useful
    /// when the upstream tag moved or when you want a fresh
    /// fetch). For path deps it's a no-op (always fresh). Without
    /// args, updates all of them.
    Update {
        /// Name of the specific dep to update. Without this flag,
        /// updates all manifest deps.
        name: Option<String>,
    },
    /// Phase 9.z.1 (a + b CLOSED) — Formats Fitz code to its
    /// canonical style (zero config). 4-space indent, double
    /// quotes, trailing comma only on multi-line. **Preserves the
    /// user's comments and blank lines** (9.z.1.b).
    ///
    /// Without arguments, formats every `.fitz` in the current
    /// project (via manifest). With explicit files, formats only
    /// those. `--check` does not write; exit 1 on diffs.
    Fmt {
        /// `.fitz` files to format. If omitted, formats the entire
        /// project (requires `fitz.toml`).
        files: Vec<PathBuf>,
        /// CI mode: no writes, exit 1 on diffs.
        #[arg(long)]
        check: bool,
    },
    /// Phase 9.z.2.b — Runs every fn marked with `@test` in the
    /// project. In manifest mode, discovers from `[lib].entry`
    /// (or `[bin].main`) + top-level `tests/*.fitz` in the
    /// manifest directory. In single-file mode (`fitz test
    /// file.fitz`), loads that file and runs its `@test`s.
    /// Filters by substring of the test name if `[filter]` is
    /// passed. Exit code 0 if all pass, 1 if any fail.
    Test {
        /// Substring of the test name to filter by. With no
        /// filter, runs all discovered tests.
        filter: Option<String>,
        /// Specific `.fitz` file. If omitted, looks for
        /// `fitz.toml` (manifest mode) and discovers from the
        /// project.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Phase 9.z.3 — Development mode with hot reload. Runs your
    /// program and restarts it automatically when a `.fitz` file
    /// (or `fitz.toml`) changes. Without args, looks for
    /// `fitz.toml` and runs the `[bin].main`. With `--file`, runs
    /// that file (single-file mode).
    ///
    /// Strategy: kill+respawn the process (incremental rebuild is
    /// debt). Excludes `target/`, `.git/`, `node_modules/`, hidden
    /// files. 100 ms debounce to collapse multiple editor saves.
    /// Ctrl+C kills the child before exiting.
    ///
    /// Phase 11.13 — when the manifest's default bin targets
    /// `wasm-client`, `fitz dev` switches to wasm mode instead:
    /// builds the bundle (`wasm-pack --dev`), serves the project
    /// on `127.0.0.1:<port>` with a live-reload WebSocket, and
    /// rebuilds + reloads the browser on each `.fitzv`/`.fitz`/
    /// `fitz.toml` save. Editing `fitz.toml` (repoint entry / add
    /// a dep / edit flags) is re-resolved live; a broken manifest
    /// keeps serving the previous bundle.
    Dev {
        /// Specific `.fitz` file. If omitted, looks for
        /// `fitz.toml` (manifest mode) and runs `[bin].main`.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Phase 11.13 — Selects which `[[bin]]` to dev when the
        /// manifest declares more than one. A `wasm-client` bin
        /// runs the wasm dev server (build + serve + live-reload);
        /// a native bin runs the classic respawn. In a `server` +
        /// `web` fullstack project: `fitz run --bin server` in one
        /// terminal, `fitz dev --bin web` in another.
        #[arg(long = "bin", value_name = "NAME")]
        bin: Option<String>,
        /// Phase 11.13 — port for the wasm-client dev server.
        /// Ignored in the classic (native/SSR) respawn mode.
        #[arg(long, default_value_t = 1234)]
        port: u16,
    },
    /// Phase 9.z.4 — Interactive REPL. Opens a `fitz> ` prompt
    /// where you can enter expressions and statements line by
    /// line. The env persists between lines: `let x = 1` stays
    /// defined for subsequent inputs. Automatic multi-line
    /// (`... `) when `{` or `(` remain open. Persistent history
    /// at `~/.fitz/history`. Special commands: `:help`, `:quit`,
    /// `:type <expr>`, `:env`, `:reset`, `:load <file>`. Ctrl+D
    /// exits. Async works (`sleep(100).await` and similar).
    Repl,
    /// Phase 9.z.5 — Linter for patterns beyond types. Detects
    /// `unused_variable`, `unused_import`, `useless_match`,
    /// `string_concat`. Default: warnings (exit 0). `--deny
    /// <lint>` treats that lint as an error (exit 1). Suppression
    /// via `// @allow(<lint>)` on the previous line. With no
    /// args, looks for `fitz.toml` (manifest mode) and lints all
    /// `.fitz` files.
    Lint {
        /// `.fitz` files to lint. If omitted, lints the entire
        /// project (requires `fitz.toml`).
        files: Vec<PathBuf>,
        /// Treat the named lint as an error (exit 1 if it
        /// appears). Can be passed multiple times: `--deny
        /// unused_variable --deny string_concat`.
        #[arg(long)]
        deny: Vec<String>,
    },

    /// Phase 10.6 — Automatic ORM migrations. Compares the schema
    /// declared in the `@table` types with the real DB schema,
    /// generates SQL DDL to sync them, and tracks which
    /// migrations ran.
    #[command(subcommand)]
    Db(DbCmd),

    /// Phase 12.4 — Generates `Dockerfile`, `.dockerignore`, and
    /// `docker-compose.yml` for the current project. Detects the
    /// program shape (HTTP port, DB usage) by reading the AST of
    /// the manifest's entry point. Smart by default: if the
    /// program uses `db.connect(...)`, compose adds a
    /// `postgres:16-alpine` service with healthcheck.
    #[command(subcommand)]
    Docker(DockerCmd),

    /// Phase 12.6 — Deployment orchestrator. Thin wrapper over
    /// `docker build/push` or `docker compose up` depending on
    /// the selected target. MVP targets: `docker` (build + push)
    /// and `compose` (up local). Future targets like
    /// `fly`/`railway`/`k8s` remain visible debt — for those, run
    /// the CLIs directly.
    #[command(subcommand)]
    Deploy(DeployCmd),
}

#[derive(Subcommand)]
enum DeployCmd {
    /// Docker image build + push. `docker build -t <tag> .`
    /// followed by `docker push <tag>`. Default tag =
    /// `<pkg-name>:latest`. Aborts if there's no Dockerfile
    /// (suggests `fitz docker init`).
    Docker {
        /// Image tag. Default: `<package.name>:latest`.
        #[arg(long)]
        tag: Option<String>,
        /// Skip the `docker push` (local build only, useful with
        /// no registry).
        #[arg(long)]
        no_push: bool,
    },
    /// Brings the stack up with `docker compose up -d --build`.
    /// Aborts if there's no `docker-compose.yml` (suggests
    /// `fitz docker init`).
    Compose {
        /// Run in foreground (no `-d`).
        #[arg(long)]
        no_detach: bool,
        /// Don't rebuild images (no `--build`).
        #[arg(long)]
        no_build: bool,
    },
}

#[derive(Subcommand)]
enum DockerCmd {
    /// Generates the 3 files in the manifest directory. If a
    /// file already exists, skips it (unless `--force` is
    /// passed).
    Init {
        /// Overwrite existing files (Dockerfile, .dockerignore,
        /// docker-compose.yml). They are preserved by default.
        #[arg(long)]
        force: bool,
    },
    /// Thin wrapper over `docker build` that uses the Dockerfile
    /// in the manifest directory and tags the image with the
    /// `fitz.toml` `package.name` (override with `--tag`). Aborts
    /// if there's no Dockerfile (suggests running `fitz docker
    /// init` first).
    Build {
        /// Image tag. Default: `<package.name>:latest`.
        #[arg(long)]
        tag: Option<String>,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// Compares the DB's current schema with the one declared in
    /// the Fitz program's `@table` types and emits SQL DDL to
    /// sync them (CREATE TABLE / ADD COLUMN / etc.).
    Diff {
        /// Fitz program with the `@table` types. If omitted,
        /// uses the cwd/ancestor `fitz.toml`'s `[bin].main`.
        file: Option<PathBuf>,
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
        /// Output `.sql` file. If omitted, writes to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// v0.10.31 (Tier A.1) — classifies each change as
        /// safe/risky/destructive and aborts if there is any
        /// destructive change without `--allow-destructive`. The
        /// emitted SQL adds `-- [SAFE]` / `-- [RISKY]` /
        /// `-- [DESTRUCTIVE]` comments per change for human
        /// review before applying.
        ///
        /// Policy: DropTable/DropColumn → Destructive; AddColumn
        /// NOT NULL without default / AlterColumnType / SET NOT
        /// NULL / AlterColumnDefault / DropIndex → Risky; rest →
        /// Safe.
        #[arg(long)]
        check_destructive: bool,
        /// v0.10.31 (Tier A.1) — together with
        /// `--check-destructive`, allows the diff to emit even
        /// with destructive changes. Without this flag,
        /// `--check-destructive` aborts with exit 1 on any
        /// destructive change. Risky does NOT block (it's only
        /// reported).
        #[arg(long)]
        allow_destructive: bool,
    },
    /// Runs every pending migration in `migrations/` (or the
    /// custom `--dir`). Idempotent — skips those already applied
    /// according to `_fitz_migrations`.
    Migrate {
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
        /// Directory with `.sql` files. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Only print what would be applied, without touching
        /// the DB.
        #[arg(long)]
        dry_run: bool,
        /// v0.10.20 — Offline SQL mode: instead of running the
        /// pending migrations, emits them as concatenated SQL to
        /// stdout (1 file per migration with header
        /// `-- migration <version>: <filename>`). Useful for
        /// handing the SQL to a DBA who applies it manually.
        /// Still connects to read `_fitz_migrations` (what's
        /// applied) — if you don't want to connect, use
        /// `--dry-run`. Rejects `.fitz` migrations (they can't
        /// be materialized as offline SQL).
        #[arg(long)]
        sql: bool,
    },
    /// Lists the dir's migrations + state (applied/pending).
    Status {
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
        /// Directory with `.sql` files. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Creates an empty migration file with a timestamp prefix
    /// `YYYYMMDDHHMMSS_<name>.sql` in `migrations/`. v0.10.17 —
    /// the stub includes `-- UP` / `-- DOWN` sections by
    /// convention.
    New {
        /// Descriptive name for the migration (snake_case).
        name: String,
        /// Destination directory. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// v0.10.17 — Reverts the last N applied migrations by
    /// running their `-- DOWN` section inside a tx and removing
    /// the entry from `_fitz_migrations`. Default `N=1`. Aborts
    /// fast if any target migration lacks a `-- DOWN`.
    Rollback {
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
        /// Directory with `.sql` files. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Number of migrations to revert (most recent first).
        /// Default: 1.
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    /// v0.10.18 — Drift check: runs the diff and returns exit 0
    /// if the declared schema matches the DB, exit 1 with the
    /// pending SQL on stderr if there are differences. Hook for
    /// blocking CI ("no merge if the schema diverges").
    Check {
        /// Fitz program with the `@table` types. If omitted,
        /// uses the `fitz.toml`'s `[bin].main`.
        file: Option<PathBuf>,
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
    },
    /// v0.10.20 — Audit log: lists the applied migrations with
    /// `version` + `applied_at` + filename (if the file still
    /// exists in the dir). Order: most recent first.
    History {
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
        /// Directory with files. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// v0.10.20 — Combines the migrations in the `[from, to]`
    /// (inclusive) range into a single one. Concatenates the UPs
    /// in order + the DOWNs in reverse order. Moves the old
    /// files to `migrations/squashed/`. If any in the range was
    /// already applied in the DB, updates the tracking to point
    /// to the new squashed migration. Only `.sql` (rejects
    /// `.fitz`).
    Squash {
        /// Version of the first migration of the range
        /// (inclusive).
        from: String,
        /// Version of the last migration of the range
        /// (inclusive).
        to: String,
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env. Only needed to update the tracking; if you
        /// pass `--no-tracking`, it does not connect.
        #[arg(long)]
        url: Option<String>,
        /// Directory with files. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Skips updating the tracking in `_fitz_migrations`.
        /// Useful for repos without an accessible staging DB
        /// (CI-only).
        #[arg(long)]
        no_tracking: bool,
    },
    /// v0.10.28 (Tier S, sub-step 1) — Introspect the DB's real
    /// schema. Lists tables, columns (type + nullability +
    /// default), primary keys, indexes (with partial WHERE) and
    /// foreign keys. Without touching your Fitz program — pure
    /// introspection of the connected DB. Useful to audit
    /// before changing types, discover legacy tables, or
    /// compare two envs (dev vs prod).
    Inspect {
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
        /// Schema to introspect. Default: `public`. Tables in
        /// other schemas are filtered out.
        #[arg(long)]
        schema: Option<String>,
        /// Restrict to a single table. Default: lists every
        /// table in the schema. If the table doesn't exist in
        /// the schema, emits a clear message without error.
        #[arg(long)]
        table: Option<String>,
        /// Machine-readable JSON output with a locked shape
        /// (for external scripts). Default: readable plain text
        /// view.
        #[arg(long)]
        json: bool,
        /// v0.10.29 — List ALL user-defined schemas at once
        /// (not only the one filtered by `--schema`). Each
        /// schema appears with its own section. Mutually
        /// exclusive with `--schema`.
        #[arg(long, conflicts_with = "schema")]
        all_schemas: bool,
    },
    /// v0.10.18 — Marks a migration as applied WITHOUT running
    /// its SQL. Useful to adopt Fitz on a legacy DB where the
    /// schema is already applied manually. `--all` marks every
    /// pending migration in the dir as applied.
    Stamp {
        /// Version to stamp (filename's timestamp prefix, e.g.
        /// `20260530120000`). Mutually exclusive with `--all`.
        #[arg(conflicts_with = "all")]
        version: Option<String>,
        /// Marks every pending migration in the dir as applied.
        /// Mutually exclusive with `<version>`.
        #[arg(long)]
        all: bool,
        /// Postgres URL. If omitted, reads `DATABASE_URL` from
        /// the env.
        #[arg(long)]
        url: Option<String>,
        /// Directory with `.sql` files. Default: `./migrations`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

fn main() {
    // Phase 12.3.a.2 — Initializes the tracing subscriber (with
    // `EnvFilter::from_default_env()`) ONCE at binary boot. Default
    // level if `RUST_LOG` is unset = `info`. Without this the
    // `log.info/warn/error/debug` builtins would not honor the filter
    // and would always emit. Idempotent: if a global subscriber is
    // already installed (e.g. `cargo test` with tests that initialize
    // logging), it's a no-op.
    fitz::logging::init_logging();
    // Phase 12.3.c.1 — If `OTEL_EXPORTER_OTLP_ENDPOINT` is set,
    // installs the global OTel provider and the interpreter's HTTP
    // spans are exported to the backend
    // (Jaeger/Tempo/Honeycomb/Datadog/etc). Without that env var,
    // it's a silent no-op — handlers continue with local
    // instrumentation (stderr logs + in-memory metrics) without
    // sending anything over the wire.
    fitz::observability::init_otel();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            file,
            bin,
            no_typecheck,
            args,
        } => {
            let resolved = resolve_entry_with_bin(file, bin.as_deref(), None);
            sync_lockfile_if_needed(&resolved);
            let dep_registry = dep_registry_from(&resolved);
            run_file(&resolved.entry, no_typecheck, dep_registry, args);
        }
        Commands::Build {
            file,
            bin,
            target,
            bundle_python,
            bundle_pip,
            bundle_pip_requirements,
        } => {
            // Phase 11.5.b — parse --target early so we can pass it
            // to resolve_entry and reject unknown values with a
            // clean message before touching the manifest.
            let target_override = match target.as_deref() {
                Some(t) => match parse_target_flag(t) {
                    Ok(tg) => Some(tg),
                    Err(msg) => {
                        eprintln!("✗ {msg}");
                        std::process::exit(1);
                    }
                },
                None => None,
            };
            let resolved = resolve_entry_with_bin(file, bin.as_deref(), target_override);
            // Phase 11.5.b — surface reserved-target warnings once
            // per invocation (the CLI is a good place; the parser
            // itself must stay side-effect-free).
            emit_manifest_warnings(&resolved);
            // Phase 11.5.b — reject targets that the current codegen
            // does NOT know how to build yet, with the specific
            // sub-phase reference. Phase 11.5.c dispatches
            // `WasmClient` to `build_wasm_client_cmd` below; `Ssr`
            // still errors out with the 11.6+ pointer.
            let effective_target = resolved.effective_target();
            if let Err(msg) = enforce_build_target_supported(effective_target) {
                eprintln!("✗ {msg}");
                std::process::exit(1);
            }
            // Phase 11.5.c — WASM client target routes to a
            // separate orchestrator (view pipeline + wasm-crate
            // scaffold + `wasm-pack build`). Ends the process on
            // success/failure — no fallthrough to the native
            // path.
            if effective_target == manifest::Target::WasmClient {
                build_wasm_client_cmd(&resolved);
                return;
            }
            sync_lockfile_if_needed(&resolved);
            // In manifest mode, output goes to
            // `<manifest_dir>/target/release/<pkg-name>(.exe)`
            // (Cargo-style). In single-file mode, copying next to the
            // source is decided inside build_file.
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
            let flag_defaults = flag_defaults_from(&resolved);
            // Phase 8.c: --bundle-pip implies --bundle-python.
            // Phase 8.c (harvest): --bundle-pip-requirements does
            // too. Any of the three routes through the bundling
            // pipeline.
            if bundle_python || !bundle_pip.is_empty() || !bundle_pip_requirements.is_empty() {
                build_file_with_bundle(
                    &resolved.entry,
                    override_dest.as_deref(),
                    dep_registry,
                    flag_defaults,
                    bundle_pip,
                    bundle_pip_requirements,
                );
            } else {
                build_file(
                    &resolved.entry,
                    override_dest.as_deref(),
                    dep_registry,
                    flag_defaults,
                );
            }
        }
        Commands::Check { file } => {
            let no_file_arg = file.is_none();
            let resolved = resolve_entry(file);
            sync_lockfile_if_needed(&resolved);
            let dep_registry = dep_registry_from(&resolved);
            let mut clean = true;
            // Check the entry. A `.fitzv` entry routes through the view
            // pipeline (Phase 11, gotcha #7) resolving cross-file
            // `<Child />` imports dep-aware; a classic `.fitz` entry runs
            // the static checker (which does NOT recurse into imported
            // modules — real cross-module validation happens in
            // `fitz run`/`build`).
            if view::is_fitzv_extension(&resolved.entry) {
                clean &= check_view_file(&resolved.entry, &dep_registry);
            } else {
                clean &= check_file(&resolved.entry);
            }
            // a2 (v0.40.0) — with NO file arg in a manifest project, ALSO
            // view-check every OTHER `.fitzv` under `src/`, so a component
            // that isn't the `[bin].main` entry still gets checked (useful
            // for CI + view-component libraries). Classic `.fitz` files
            // are NOT swept: the checker doesn't resolve cross-module
            // imports, so a non-entry `.fitz` would report spurious
            // "unknown import" noise; `.fitzv` files are self-contained
            // (their imports resolve dep-aware in `check_view_source`).
            if no_file_arg {
                if let Some(ctx) = resolved.manifest_ctx.as_ref() {
                    let src_dir = ctx.manifest_dir.join("src");
                    let mut fitzv_files: Vec<PathBuf> = Vec::new();
                    if src_dir.is_dir() {
                        collect_fitzv_recursive(&src_dir, &mut fitzv_files);
                    }
                    fitzv_files.sort();
                    let entry_canon = fs::canonicalize(&resolved.entry).ok();
                    for f in fitzv_files {
                        // Skip the entry — already checked above.
                        if entry_canon.is_some() && fs::canonicalize(&f).ok() == entry_canon {
                            continue;
                        }
                        clean &= check_view_file(&f, &dep_registry);
                    }
                }
            }
            if !clean {
                std::process::exit(1);
            }
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
        Commands::New {
            name,
            http,
            template,
            no_git,
        } => {
            new_project(&name, http, template.as_deref(), no_git);
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
        Commands::Dev { file, bin, port } => {
            dev_cmd(file, bin, port);
        }
        Commands::Repl => {
            repl_cmd();
        }
        Commands::Lint { files, deny } => {
            lint_cmd(files, deny);
        }
        Commands::Db(sub) => match sub {
            DbCmd::Diff {
                file,
                url,
                out,
                check_destructive,
                allow_destructive,
            } => db_diff_cmd(file, url, out, check_destructive, allow_destructive),
            DbCmd::Migrate {
                url,
                dir,
                dry_run,
                sql,
            } => db_migrate_cmd(url, dir, dry_run, sql),
            DbCmd::Status { url, dir } => db_status_cmd(url, dir),
            DbCmd::New { name, dir } => db_new_cmd(name, dir),
            DbCmd::Rollback { url, dir, count } => db_rollback_cmd(url, dir, count),
            DbCmd::Check { file, url } => db_check_cmd(file, url),
            DbCmd::History { url, dir } => db_history_cmd(url, dir),
            DbCmd::Squash {
                from,
                to,
                url,
                dir,
                no_tracking,
            } => db_squash_cmd(from, to, url, dir, no_tracking),
            DbCmd::Stamp {
                version,
                all,
                url,
                dir,
            } => db_stamp_cmd(version, all, url, dir),
            DbCmd::Inspect {
                url,
                schema,
                table,
                json,
                all_schemas,
            } => db_inspect_cmd(url, schema, table, json, all_schemas),
        },
        Commands::Docker(sub) => match sub {
            DockerCmd::Init { force } => docker_init_cmd(force),
            DockerCmd::Build { tag } => docker_build_cmd(tag),
        },
        Commands::Deploy(sub) => match sub {
            DeployCmd::Docker { tag, no_push } => deploy_docker_cmd(tag, no_push),
            DeployCmd::Compose {
                no_detach,
                no_build,
            } => deploy_compose_cmd(no_detach, no_build),
        },
    }
}

// ---- Phase 9.y.2 — entry point resolution (single-file vs manifest) ----

/// Manifest context loaded during `resolve_entry`. When present, the
/// caller knows the run/build/check started from a Fitz project (not
/// single-file mode).
///
/// For now it's consumed by: `build_file` to decide where the binary
/// goes; `sync_lockfile_if_needed` to emit `fitz.lock`; and the
/// dispatch (Run/Build) to build the `dep_registry` that the
/// evaluator and codegen receive (9.y.3.b).
struct ManifestCtx {
    manifest: manifest::Manifest,
    manifest_dir: PathBuf,
    /// Resolved deps (path deps resolved to absolute `lib_entry`).
    /// Phase 9.y.3.a: populated in `resolve_entry`, consumed by
    /// `sync_lockfile_if_needed`. Phase 9.y.3.b: also used to build
    /// the `dep_registry` passed to the evaluator / codegen.
    resolved_deps: Vec<manifest::ResolvedDep>,
    /// Phase 11.5.b — Which `[[bin]]` was selected for this
    /// invocation. `None` when the manifest has no `[bin]` section
    /// (lib-only project); consumers that require a bin (`fitz
    /// run`/`build`) reject before this point.
    selected_bin: Option<manifest::ManifestBin>,
}

/// Result of resolving the command's entry point. `entry` points to
/// the `.fitz` to process; `manifest_ctx` is present when we got
/// here via `fitz.toml` (manifest mode). Phase 11.5.b:
/// `target_override` carries the `--target` flag (if any) so
/// `effective_target()` can compute what the codegen should emit
/// without re-walking the CLI.
struct ResolvedEntry {
    entry: PathBuf,
    manifest_ctx: Option<ManifestCtx>,
    /// Explicit `--target` override from the CLI. `None` means
    /// "use the selected bin's `target` field (or infer from
    /// extension in single-file mode)".
    target_override: Option<manifest::Target>,
}

impl ResolvedEntry {
    /// Computes what target the caller should build. Precedence:
    /// 1. `--target` CLI flag (if passed).
    /// 2. Selected bin's `target` field (manifest mode).
    /// 3. Extension-based inference (single-file mode): `.fitzv`
    ///    → `WasmClient`, everything else → `Native`.
    fn effective_target(&self) -> manifest::Target {
        if let Some(t) = self.target_override {
            return t;
        }
        if let Some(ctx) = &self.manifest_ctx {
            if let Some(b) = &ctx.selected_bin {
                return b.effective_target();
            }
        }
        // Single-file mode: infer from extension.
        let ext_view = self
            .entry
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("fitzv"))
            .unwrap_or(false);
        if ext_view {
            manifest::Target::WasmClient
        } else {
            manifest::Target::Native
        }
    }
}

/// Legacy entry point: resolves without honouring `--bin`/`--target`
/// (`fitz run`/`check`/etc). Multi-bin manifests error out here
/// (they only work through `fitz build --bin <name>` in 11.5.b MVP).
fn resolve_entry(file_opt: Option<PathBuf>) -> ResolvedEntry {
    resolve_entry_with_bin(file_opt, None, None)
}

/// Phase 11.5.b — resolves the subcommand's entry point with
/// optional bin selector and target override:
///
/// - If `file_opt.is_some()`, **single-file** mode: returns the path
///   as-is. `bin_selector` is ignored (single-file mode has no
///   manifest). `target_override` is honoured (extension inference
///   happens later via `ResolvedEntry::effective_target`).
/// - If `file_opt.is_none()`, **manifest** mode: searches for
///   `fitz.toml` walking up from the cwd (Cargo-style), parses it,
///   selects a bin via `Manifest::select_bin(bin_selector)`, and
///   returns `<manifest_dir>/<bin>.main` as the entry. Exits with a
///   clear message on: missing manifest, parse error, missing/
///   ambiguous bin, or unresolvable deps.
fn resolve_entry_with_bin(
    file_opt: Option<PathBuf>,
    bin_selector: Option<&str>,
    target_override: Option<manifest::Target>,
) -> ResolvedEntry {
    try_resolve_entry_with_bin(file_opt, bin_selector, target_override).unwrap_or_else(|e| {
        eprintln!("✗ {e}");
        std::process::exit(1);
    })
}

/// Phase 11.13 — `Result` core of [`resolve_entry_with_bin`]. Returns a
/// formatted error message (without the `✗ ` prefix) instead of
/// `exit(1)`, so the `fitz dev` wasm-client loop can re-resolve on a
/// `fitz.toml` save without the process dying on a transiently-broken
/// manifest (a mid-edit parse error). `resolve_entry_with_bin` wraps
/// this and exits on `Err`, preserving the exact behaviour + messages
/// of every other subcommand.
fn try_resolve_entry_with_bin(
    file_opt: Option<PathBuf>,
    bin_selector: Option<&str>,
    target_override: Option<manifest::Target>,
) -> Result<ResolvedEntry, String> {
    if let Some(entry) = file_opt {
        return Ok(ResolvedEntry {
            entry,
            manifest_ctx: None,
            target_override,
        });
    }

    let cwd = std::env::current_dir()
        .map_err(|e| format!("could not read the current directory: {e}"))?;

    let manifest_path = manifest::find_manifest(&cwd).ok_or_else(|| {
        format!(
            "could not find `{}` in `{}` or in parent directories.\n   \
             Pass an explicit file (`fitz <cmd> file.fitz`) or create a \
             project with `fitz new <name>` / `fitz init`.",
            manifest::MANIFEST_FILE,
            cwd.display()
        )
    })?;

    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("could not read `{}`: {e}", manifest_path.display()))?;

    let manifest = manifest::Manifest::parse(&manifest_text)
        .map_err(|e| format!("`{}`: {e}", manifest_path.display()))?;

    let manifest_dir = manifest_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.clone());

    // Phase 11.5.b — pick the right bin (or single-bin, or first,
    // depending on selector state). Errors are ManifestError with
    // formatted messages already prefixed by the caller.
    let selected_bin = match manifest.select_bin(bin_selector) {
        Ok(Some(b)) => b.clone(),
        Ok(None) => {
            return Err(format!(
                "`{}` has no `[bin]` section with a `main`. The package \
                 manager MVP (Phase 9.y) requires one. Add:\n\n[bin]\nmain = \"src/main.fitz\"\n",
                manifest_path.display()
            ));
        }
        Err(e) => return Err(e.to_string()),
    };

    let entry = manifest_dir.join(&selected_bin.main);

    // Phase 9.y.3.a — eager dep resolution (fail-fast with the
    // resolver's message). On errors, abort before touching the
    // lockfile or invoking the evaluator/codegen.
    let resolved_deps = manifest::resolve_dependencies(&manifest, &manifest_dir)
        .map_err(|e| format!("no se pudieron resolver las dependencias: {e}"))?;

    // Phase 12.8 — load feature flag defaults into the runtime
    // registry. Idempotent, called once per process. Without a
    // manifest (single-file mode), the registry stays empty and all
    // flags fall back to the `false` default unless an env var
    // overrides them. The BTreeMap→HashMap conversion is O(N) over
    // N declared flags (typically < 50). Re-running this on a live
    // `fitz dev` re-resolve is intentional: editing `[flags]` in
    // `fitz.toml` is picked up on the next build.
    let flag_defaults: std::collections::HashMap<String, bool> = manifest
        .flags
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    evaluator::set_flag_defaults(flag_defaults);

    Ok(ResolvedEntry {
        entry,
        manifest_ctx: Some(ManifestCtx {
            manifest,
            manifest_dir,
            resolved_deps,
            selected_bin: Some(selected_bin),
        }),
        target_override,
    })
}

/// Phase 11.5.b — parses the `--target <t>` CLI flag into
/// `manifest::Target`. Same vocabulary as the TOML field:
/// `native` / `wasm-client` / `ssr`. Returns a formatted user-facing
/// error message on unknown values.
fn parse_target_flag(raw: &str) -> Result<manifest::Target, String> {
    match raw {
        "native" => Ok(manifest::Target::Native),
        "wasm-client" => Ok(manifest::Target::WasmClient),
        "ssr" => Ok(manifest::Target::Ssr),
        other => Err(format!(
            "unknown `--target` value: `{other}`. Accepted: \
             `native`, `wasm-client`, `ssr`."
        )),
    }
}

/// Phase 11.5.b — emit reserved-target warnings to stderr. Called
/// from `fitz build` right after `resolve_entry_with_bin` so the
/// user sees the notice ONCE per invocation, before any long
/// operation kicks off.
fn emit_manifest_warnings(resolved: &ResolvedEntry) {
    let ctx = match &resolved.manifest_ctx {
        Some(c) => c,
        None => return,
    };
    for w in ctx.manifest.warnings() {
        eprintln!("⚠ notice: {w}");
    }
}

/// Phase 11.5.b + 11.5.c — validates that the requested target
/// is supported by the current codegen. `Native` and `WasmClient`
/// build; `Ssr` errors with the 11.6+ pointer. The `WasmClient`
/// path routes through `build_wasm_client_cmd` (Phase 11.5.c),
/// separate from the native `build_file` path.
fn enforce_build_target_supported(target: manifest::Target) -> Result<(), String> {
    match target {
        manifest::Target::Native | manifest::Target::WasmClient => Ok(()),
        manifest::Target::Ssr => Err("`target = \"ssr\"` — the server-side rendering emitter is \
             scheduled for Phase 11.6+. The manifest accepts the field \
             so you can declare intent today; the emitter itself has \
             not landed. Track progress at `docs/fase-11-plan.md` §9.p."
            .to_string()),
    }
}

/// Phase 11.5.c — build orchestrator for the `wasm-client`
/// target. Reads the selected `.fitzv` (or `.fitz` — MVP only
/// handles `.fitzv`), runs the view pipeline (parse → expand →
/// check → `emit_module`), composes the `#[wasm_bindgen(start)]`
/// wrapper, materialises the scaffold at
/// `<manifest_dir>/target/wasm-build/<bin_name>/`, shells out
/// to `wasm-pack build --release --target web` inside it, and
/// copies `pkg/` to `<manifest_dir>/target/wasm/<bin_name>/`.
///
/// Aborts the process on any failure (bad extension, view
/// pipeline error, wasm-pack missing, non-zero wasm-pack exit,
/// or filesystem error). Single-file mode (no manifest ctx) is
/// rejected here — the wasm-client path currently requires a
/// manifest because it needs `mount` from the `[[bin]]` entry
/// AND a fixed output layout.
fn build_wasm_client_cmd(resolved: &ResolvedEntry) {
    // Thin CLI wrapper: run the release build and print the
    // success/serve tip. Any error aborts the process (the classic
    // `fitz build` UX). The Result-returning core `build_wasm_client`
    // is shared with `fitz dev`'s wasm-client mode (Phase 11.13),
    // which must survive build errors instead of exiting.
    let out = match build_wasm_client(resolved, /*release=*/ true) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    println!("✓ wasm bundle at `{}`", out.pkg_dir.display());
    println!(
        "  serve it with: `python -m http.server` (or `fitz dev` for \
         live reload) and point `<script type=\"module\">` at \
         `{}/{}.js`",
        out.pkg_dir.display(),
        out.pkg_name
    );
}

/// Result of a successful wasm-client build — the paths the caller
/// (CLI wrapper or `fitz dev` server) needs to serve the bundle.
struct WasmBuildOutput {
    /// Final `pkg/` copy the browser consumes
    /// (`<manifest>/target/wasm/<bin>/`).
    pkg_dir: PathBuf,
    /// Sanitised package name (`<bin>.js` / `<bin>_bg.wasm` inside
    /// `pkg_dir`).
    pkg_name: String,
}

/// Phase 11.5.c core + Phase 11.13 refactor — builds the
/// wasm-client bundle and returns its layout. Runs the view
/// pipeline (parse → expand → check → `emit_module`), materialises
/// the scaffold at `<manifest_dir>/target/wasm-build/<bin>/`,
/// shells out to `wasm-pack build [--release|--dev] --target web`,
/// and copies `pkg/` to `<manifest_dir>/target/wasm/<bin>/`.
///
/// `release = true` uses `--release` (with `wasm-opt`, slower — the
/// `fitz build` default); `release = false` uses `--dev` (skips
/// `wasm-opt`, much faster — the `fitz dev` inner loop). The
/// scaffold dir is stable across calls, so cargo's incremental
/// cache survives between rebuilds.
///
/// Returns `Err(msg)` instead of exiting so `fitz dev` can print
/// the error and keep serving the previous bundle.
fn build_wasm_client(resolved: &ResolvedEntry, release: bool) -> Result<WasmBuildOutput, String> {
    let ctx = resolved.manifest_ctx.as_ref().ok_or_else(|| {
        "`--target wasm-client` requires a manifest with a `[[bin]]` entry (needs \
         `mount = \"<selector>\"`). Single-file mode is not yet supported — declare \
         the bin in `fitz.toml`."
            .to_string()
    })?;
    let bin = ctx
        .selected_bin
        .as_ref()
        .ok_or_else(|| "internal error: manifest ctx has no selected bin".to_string())?;

    // `mount` is REQUIRED by Manifest::parse cross-field
    // validation when target = WasmClient. This is safe.
    let mount = bin.mount.as_deref().ok_or_else(|| {
        format!(
            "internal error: `[[bin]] name = \"{}\"` has `target = \"wasm-client\"` but no \
             `mount` (cross-field validation should have caught this at parse time).",
            bin.name
        )
    })?;

    // Extension check — the emitter is `.fitzv`-only in Phase
    // 11.5.c. Classic `.fitz` composition lands with 11.5.d
    // (`<Child />` wire-up).
    let ext_view = resolved
        .entry
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("fitzv"))
        .unwrap_or(false);
    if !ext_view {
        return Err(format!(
            "`[[bin]] name = \"{}\"` targets `wasm-client` but its `main = \"{}\"` is not a \
             `.fitzv` file. Phase 11.5.c only wires the single-component wasm-client emit for \
             `.fitzv` sources; composing classic `.fitz` files as WASM roots is 11.5.d work.",
            bin.name, bin.main
        ));
    }

    // Load + run the view pipeline.
    let src_text = fs::read_to_string(&resolved.entry)
        .map_err(|e| format!("could not read `{}`: {e}", resolved.entry.display()))?;
    let raw = view::parse(&src_text)
        .map_err(|e| format!("view::parse on `{}`: {e}", resolved.entry.display()))?;
    let expanded = view::expand(&raw)
        .map_err(|e| format!("view::expand on `{}`: {e}", resolved.entry.display()))?;
    // Phase 11.7 — cross-file `<Child />`. Load the components declared
    // in imported sibling `.fitzv` files BEFORE the check so composition
    // of an imported child validates against its real surface (state /
    // events / slots) instead of being reported as an unknown component.
    // Resolved against the entry `.fitzv`'s directory. Empty when the
    // file composes no cross-file children.
    let base_dir = resolved
        .entry
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // CW.8 — the dep registry (`dep-name → lib_entry`) from the manifest, so
    // an import whose first path segment names a `fitz.toml` dependency
    // (`from fitz_liveviews.ui.Badge import Badge`) resolves through the dep
    // instead of only a flat sibling. Empty in single-file mode → the loaders
    // fall back to sibling-only resolution (byte-for-byte with the old path).
    let dep_registry = dep_registry_from(resolved);
    // Transitivity — walk the `.fitzv` import graph so a grandchild
    // component / nominal / helper `fn` that lives in a file the entry does
    // not import directly is still discovered. The three loaders below run
    // over this union (each de-dupes by resolved path). Byte-for-byte with the
    // one-level path when nothing is transitively reachable.
    let all_imports = view::collect_transitive_view_imports_with_deps(
        &expanded.imports,
        &base_dir,
        &dep_registry,
    );
    let imported_components =
        view::load_imported_components_with_deps(&all_imports, &base_dir, &dep_registry).map_err(
            |e| {
                format!(
                    "loading imported components for `{}`: {e}",
                    resolved.entry.display()
                )
            },
        )?;

    let check_errs =
        view::check_with_imported_components(&expanded, imported_components.components());
    if !check_errs.is_empty() {
        let mut msg = format!(
            "view::check on `{}` reported {} error(s):",
            resolved.entry.display(),
            check_errs.len()
        );
        for err in &check_errs {
            msg.push_str(&format!("\n  - {err}"));
        }
        return Err(msg);
    }

    // Phase 11.7 R3 — load the classic `type` defs imported by the
    // `.fitzv` (e.g. `from card import Card`) from their sibling
    // `.fitz` files so the emitter can synthesise a Rust `struct` for
    // each. Empty when the file imports no nominals (the pre-R3 examples).
    let nominals = view::load_imported_nominals_with_deps(&all_imports, &base_dir, &dep_registry)
        .map_err(|e| {
        format!(
            "loading imported nominals for `{}`: {e}",
            resolved.entry.display()
        )
    })?;

    // Phase 11.7 R3.5a.2 — load the sibling `.fitz` helper functions the
    // `.fitzv` imports (`from board_helpers import cards_in, ...`) so the
    // emitter can transpile each into the bundle.
    let imported_fns = view::load_imported_fns_with_deps(&all_imports, &base_dir, &dep_registry)
        .map_err(|e| {
            format!(
                "loading imported functions for `{}`: {e}",
                resolved.entry.display()
            )
        })?;

    // Materialise the scaffold. Path is Cargo-style —
    // `target/wasm-build/<bin>/` for the temporary crate,
    // `target/wasm/<bin>/` for the final `pkg/` copy that the
    // browser consumes.
    let sanitised_pkg = view::sanitise_wasm_pkg_name(&bin.name);
    let scaffold_dir = ctx
        .manifest_dir
        .join("target")
        .join("wasm-build")
        .join(&sanitised_pkg);
    let final_pkg_dir = ctx
        .manifest_dir
        .join("target")
        .join("wasm")
        .join(&sanitised_pkg);

    let source_label = bin.main.clone();
    let scaffold = view::write_wasm_crate_scaffold(
        &scaffold_dir,
        &expanded,
        &nominals,
        &imported_fns,
        &imported_components,
        &sanitised_pkg,
        mount,
        Some(&source_label),
        // Phase 11.13 slice-2 — dev profile (`fitz dev`) carries the
        // hot-reload state-preservation glue; `fitz build` (release) does not.
        /*dev_mode=*/
        !release,
    )
    .map_err(|e| format!("wasm-crate scaffold at `{}`: {e}", scaffold_dir.display()))?;

    // Shell out to `wasm-pack build [--release|--dev] --target web`.
    let profile_flag = if release { "--release" } else { "--dev" };
    println!("  running `wasm-pack build {profile_flag} --target web` ...");
    let status = std::process::Command::new("wasm-pack")
        .args(["build", profile_flag, "--target", "web"])
        .current_dir(&scaffold.crate_dir)
        .status()
        .map_err(|e| {
            format!(
                "could not invoke `wasm-pack`: {e}\n(install with `cargo install wasm-pack`, or \
                 grab the installer from https://rustwasm.github.io/wasm-pack/)"
            )
        })?;
    if !status.success() {
        return Err(format!(
            "`wasm-pack build {profile_flag} --target web` exited with {status} at `{}`. See the \
             wasm-pack output above.",
            scaffold.crate_dir.display()
        ));
    }

    // Copy `<scaffold>/pkg/` → `<manifest>/target/wasm/<bin>/`.
    // Overwrite whatever is there so re-builds are idempotent.
    let src_pkg = scaffold.crate_dir.join("pkg");
    if !src_pkg.is_dir() {
        return Err(format!(
            "wasm-pack succeeded but did not produce `pkg/` at `{}`",
            src_pkg.display()
        ));
    }
    copy_dir_tree_overwriting(&src_pkg, &final_pkg_dir).map_err(|e| {
        format!(
            "copying `{}` → `{}`: {e}",
            src_pkg.display(),
            final_pkg_dir.display()
        )
    })?;

    Ok(WasmBuildOutput {
        pkg_dir: final_pkg_dir,
        pkg_name: sanitised_pkg,
    })
}

/// Recursively copies `src` to `dst`, overwriting existing files
/// under `dst`. Creates `dst` if missing. Does NOT delete files
/// that exist under `dst` but not under `src` (rare in practice —
/// `wasm-pack` produces the same set of files every run).
fn copy_dir_tree_overwriting(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let dst_child = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_tree_overwriting(&entry.path(), &dst_child)?;
        } else {
            fs::copy(entry.path(), &dst_child)?;
        }
    }
    Ok(())
}

/// Phase 9.y.3.b — builds the `DepRegistry` (map `dep-name →
/// absolute-lib_entry`) consumed by `eval_with_base_and_deps_sync`
/// (`fitz run`) and `codegen::generate_project` (`fitz build`).
///
/// Returns an empty registry in single-file mode (no manifest) or
/// when the manifest has no `[dependencies]`. The loader treats
/// empty just like pre-9.y.3.b: only path-relative, no shortcuts.
fn dep_registry_from(resolved: &ResolvedEntry) -> manifest::DepRegistry {
    match &resolved.manifest_ctx {
        Some(ctx) => manifest::build_dep_registry(&ctx.resolved_deps),
        None => manifest::DepRegistry::new(),
    }
}

/// Phase 12.8 — returns the manifest's flag defaults (`[flags]`
/// section). Empty map in single-file mode or when the manifest
/// declares no flags. Codegen embeds them in the generated binary
/// at boot (parallel to `evaluator::set_flag_defaults` for `fitz
/// run`).
fn flag_defaults_from(resolved: &ResolvedEntry) -> std::collections::BTreeMap<String, bool> {
    match &resolved.manifest_ctx {
        Some(ctx) => ctx.manifest.flags.clone(),
        None => std::collections::BTreeMap::new(),
    }
}

/// Phase 9.y.3.a — syncs `fitz.lock` with the manifest's deps. No-op
/// in single-file mode and when the manifest has no deps.
///
/// For 9.y.3.a deps are only path deps (trivial deterministic
/// resolution). The lockfile is always regenerated;
/// `write_lockfile_if_changed` short-circuits byte-by-byte when the
/// contents match, to avoid mtime spam and empty diffs.
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
            // Only notify when we actually wrote something new.
            // Silent regeneration is the 90% case and doesn't
            // deserve spam.
            println!("✓ actualizado {}", path.display());
        }
        Ok(false) => {} // no changes
        Err(e) => {
            eprintln!("✗ no se pudo escribir `{}`: {e}", path.display());
            std::process::exit(1);
        }
    }
}

/// `fitz py-types <file.py> [--out <file.fitz>]` — Phase 8.5.
/// Imports the Python file via PyO3, introspects classes with
/// `__table__.columns` (compatible with real SQLAlchemy and mocks),
/// and generates the corresponding Fitz `type`. Writes to stdout or
/// to the given `--out`.
///
/// Without the `python` feature, emits a clear error citing the
/// build flag.
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
        "✗ `fitz py-types` requires recompiling `fitz` with Python interop enabled. \
         This binary was compiled without the `python` feature. \
         Rebuild with `cargo install --features python` (or \
         `cargo build --features python`)."
    );
    std::process::exit(1);
}

/// `fitz py-stubs <file.pyi> [--out <file.fitz>]` — pyi-stubs
/// (v0.9.39). Parses a `.pyi` file (PEP 484/561) and emits the
/// equivalent Fitz `type` for every top-level `class` in the stub.
/// Available without the `python` feature (the parser is ad-hoc,
/// doesn't use PyO3).
///
/// Output: a committable Fitz file the user can import normally
/// (`from <file> import User`). The checker then sees real types,
/// not opaque `PyAny`. Trade-off: loses automatic sync with the
/// .pyi (regenerate if the stub changes).
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

/// Converts the stub items to committable Fitz code. For now it
/// only emits `class` → `type` (top-level fns/vars in the stub
/// remain minor debt — the real `.py` exposes those via field
/// access with opaque `PyAny` today and that already works).
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
                // class with no fields → adds no useful info for
                // the checker.
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
            // Custom name — the user should have the type declared
            // alongside or will import it separately.
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
            // `T | None` → `T?`. Anything else → `Any`.
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

/// 8-pyi.B (v0.9.57) — Computes the `base_dir` for the adjacent
/// `.pyi` stub lookup. It's the parent directory of the `.fitz`
/// `path`; if the path has no clear parent (e.g. file in cwd
/// without a prefix), fallback to `current_dir()`.
fn base_dir_for_stub_lookup(path: &std::path::Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// 8-pyi.B (v0.9.57) — Wrapper of `check_program` that BEFORE
/// invoking the checker loads `.pyi` stubs adjacent to `path` and
/// registers their nominals in the TypeEnv. Silent-fallback policy:
/// if the `.pyi` doesn't exist or fails to parse, it carries on as
/// if it weren't there (the `from python import` binding stays as
/// opaque `Type::PyAny`).
///
/// **Why main.rs and not types.rs**: the checker doesn't know the
/// filesystem; only the caller that started the pipeline has the
/// `path`. We keep `types::check_program(program)` pure for call
/// sites without file context (tests, REPL, openapi of programs
/// without a known path).
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

/// W12 (v0.10.8) — variant of `check_program_with_pyi_stubs` that
/// also receives the `dep_registry` resolved from `fitz.toml` (when
/// `fitz check`/`build`/`run` runs inside a Fitz project with
/// `[dependencies]`). The cross-module `@auth_provider` pre-scan
/// uses it to resolve `from <dep-name> import` to the dep's
/// absolute `lib_entry` instead of falling back to a path relative
/// to the importer.
///
/// **Coordinated steps** (order matters):
/// 1. `pyi_loader::load_stubs` — pre-populates the TypeEnv with
///    nominals declared in adjacent `.pyi` files (later call sites
///    can reference the stub's `User`).
/// 2. `resolve_program_with_env` — registers the `.fitz`'s local
///    nominals + nominals brought in by `from <mod> import <T>`
///    (pass 1b) in the importer's TypeEnv.
/// 3. `pyi_loader::load_callables` — pre-populates the stubs'
///    callables (depends on types resolved in step 2 for the
///    returns).
/// 4. **`pre_scan_imported_auth_provider`** — scans each
///    `Stmt::Import` / `Stmt::FromImport`, resolves the .fitz file,
///    parses it, extracts `@auth_provider` with
///    `types::extract_auth_provider_signature`. The first one to
///    appear wins (caller order, not import order). If there's a
///    provider, it's registered in the TypeEnv with
///    `set_imported_auth_provider`. Module read/parse errors are
///    silenced — the real runtime/codegen loader will report theirs
///    when it actually runs.
/// 5. `check_with_env` — the checker runs with the enriched env;
///    `collect_auth_provider` falls back to the imported provider
///    when it doesn't find a local one.
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
    // W12 — cross-module `@auth_provider` pre-scan.
    if let Some(provider) = pre_scan_imported_auth_provider(program, &base_dir, dep_registry) {
        env.set_imported_auth_provider(provider);
    }
    // B10 — cross-module `@background` fns pre-scan. Adds the names
    // collected across imports so the importer's `spawn(<imp>(...))`
    // passes the checker's `@background` validation.
    let bg_names = pre_scan_imported_background_fns(program, &base_dir, dep_registry);
    if !bg_names.is_empty() {
        env.add_imported_background_fns(bg_names);
    }
    // Phase 11.6.e continuation (§9.bb, 2026-07-16) — cross-module
    // `@live_component` pre-scan. Extracts component metadata
    // (state type + render_fn + events) from each imported module
    // so `types::inject_live_component_registrations` can synthesise
    // `flv_register(...)` calls for components declared in sibling
    // `.fitzv`/`.fitz` modules — removes the manual boot boilerplate
    // that Phase 4 (v0.20.1) required. Paralelo bit-a-bit a
    // `pre_scan_imported_background_fns` (B10) y
    // `pre_scan_imported_auth_provider` (W12).
    let live_comps = pre_scan_imported_live_components(program, &base_dir, dep_registry);
    if !live_comps.is_empty() {
        env.add_imported_live_components(live_comps);
    }
    types::check_with_env(program, env, errors)
}

/// W12 (v0.10.8) — Scans the importer's `Stmt::Import` /
/// `Stmt::FromImport` statements. For every imported module,
/// resolves its `.fitz` file, reads + lexes + parses it, and
/// invokes `types::extract_auth_provider_signature` on the AST.
/// Returns the first provider found (following the top-down order
/// of the imports in the file).
///
/// **Error policy**: module read/parse errors are silenced (silent
/// fallback — parallel to `pyi_loader::load_stubs`'s policy). The
/// real runtime loader (`evaluator::eval_with_base_and_deps`) and
/// codegen (`ModuleLoader` in `codegen.rs`) load modules on their
/// own and report clear errors if they fail. We do NOT want
/// double-reporting here — the goal is to enrich the TypeEnv with
/// static info, not to validate imports.
///
/// **MVP scope**: a single level of depth. The importer sees
/// `@auth_provider` from its direct imports; it does not recurse
/// into transitive ones. Typical case covered: `main.fitz` imports
/// `auth.fitz`+`posts.fitz` (provider in `auth.fitz`) — works.
/// Case outside the MVP: `posts.fitz` imports `lib.fitz`, which in
/// turn imports the provider from `auth.fitz` — would require
/// recursion. If pressure appears, extend in a future sub-step.
fn pre_scan_imported_auth_provider(
    program: &ast::Program,
    base_dir: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Option<types::ImportedAuthProvider> {
    for stmt in program {
        let (path_segments, module_binding_name) = match stmt {
            ast::Stmt::Import { path, alias, .. } => {
                // Skip Python imports — they have no .fitz file on
                // disk.
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
                // The "module_binding_name" for `from auth import
                // User` is `auth` (the last segment of the path).
                // Codegen uses it as the mod name in the generated
                // crate.
                let binding = path.last().cloned().unwrap_or_default();
                (path.clone(), binding)
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path(&path_segments, base_dir, dep_registry)
        else {
            continue;
        };
        let Ok(source_raw) = fs::read_to_string(&file_path) else {
            continue;
        };
        // Phase 11.6.d — `.fitzv` transparent handling in the
        // pre-scan path. `fitz::view::is_fitzv_extension` matches
        // the same rule as the runtime + codegen loaders; a view
        // transform failure silences the pre-scan (same policy
        // as read/lex failures — the main loader will report
        // the error).
        let source = if fitz::view::is_fitzv_extension(&file_path) {
            match fitz::view::transform_fitzv_source(&source_raw, &file_path) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            source_raw
        };
        let Ok(tokens) = lexer::tokenize(&source) else {
            continue;
        };
        let Ok(module_program) = parser::parse(tokens) else {
            continue;
        };
        if let Some(mut provider) =
            types::extract_auth_provider_signature(&module_program, &module_binding_name)
        {
            // v0.37.4 — the provider's `User` type may be imported into the
            // provider's own module (`auth.fitz` does `from models import
            // User`). Follow the provider module's imports to resolve the
            // `role: Str` field so `@admin`/`@requires` don't wrongly fail
            // the checker in single-file mode. Parallel to the codegen +
            // LSP fixes (`pre_scan_imported_auth_provider_for_loader`,
            // `pre_scan_imported_auth_provider_lsp`).
            if !provider.has_role_field {
                let provider_base = file_path.parent().unwrap_or(base_dir);
                if role_field_across_module_imports_main(
                    &module_program,
                    &provider.user_type_name,
                    provider_base,
                    dep_registry,
                ) {
                    provider.has_role_field = true;
                }
            }
            return Some(provider);
        }
    }
    None
}

/// v0.37.4 — follows a module's DIRECT imports to find a sibling that
/// declares `type <type_name> { ... role: Str }`. Used to resolve
/// `has_role_field` for an `@auth_provider` whose `User` type is imported
/// into the provider's own module. Checker-path variant of the codegen
/// `resolve_role_field_across_module_imports`. Only direct imports — Fitz
/// has no re-export, so the provider's `User` is always a direct import.
fn role_field_across_module_imports_main(
    module_program: &ast::Program,
    type_name: &str,
    base_dir: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> bool {
    for stmt in module_program {
        let path_segments = match stmt {
            ast::Stmt::Import { path, .. } | ast::Stmt::FromImport { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path(&path_segments, base_dir, dep_registry)
        else {
            continue;
        };
        let Ok(source_raw) = fs::read_to_string(&file_path) else {
            continue;
        };
        let source = if fitz::view::is_fitzv_extension(&file_path) {
            match fitz::view::transform_fitzv_source(&source_raw, &file_path) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            source_raw
        };
        let Ok(tokens) = lexer::tokenize(&source) else {
            continue;
        };
        let Ok(imported_program) = parser::parse(tokens) else {
            continue;
        };
        if types::type_decl_has_role_field(&imported_program, type_name) {
            return true;
        }
    }
    false
}

/// B10 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) — scans each
/// `Stmt::Import` / `Stmt::FromImport`, resolves the `.fitz` file,
/// parses it, and invokes `types::extract_background_fn_names` to
/// collect the names of top-level fns marked with `@background`
/// across all direct imports. The importer's checker merges them
/// into `CheckCtx.background_fns` so `spawn(<imp>(args))` passes
/// the static validation.
///
/// **Error policy**: module read/parse errors are silenced (silent
/// fallback — parallel to `pre_scan_imported_auth_provider`). The
/// real runtime loader (`evaluator::eval_with_base_and_deps`) and
/// codegen (`ModuleLoader` in `codegen.rs`) load modules on their
/// own and report clear errors if they fail.
///
/// **MVP scope**: a single level of depth (does not recurse into
/// transitive imports), paralelo a `pre_scan_imported_auth_provider`.
fn pre_scan_imported_background_fns(
    program: &ast::Program,
    base_dir: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for stmt in program {
        let path_segments = match stmt {
            ast::Stmt::Import { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            ast::Stmt::FromImport { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path(&path_segments, base_dir, dep_registry)
        else {
            continue;
        };
        let Ok(source_raw) = fs::read_to_string(&file_path) else {
            continue;
        };
        // Phase 11.6.d — `.fitzv` transparent handling in the
        // pre-scan path. `fitz::view::is_fitzv_extension` matches
        // the same rule as the runtime + codegen loaders; a view
        // transform failure silences the pre-scan (same policy
        // as read/lex failures — the main loader will report
        // the error).
        let source = if fitz::view::is_fitzv_extension(&file_path) {
            match fitz::view::transform_fitzv_source(&source_raw, &file_path) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            source_raw
        };
        let Ok(tokens) = lexer::tokenize(&source) else {
            continue;
        };
        let Ok(module_program) = parser::parse(tokens) else {
            continue;
        };
        out.extend(types::extract_background_fn_names(&module_program));
    }
    out
}

/// Phase 11.6.e continuation (§9.bb, 2026-07-16) — Scans each
/// `Stmt::Import`/`Stmt::FromImport`, resolves the imported module
/// (`.fitz` first, `.fitzv` fallback — parallel to
/// `pre_scan_imported_background_fns`), parses it, transforms
/// `.fitzv` via `crate::view::transform_fitzv_source`, and invokes
/// `types::extract_live_components_from_program` to collect
/// `ImportedLiveComponent` entries. Populates the importer's
/// `TypeEnv.imported_live_components` so
/// `inject_live_component_registrations` can synthesise
/// `flv_register(...)` calls for cross-module `@live_component`
/// types.
///
/// **Error policy**: module read/parse/view-transform errors are
/// silenced (silent fallback — parallel to
/// `pre_scan_imported_background_fns`). The real runtime/codegen
/// loader reports its own errors when it actually loads the module.
///
/// **Module name**: derived from the last segment of the import
/// path (`from Counter import ...` → `"Counter"`;
/// `import sub.foo` → `"foo"`). Used by the injector in the
/// missing-imports error message so the fix is actionable.
///
/// **MVP scope**: single level of depth (does not recurse into
/// transitive imports), paralelo a
/// `pre_scan_imported_background_fns` (B10) y
/// `pre_scan_imported_auth_provider` (W12).
fn pre_scan_imported_live_components(
    program: &ast::Program,
    base_dir: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Vec<types::ImportedLiveComponent> {
    let mut out: Vec<types::ImportedLiveComponent> = Vec::new();
    for stmt in program {
        let path_segments = match stmt {
            ast::Stmt::Import { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            ast::Stmt::FromImport { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path(&path_segments, base_dir, dep_registry)
        else {
            continue;
        };
        let Ok(source_raw) = fs::read_to_string(&file_path) else {
            continue;
        };
        // Phase 11.6.d — `.fitzv` transparent handling; a view
        // transform failure silences the pre-scan (paralelo a
        // `pre_scan_imported_background_fns`).
        let source = if fitz::view::is_fitzv_extension(&file_path) {
            match fitz::view::transform_fitzv_source(&source_raw, &file_path) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            source_raw
        };
        let Ok(tokens) = lexer::tokenize(&source) else {
            continue;
        };
        let Ok(module_program) = parser::parse(tokens) else {
            continue;
        };
        // Module binding name: last segment of the import path.
        // Used in the missing-imports error message so the fix is
        // actionable (`from <module> import ...`).
        let module_name = path_segments
            .last()
            .cloned()
            .unwrap_or_else(|| String::from("<module>"));
        out.extend(types::extract_live_components_from_program(
            &module_program,
            &module_name,
        ));
    }
    out
}

/// W12 (v0.10.8) — resolves the `path` of a `Stmt::Import` /
/// `Stmt::FromImport` to the corresponding `.fitz` file. Light
/// mirror of `evaluator::resolve_module_path`:
/// 1. Single-segment matches a key of the `dep_registry` → dep's
///    `lib_entry`.
/// 2. Fallback to a path relative to `base_dir`: `["foo"]` →
///    `<base>/foo.fitz`; `["sub", "foo"]` → `<base>/sub/foo.fitz`.
///
/// Returns `None` if the file doesn't exist (we check with
/// `exists()` so the silent fallback works without throwing
/// spurious warnings).
fn resolve_import_file_path(
    segments: &[String],
    base_dir: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Option<PathBuf> {
    if let Some(lib_entry) = dep_registry.get(&segments[0]) {
        if segments.len() == 1 {
            if lib_entry.exists() {
                return Some(lib_entry.clone());
            }
        } else {
            // Dotted sub-path into the dep (`from dep.sub.Mod import X`).
            return fitz::view::resolve_dep_subpath_file(lib_entry, &segments[1..]);
        }
    }
    let mut dir = base_dir.to_path_buf();
    for seg in &segments[..segments.len().saturating_sub(1)] {
        dir.push(seg);
    }
    // Phase 11.6.d — try `.fitz` first, `.fitzv` as fallback.
    let last = segments.last()?;
    fitz::view::resolve_module_file_candidates(&dir, last)
}

/// `fitz openapi <file>` — Phase 7.1. Lex + parse + check + eval
/// with an active `HttpRegistry` so HTTP decorators register their
/// routes; then dumps the OpenAPI 3.1 schema to stdout
/// (pretty-printed).
///
/// Does not start the server: the registry is populated during
/// `eval` (HTTP decorators are top-level side effects) and the
/// schema can be derived from there + the AST.
///
/// Useful for CI, generating SDKs with openapi-generator, snapshot
/// testing the contract.
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

    // Strict checker: it makes no sense to emit a schema for a
    // program with type errors (the handler might not even type).
    // Same criterion as `fitz build`. 8-pyi.B: loads adjacent .pyi
    // stubs.
    let (_env, _types, _defs, type_errors) = check_program_with_pyi_stubs(&program, path);
    if !type_errors.is_empty() {
        eprintln!(
            "✗ {} — {} type error(s):",
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

/// `fitz check <file>` — runs lexer + parser + static checker and
/// reports errors. Returns `true` when the file is clean, `false` when
/// it has errors of any kind (read/lex/parse/type). The caller owns the
/// exit code so `fitz check` (no arg) can aggregate several files (a2).
fn check_file(path: &PathBuf) -> bool {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error leyendo {}: {}", path.display(), e);
            return false;
        }
    };
    let tokens = match lexer::tokenize(&source) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return false;
        }
    };
    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            return false;
        }
    };
    // 8-pyi.B: loads adjacent .pyi stubs before the check.
    let (_env, _types, _defs, errors) = check_program_with_pyi_stubs(&program, path);
    if errors.is_empty() {
        println!("✓ {} — no type errors", path.display());
        true
    } else {
        eprintln!("✗ {} — {} type error(s):", path.display(), errors.len());
        for e in &errors {
            eprintln!("  {}", e);
        }
        false
    }
}

/// `fitz check <file.fitzv>` — Phase 11 (gotcha #7). Runs the view
/// pipeline (parse → expand → type-check) over a single-file
/// component and reports view errors with the SAME exit-code contract
/// as [`check_file`] (0 = clean, 1 = errors) so CI greps and existing
/// checks keep working. Cross-file `<Child />` imports resolve
/// dep-aware (via `dep_registry`), matching the
/// `fitz build --target wasm-client` path so a `check` failure
/// predicts a build failure.
///
/// Returns `true` when the file is clean, `false` on any error, so the
/// caller can aggregate several files (a2) and own the exit code.
fn check_view_file(path: &Path, dep_registry: &manifest::DepRegistry) -> bool {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error leyendo {}: {}", path.display(), e);
            return false;
        }
    };
    let base_dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let errors = view::check_view_source(&source, &base_dir, dep_registry);
    if errors.is_empty() {
        println!("✓ {} — no type errors", path.display());
        true
    } else {
        eprintln!("✗ {} — {} view error(s):", path.display(), errors.len());
        for e in &errors {
            eprintln!("  {}", e);
        }
        false
    }
}

/// a2 (v0.40.0) — collect every `.fitzv` file under `dir` (recursively),
/// skipping hidden dirs and `target/`. Parallel to
/// [`collect_fitz_recursive`] (which collects `.fitz`). Used by
/// `fitz check` with no file arg to view-check every component in the
/// project, not only the `[bin].main` entry.
fn collect_fitzv_recursive(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
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
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_fitzv_recursive(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("fitzv") {
            out.push(path);
        }
    }
}

/// `fitz build <file>` — Phase 5b. Compiles the .fitz to a native
/// binary. Flow: lex → parse → checker (strict) → codegen to a
/// Cargo project → `cargo build --release` → copy the binary.
///
/// Binary destination:
/// - **Single-file mode** (`override_dest = None`): next to the
///   `.fitz` with the original stem (`hello.fitz` → `hello.exe`).
///   Pre-9.y.2 behavior.
/// - **Manifest mode** (`override_dest = Some(p)`): the caller
///   provides the full destination path (typically
///   `<manifest_dir>/target/release/<pkg-name>(.exe)`). Comes from
///   the dispatch in `main()` when the user runs `fitz build`
///   without args and there's a `fitz.toml`.
///
/// Copy the freshly-built binary to its final destination, retrying
/// on Windows `ERROR_SHARING_VIOLATION` (os error 32).
///
/// On Windows the linker/antivirus/Search indexer can keep a file
/// handle open on the just-linked `target/release/<stem>.exe` for a
/// brief moment after the build completes, and a previous run's exe
/// that is still exiting can hold the destination. A bare `fs::copy`
/// then fails with os error 32, which surfaced as a flaky-test family
/// (`hidden_decorator`, `handler_panic_r6`) once the serializing
/// `SERIAL` mutex was removed (T2, v0.10.13) and E2E builds began
/// running fully in parallel. A short retry-with-backoff eliminates
/// the race without serializing anything. No-op on the happy path
/// (the first attempt succeeds).
fn copy_binary_with_retry(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    // os error 32 = ERROR_SHARING_VIOLATION on Windows: the AV /
    // indexer / linker can hold a handle on the freshly-linked `.exe`
    // for a moment after the build finishes. On other platforms this
    // code never matches, so the loop returns the first result.
    //
    // Retry window widened (2026-08-07) from 8 attempts × linear 25ms
    // (~700ms) to 20 attempts × exponential-capped backoff (50→400ms,
    // ~6.7s worst case) because ~700ms wasn't enough on machines with
    // an aggressive real-time AV — it left `compile_e2e` tests flaky
    // (`os error 32`) even with `--test-threads=1`. The sleeps only
    // happen on a SHARING_VIOLATION retry; the happy path is instant.
    const SHARING_VIOLATION: i32 = 32;
    const MAX_ATTEMPTS: u32 = 20;

    let mut attempt = 0;
    loop {
        attempt += 1;
        match fs::copy(src, dst) {
            Ok(_) => return Ok(()),
            Err(e) if attempt < MAX_ATTEMPTS && e.raw_os_error() == Some(SHARING_VIOLATION) => {
                // Exponential backoff capped at 400ms: 50, 100, 200,
                // 400, 400, ... (the exponent is clamped to 3).
                let backoff_ms = 50u64 * 2u64.pow((attempt - 1).min(3));
                std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Since 5b.5 we generate a Cargo project instead of invoking
/// rustc directly. Reasons: (a) cross-file imports need multiple
/// `.rs` with `mod`, which is native to cargo; (b) when 5b.6
/// arrives with HTTP, we add `axum`/`tokio`/`serde_json` to the
/// generated `Cargo.toml` without rewriting the pipeline; (c)
/// cargo caches incrementally, which cheapens the second
/// compile. Trade-off: the first compile costs ~1-2s more than
/// `rustc` directly.
fn build_file(
    path: &PathBuf,
    override_dest: Option<&std::path::Path>,
    dep_registry: manifest::DepRegistry,
    flag_defaults: std::collections::BTreeMap<String, bool>,
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

    // Checker in strict mode — there is no `--no-typecheck` in
    // build. 8-pyi.B: loads adjacent .pyi stubs before the check.
    let (env, types, _defs, type_errors) =
        check_program_with_pyi_stubs_and_deps(&program, path, &dep_registry);
    if !type_errors.is_empty() {
        eprintln!(
            "✗ {} — {} type error(s):",
            path.display(),
            type_errors.len()
        );
        for e in &type_errors {
            eprintln!("  {}", e);
        }
        eprintln!("   Use `fitz check` to review before building.");
        std::process::exit(1);
    }

    // Mini-batch P2 — 5b.1/Hpx.2 chained fix. If there are fns with
    // unannotated params (5b.1) AND an unannotated return type
    // (Hpx.2), the checker's first pass types the body assuming
    // params are Any, and Hpx.2 fails because the body returns Any.
    // Strategy: infer params via call sites
    // (codegen::infer_param_type_from_call_sites) and mutate the
    // AST in place filling Param.type_, then re-run the checker to
    // refine TypeInfo. Extra cost: ~1 check pass for programs with
    // unannotated fns; free for annotated programs.
    let (env, types) = if codegen::has_unannotated_fn_params(&program) {
        codegen::fill_inferred_param_types(&mut program, &types, &env);
        // 8-pyi.B: re-check also loads stubs (idempotent).
        let (env2, types2, _defs2, errs2) =
            check_program_with_pyi_stubs_and_deps(&program, path, &dep_registry);
        if !errs2.is_empty() {
            // If the re-check produces new errors with the
            // inferred types, surface them.
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

    // Phase 5 (fitz-liveviews) — auto-inject `flv_register(...)` for
    // every `@live_component` type. Mutates `program` in place so
    // codegen sees the synthetic calls. Errors abort the build the
    // same way the checker does.
    if let Err(inject_errs) = fitz::types::inject_live_component_registrations(&mut program, &env) {
        eprintln!(
            "✗ {} — {} error(s) at implicit @live_component registration:",
            path.display(),
            inject_errs.len()
        );
        for e in &inject_errs {
            eprintln!("  {}", e);
        }
        std::process::exit(1);
    }

    // Codegen to a Cargo project. Mini-batch Hpx.2 — the checker's
    // TypeInfo is passed to codegen to infer return types of
    // unannotated fns. Phase 12.8 — manifest's flag_defaults are
    // embedded in the generated main.rs via `__fitz_flag_init(...)`
    // at boot.
    let project = match codegen::generate_project(
        path,
        &program,
        &env,
        &types,
        dep_registry,
        flag_defaults,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ codegen: {}", e);
            eprintln!("   (Fase 5b soporta un subset progresivo; los mensajes citan el sub-paso correspondiente.)");
            std::process::exit(1);
        }
    };

    // Cargo project layout: target/fitz-build/<stem>/{Cargo.toml, src/...}.
    let build_dir = PathBuf::from("target")
        .join("fitz-build")
        .join(&project.bin_name);
    let src_dir = build_dir.join("src");
    if let Err(e) = fs::create_dir_all(&src_dir) {
        eprintln!("Error creando {}: {}", src_dir.display(), e);
        std::process::exit(1);
    }

    // Write Cargo.toml.
    let cargo_toml_path = build_dir.join("Cargo.toml");
    if let Err(e) = fs::write(&cargo_toml_path, &project.cargo_toml) {
        eprintln!("Error escribiendo {}: {}", cargo_toml_path.display(), e);
        std::process::exit(1);
    }

    // Write src/main.rs.
    let main_rs_path = src_dir.join("main.rs");
    if let Err(e) = fs::write(&main_rs_path, &project.main_rs) {
        eprintln!("Error escribiendo {}: {}", main_rs_path.display(), e);
        std::process::exit(1);
    }

    // Write each mod file (5b.5+).
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

    // Invoke `cargo build --release`. We work against the generated
    // project's manifest; the target dir is inherited (cargo
    // decides).
    let output = std::process::Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&cargo_toml_path)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error invoking cargo: {}", e);
            eprintln!("   Is cargo on your PATH? (`rustup` provides it.)");
            std::process::exit(1);
        }
    };

    if !output.status.success() {
        eprintln!("✗ cargo build failed to compile the generated code:");
        eprintln!(
            "   (check {} to see what was attempted.)",
            src_dir.display()
        );
        eprintln!("--- cargo stderr ---");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }

    // Binary in target/release/<bin_name>; copy next to the .fitz
    // with `output_basename` (= original stem of the .fitz, not
    // sanitized). If the user builds `02-hola.fitz`, the final
    // file is `02-hola.exe` even though the crate inside Cargo is
    // called `fitz_02-hola`.
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

    // Destination: manifest override (9.y.2) or next to the source.
    let bin_out = match override_dest {
        Some(p) => p.to_path_buf(),
        None => path
            .parent()
            .map(|p| p.join(&output_filename))
            .unwrap_or_else(|| PathBuf::from(&output_filename)),
    };

    // Create the destination directory if needed (manifest mode:
    // the first build of a freshly created project doesn't have
    // target/release/ yet).
    if let Some(parent) = bin_out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("Error creando {}: {}", parent.display(), e);
                std::process::exit(1);
            }
        }
    }

    if let Err(e) = copy_binary_with_retry(&release_bin_path, &bin_out) {
        eprintln!(
            "Error copying {} to {}: {}",
            release_bin_path.display(),
            bin_out.display(),
            e
        );
        std::process::exit(1);
    }

    println!("✓ binario: {}", bin_out.display());
}

/// Phase 8.b — Variant of `build_file` that produces a standalone
/// binary.
///
/// Deterministic hash of the pip install inputs (positionals
/// `--bundle-pip` + bytes of the `--bundle-pip-requirements`).
///
/// Used as the cache key of the pip_packages tarball to avoid
/// re-running pip install on every `fitz build` when the packages
/// didn't change.
///
/// The sidecar `<bin>_pip_packages.inputs_hash` next to the
/// tarball stores the result; on the next build, if it matches
/// the current hash, the existing tarball is reused.
///
/// Hash rules:
/// - `--bundle-pip` positionals are sorted alphabetically
///   (reordering args must not invalidate the cache).
/// - Bytes of each requirements file in CLI order (reordering
///   files DOES invalidate; pip processes them in order and may
///   give different results with the same set of packages).
/// - `\n---\n` separator between the two sections so that
///   `["foo", "bar"]` positionals ≠ `"foo\nbar"` in requirements.
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

/// Builds with embedded CPython. The output is a single file that
/// internally carries: PBS tarball (install_only_stripped 3.14.x)
/// plus the real binary plus a standalone Rust launcher. The first
/// run extracts to `$TMPDIR/fitz-py-<hash>/`, sets PYTHONHOME and
/// LD_LIBRARY_PATH/DYLD/PATH depending on the OS, then execs the
/// real binary. Subsequent runs are instant (TMP cache).
///
/// Early validations: host triple supported by PBS, program uses
/// `from python import` (without interop there's no point in
/// bundling).
///
/// Known Linux constraint (debt R.bug-pyo3-abi3-portable-link):
/// the real binary links against the builder's specific
/// `libpython3.X.so.1.0`, not against the stable-ABI
/// `libpython3.so`. PBS 3.14.5 requires the builder to have
/// Python 3.14.x available at `cargo build` time so the linked
/// symlink version matches the bundle's libpython. On Windows
/// this problem does not exist (it links against the
/// `python3.dll` stable ABI shim).
fn build_file_with_bundle(
    path: &PathBuf,
    override_dest: Option<&std::path::Path>,
    dep_registry: manifest::DepRegistry,
    flag_defaults: std::collections::BTreeMap<String, bool>,
    bundle_pip: Vec<String>,
    bundle_pip_requirements: Vec<PathBuf>,
) {
    // --- Early validation: supported host triple ---
    let triple = match pbs::host_triple() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("✗ {}", e);
            std::process::exit(1);
        }
    };

    // --- Early validation: requirements files exist and are readable ---
    //
    // We do this BEFORE touching lex/parse/PBS so we fail fast on
    // user input (typical case: typo in the file path).
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
        // Approximate count only for the summary: non-blank lines
        // that don't start with `#`. pip handles the real parsing
        // (includes `-r other.txt`, options like `--hash`, etc.).
        requirements_pkg_count += content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count();
        requirements_abs_paths.push(abs);
    }

    // --- Lex + parse to detect `from python import` ---
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
        eprintln!(
            "✗ `--bundle-python` only applies to programs that use `from python import ...`."
        );
        eprintln!(
            "  If your program does not use Python interop, run `fitz build` without the flag — the resulting \
             binary is already standalone (it requires no external runtime)."
        );
        std::process::exit(1);
    }

    // --- Strict checker (no `--no-typecheck` in build) ---
    // 8-pyi.B: loads adjacent .pyi stubs before the check.
    let (env, types, _defs, type_errors) =
        check_program_with_pyi_stubs_and_deps(&program, path, &dep_registry);
    if !type_errors.is_empty() {
        eprintln!(
            "✗ {} — {} type error(s):",
            path.display(),
            type_errors.len()
        );
        for e in &type_errors {
            eprintln!("  {}", e);
        }
        eprintln!("   Use `fitz check` to review before building.");
        std::process::exit(1);
    }

    let (env, types) = if codegen::has_unannotated_fn_params(&program) {
        codegen::fill_inferred_param_types(&mut program, &types, &env);
        // 8-pyi.B: re-check also loads stubs (idempotent).
        let (env2, types2, _defs2, errs2) =
            check_program_with_pyi_stubs_and_deps(&program, path, &dep_registry);
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

    // Phase 5 (fitz-liveviews) — auto-inject `flv_register(...)` for
    // every `@live_component` type (parallel to `build_file`).
    if let Err(inject_errs) = fitz::types::inject_live_component_registrations(&mut program, &env) {
        eprintln!(
            "✗ {} — {} error(s) at implicit @live_component registration:",
            path.display(),
            inject_errs.len()
        );
        for e in &inject_errs {
            eprintln!("  {}", e);
        }
        std::process::exit(1);
    }

    // --- Codegen + write the real binary's Cargo project ---
    let project = match codegen::generate_project(
        path,
        &program,
        &env,
        &types,
        dep_registry,
        flag_defaults,
    ) {
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

    // --- Build the real binary ---
    println!("→ compilando real binary…");
    let output = std::process::Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&cargo_toml_path)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error invoking cargo (real binary): {}", e);
            std::process::exit(1);
        }
    };
    if !output.status.success() {
        eprintln!("✗ cargo build of the real binary failed:");
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
                "✗ real binary not found after cargo build ({}): {}",
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
            eprintln!("✗ canonicalize of the PBS tarball failed: {}", e);
            std::process::exit(1);
        }
    };
    let tarball_bytes = match fs::read(&tarball_abs) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("✗ read PBS tarball for hashing: {}", e);
            std::process::exit(1);
        }
    };

    // --- Phase 8.c — Pip install + secondary tarball (if --bundle-pip) ---
    //
    // If the user passed `--bundle-pip <pkg>` (one or more times),
    // we install those packages with pip at build time inside a
    // local project dir and pack them into a secondary tarball
    // that the launcher embeds as a second `include_bytes!`. On
    // the binary's first run, the launcher extracts the packages
    // into `python/Lib/site-packages/` (Windows) or
    // `python/lib/python3.X/site-packages/` (Unix) of the TMP
    // extract dir, automatically accessible via `import` from the
    // user's Python code.
    //
    // To run `pip install` we need a Python executable; we use
    // PBS's embedded python, extracted to the project's local
    // cache. The extract is ~60 MB but is reused between builds.
    let pip_total_count = bundle_pip.len() + requirements_pkg_count;
    let pip_tarball_abs: Option<PathBuf> = if !bundle_pip.is_empty()
        || !requirements_abs_paths.is_empty()
    {
        // --- pip_packages tarball cache key ---
        //
        // The `pip_inputs_hash` helper computes the deterministic
        // hash over the pip install inputs (sorted
        // `--bundle-pip` positionals + bytes of the requirements
        // files). The sidecar lives inside
        // `target/fitz-build/<bin>_*` → scoped by project.
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

        // Cache hit: tarball + sidecar exist and the hash matches.
        // We skip PBS extraction, pip install, and tar.
        let cache_hit = pip_tarball_path.exists()
            && pip_hash_sidecar
                .exists()
                .then(|| fs::read_to_string(&pip_hash_sidecar).ok())
                .flatten()
                .map(|s| s.trim() == pip_inputs_hash)
                .unwrap_or(false);

        if cache_hit {
            println!(
                "→ pip cache hit ({} package(s), hash {}…) — reusing tarball",
                pip_total_count,
                &pip_inputs_hash[..8]
            );
            match pip_tarball_path.canonicalize() {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("✗ canonicalize of the cached pip tarball failed: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            println!(
                "→ extracting PBS to local cache to run pip ({} package(s))…",
                pip_total_count
            );
            let pbs_extract_dir = PathBuf::from("target")
                .join("fitz-build")
                .join(format!("{}_pbs_extract", project.bin_name));
            if !pbs_extract_dir.join("python").exists() {
                if let Err(e) = fs::create_dir_all(&pbs_extract_dir) {
                    eprintln!("Error creating {}: {}", pbs_extract_dir.display(), e);
                    std::process::exit(1);
                }
                // Extract the PBS tarball using `tar -xzf` (same
                // subprocess the launcher uses at runtime).
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
                            "✗ tar -xzf of the PBS tarball failed (exit code: {:?})",
                            s.code()
                        );
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("✗ could not invoke `tar` to extract PBS: {}", e);
                        eprintln!(
                            "  You need `tar` on your PATH (bsdtar on Win11/macOS, GNU tar on Linux)."
                        );
                        std::process::exit(1);
                    }
                }
            }
            // Path of the Python executable inside the PBS extract.
            let python_exe = if cfg!(windows) {
                pbs_extract_dir.join("python").join("python.exe")
            } else {
                pbs_extract_dir.join("python").join("bin").join("python3")
            };
            if !python_exe.exists() {
                eprintln!(
                    "✗ python not found in the PBS extract: {}",
                    python_exe.display()
                );
                std::process::exit(1);
            }

            // pip install dir. Clean previous one if it exists for
            // reproducible builds (pip install --target is
            // additive; without cleaning, packages accumulate
            // between builds).
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
                // Quiet to avoid noise in CI; errors are still
                // visible on stderr.
                "--quiet".to_string(),
            ];
            // `-r <file>` per requirements file. pip accumulates
            // them with the positionals that come after; all the
            // file's native syntax (comments, includes, version
            // pins, --hash) is handled by pip directly, without
            // parsing on the Fitz side.
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
                    eprintln!("✗ could not invoke pip: {}", e);
                    std::process::exit(1);
                }
            };
            if !pip_out.status.success() {
                eprintln!("✗ pip install failed:");
                eprintln!("--- stderr ---");
                eprintln!("{}", String::from_utf8_lossy(&pip_out.stderr));
                std::process::exit(1);
            }

            // Create the secondary tarball from pip_install_dir.
            // We use `tar -czf` with `-C <dir>` so the inner
            // paths are relative (without the cwd prefix).
            println!("→ packing pip_packages.tar.gz…");
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
                        "✗ tar -czf of the pip_packages tarball failed (exit code: {:?})",
                        s.code()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("✗ could not invoke `tar` to pack pip: {}", e);
                    std::process::exit(1);
                }
            }
            // Write sidecar with the hash of the inputs. The
            // sidecar lives next to the tarball so that
            // `rm target/fitz-build/<bin>_*` cleans everything
            // together. If the write fails we don't abort the
            // build — the cache simply won't work next time.
            if let Err(e) = fs::write(&pip_hash_sidecar, &pip_inputs_hash) {
                eprintln!(
                    "Warning: could not write cache sidecar `{}`: {}",
                    pip_hash_sidecar.display(),
                    e
                );
            }
            match pip_tarball_path.canonicalize() {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("✗ canonicalize of the pip tarball failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
    } else {
        None
    };

    // Combined hash: if there's pip, include the pip tarball
    // bytes in the hash so two projects with different packages
    // get different extract dirs in TMP (correct cache
    // hit-or-miss).
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

    // --- Generate + build the launcher ---
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
            eprintln!("Error invoking cargo (launcher): {}", e);
            std::process::exit(1);
        }
    };
    if !output.status.success() {
        eprintln!("✗ cargo build of the launcher failed:");
        eprintln!(
            "   (check {} to see the generated code.)",
            launcher_src.display()
        );
        eprintln!("--- stderr ---");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }

    // --- Copy the launcher to the user's destination ---
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
    if let Err(e) = copy_binary_with_retry(&launcher_release_path, &bin_out) {
        eprintln!(
            "Error copying {} to {}: {}",
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

/// Detects `from python import X` in the AST. Returns true if at
/// least one `Stmt::FromImport` has `path[0] == "python"`. Used by
/// `build_file_with_bundle` to validate the use of
/// `--bundle-python`.
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

/// Phase 13 (v0.11.0) — Dispatches CLI args to the matching
/// command. Returns the exit code so the caller can propagate it
/// via `std::process::exit`. Prints help / errors to stderr as
/// appropriate.
///
/// Exit-code policy:
/// - `0` success (handler returned 0 or asked for --help).
/// - `1`+ returned by the handler as the exit code.
/// - `2` CLI parse error (unknown command, missing arg, invalid
///   type). Standard POSIX convention.
fn dispatch_cli(registry: &fitz::cli::CliRegistry, argv: &[String]) -> i32 {
    let bin_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "fitz".to_string());
    let cmds_snapshot = registry.snapshot();
    let multi = cmds_snapshot.len() >= 2;
    match fitz::cli::parse_argv(argv, registry) {
        fitz::cli::ParseResult::GlobalHelp => {
            println!(
                "{}",
                fitz::cli::render_global_help(&bin_name, &cmds_snapshot)
            );
            0
        }
        fitz::cli::ParseResult::CommandHelp(cmd) => {
            println!("{}", fitz::cli::render_command_help(&bin_name, &cmd, multi));
            0
        }
        fitz::cli::ParseResult::Error(msg, code) => {
            eprintln!("error: {}", msg);
            code
        }
        fitz::cli::ParseResult::Invoke(inv) => {
            // Invoke the handler. The evaluator's tokio runtime
            // builds a current_thread runtime.
            let rt = evaluator::build_runtime();
            let value_result = rt.block_on(async move {
                // Resolve defaults for args == Value::Null (parser
                // sentinel when a flag was not passed). The
                // Param's default is evaluated at the handler's
                // call site.
                let cmd = inv.cmd;
                let mut final_args: Vec<fitz::value::Value> = Vec::with_capacity(cmd.params.len());
                for (i, p) in cmd.params.iter().enumerate() {
                    let v = &inv.args[i];
                    let is_default_sentinel = matches!(v, fitz::value::Value::Null);
                    match (is_default_sentinel, p.default.as_ref()) {
                        (true, Some(de)) => {
                            // Eval the default expression with a
                            // fresh env — defaults are simple
                            // literals by convention.
                            let env = fitz::env::Environment::new();
                            match evaluator::eval_expr_for_default(de, env).await {
                                Ok(dv) => final_args.push(dv),
                                Err(e) => {
                                    return Err(format!(
                                        "fallo al resolver default de `--{}`: {}",
                                        p.name, e
                                    ));
                                }
                            }
                        }
                        _ => final_args.push(v.clone()),
                    }
                }
                evaluator::invoke_value(
                    cmd.handler.clone(),
                    final_args,
                    &format!("@command(\"{}\")", cmd.name),
                    fitz::ast::Span::ZERO,
                )
                .await
                .map_err(|signal| format!("{:?}", signal))
            });
            match value_result {
                Ok(fitz::value::Value::Int(n)) => n as i32,
                Ok(other) => {
                    eprintln!(
                        "error: the command returned `{}` instead of Int (exit code)",
                        other.type_name()
                    );
                    1
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    1
                }
            }
        }
    }
}

fn run_file(
    path: &PathBuf,
    no_typecheck: bool,
    dep_registry: manifest::DepRegistry,
    cli_args: Vec<String>,
) {
    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error leyendo {}: {}", path.display(), e);
        std::process::exit(1);
    });

    // Phase 2.1: lexer
    let tokens = match lexer::tokenize(&source) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Phase 2.3: parser
    let program = match parser::parse(tokens) {
        Ok(program) => program,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Phase 5.4: static checker in strict mode by default. Type
    // errors abort execution before reaching the evaluator. The
    // `--no-typecheck` flag changes the behavior to warning (they
    // get reported but execution continues), intended for legacy
    // code or to diagnose checker bugs.
    // 8-pyi.B: loads adjacent .pyi stubs before the check.
    let (type_env, _types, _defs, type_errors) =
        check_program_with_pyi_stubs_and_deps(&program, path, &dep_registry);
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
                "✗ {} — {} type error(s):",
                path.display(),
                type_errors.len()
            );
            for e in &type_errors {
                eprintln!("  {}", e);
            }
            eprintln!(
                "   Use `fitz check` to review, or `fitz run --no-typecheck {}` to run anyway.",
                path.display()
            );
            std::process::exit(1);
        }
    }

    // Phase 5 (fitz-liveviews) — auto-inject `flv_register(...)` for
    // every `@live_component` type. Only if the checker succeeded
    // (in `--no-typecheck` mode we skip injection to preserve the
    // legacy behavior of running through even with type errors).
    let mut program = program;
    if type_errors.is_empty() {
        if let Err(inject_errs) =
            fitz::types::inject_live_component_registrations(&mut program, &type_env)
        {
            eprintln!(
                "✗ {} — {} error(s) at implicit @live_component registration:",
                path.display(),
                inject_errs.len()
            );
            for e in &inject_errs {
                eprintln!("  {}", e);
            }
            std::process::exit(1);
        }
    }

    // Base dir for resolving `import`s: the directory of the file
    // being executed. If for some reason we can't derive it (path
    // without a parent), we fall back to cwd.
    let base_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Phase 2.4 + Phase 4: we evaluate the program inside an
    // active `HttpRegistry`. HTTP decorators register routes there
    // while the eval runs. If after eval the registry has routes,
    // we start the server; otherwise we finish like a regular CLI
    // program.
    //
    // Phase 7.2: the server also needs the original AST to
    // pre-compute the OpenAPI schema (`components.schemas` walks
    // the `Stmt::TypeDef`s). We clone before moving it to the
    // evaluator.
    //
    // Phase 13 (v0.11.0) — We install an empty CliRegistry before
    // the eval. If the program has `@command`s, they register
    // here. Post-eval, if count > 0, we dispatch the CLI from
    // `cli_args`.
    let cli_registry = std::sync::Arc::new(fitz::cli::CliRegistry::new());
    evaluator::install_cli_registry(std::sync::Arc::clone(&cli_registry));
    let program_for_server = program.clone();
    // v0.37.3 — build ONE shared multi-thread runtime up-front and run the
    // eval on it (instead of the eval building + dropping its own
    // `current_thread` runtime via `eval_with_base_and_deps_sync`). The HTTP
    // server / cron scheduler below reuse this SAME runtime, so any DB
    // connection the eval opens (e.g. `let db = db.connect(...).await` used by
    // `@cron(store=db)`) stays bound to a live reactor for the whole
    // `fitz run`. Fixes the "A Tokio 1.x context was found, but it is being
    // shutdown" panic that hit persistent cron in both cron-only and HTTP+cron
    // modes. The eval future is polled on the current (main) thread by
    // `block_on`, exactly as before — same stack, no behavior change.
    let shared_runtime = match http::build_server_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error inicializando el runtime: {}", e);
            std::process::exit(1);
        }
    };
    let (eval_result, registry) = http::with_active_registry(|| {
        shared_runtime.block_on(evaluator::eval_with_base_and_deps(
            program,
            base_dir,
            dep_registry,
        ))
    });
    evaluator::uninstall_cli_registry();

    if let Err(e) = eval_result {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    // v0.37.7 — Install the background registry globally so `spawn(...)`
    // from an HTTP handler / cron job (running on a tokio worker
    // without the eval's thread-local) resolves the persistence config.
    // Cheap even when there are no `@background(store=db)` fns.
    http::install_background_registry(registry.background_registry.clone());

    // v0.37.7 — `@background(store=db, catch_up=true)`: at boot, mark
    // orphaned rows (`running`/`retrying` left mid-flight by a crash)
    // as `failed`. Best-effort — never aborts the process. Runs before
    // the serve/cron branch so it covers HTTP, cron-only, and plain
    // script modes.
    if !registry.background_registry.is_empty() {
        let stores = registry.background_registry.catch_up_stores();
        if !stores.is_empty() {
            shared_runtime.block_on(async {
                for s in &stores {
                    if background_jobs::ensure_bg_storage_initialized(s)
                        .await
                        .is_ok()
                    {
                        match background_jobs::mark_orphaned_failed(s).await {
                            Ok(n) if n > 0 => {
                                eprintln!(
                                    "⚙️  @background catch_up: marked {} orphaned job(s) as failed",
                                    n
                                );
                            }
                            Ok(_) => {}
                            Err(e) => eprintln!("⚙️  @background catch_up failed: {}", e),
                        }
                    }
                }
            });
        }
    }

    // v0.11.0 — CLI mode takes priority over HTTP server / cron.
    // If the program declared `@command`s, we don't serve HTTP —
    // we're a CLI tool. (In the future we could support HTTP + CLI
    // coexisting via `serve` / `cmd1` subcommands, but the MVP
    // keeps them separate.)
    if cli_registry.count() > 0 {
        let exit_code = dispatch_cli(&cli_registry, &cli_args);
        std::process::exit(exit_code);
    }

    if !registry.is_empty() {
        // If the program declared `@server(port, host)`, we use
        // that; otherwise default 127.0.0.1:3000.
        let config = registry.resolved_config();
        let addr = match config.to_socket_addr() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Error en @server: {}", e);
                std::process::exit(1);
            }
        };
        if let Err(e) = http::serve_on_runtime(&shared_runtime, registry, program_for_server, addr)
        {
            eprintln!("Error del servidor HTTP: {}", e);
            std::process::exit(1);
        }
    } else if registry.cron_registry.has_jobs() || registry.every_registry.has_jobs() {
        // Phase 9.w.3 + Phase 3c — scheduler-only mode: the program has NO HTTP
        // routes but DOES have `@cron` and/or `@every` jobs. We start both
        // schedulers standalone and block until SIGINT/Ctrl+C (decision
        // confirmed with the author: live blocking, systemd-friendly mode).
        let cron_registry = registry.cron_registry.clone();
        let every_registry = registry.every_registry.clone();
        if let Err(e) =
            cron_jobs::run_schedulers_on_runtime(&shared_runtime, cron_registry, every_registry)
        {
            eprintln!("Error del scheduler: {}", e);
            std::process::exit(1);
        }
    }
}

// ---- Phase 9.y.1 — scaffolding (`fitz new` / `fitz init`) ----

/// Template for the default `src/main.fitz` (CLI hello world).
/// Follows the style of guide chapter 2
/// (`examples/guide/02-hola.fitz`): top-level `print(...)` without
/// `fn main`.
fn template_cli(name: &str) -> String {
    format!(
        "// main.fitz — generated by `fitz new`\n\
         //\n\
         // Your first Fitz program. Run it with `fitz run src/main.fitz`.\n\
         // When 9.y.2 lands, you will also be able to just `fitz run`\n\
         // from the project root (it reads `fitz.toml` automatically).\n\
         \n\
         print(\"Hello from {name} 🏔️\")\n"
    )
}

/// Template for `src/main.fitz` with `--http`. Minimal server that
/// responds to a GET at `/`. Follows the canonical
/// `@server(...) fn main() => 0` pattern from guide chapter 17.
fn template_http(name: &str) -> String {
    format!(
        "// main.fitz — generated by `fitz new --http`\n\
         //\n\
         // Minimal HTTP server. Run it with `fitz run src/main.fitz` and\n\
         // try: curl http://127.0.0.1:3000/\n\
         \n\
         @get(\"/\")\n\
         fn index() -> Str {{\n\
         \x20   return \"Hello from {name} 🏔️\"\n\
         }}\n\
         \n\
         @server(3000)\n\
         fn main() => 0\n"
    )
}

/// Template for the `.gitignore`. `fitz.lock` is NOT here: the
/// lockfile is committed (Cargo-style), not ignored.
fn template_gitignore() -> &'static str {
    "# Build artifacts\n\
     target/\n\
     \n\
     # Binaries generated by `fitz build` next to the source.\n\
     # If you publish a package, adjust this to your needs.\n\
     *.exe\n\
     *.pdb\n"
}

/// `fitz new <name> [--http] [--no-git]` — creates a new Fitz
/// project in a folder. Fails if the folder already exists.
fn new_project(name: &str, http: bool, template: Option<&str>, no_git: bool) {
    if !manifest::is_valid_package_name(name) {
        eprintln!(
            "✗ invalid name: `{name}`. Must match `^[a-z][a-z0-9_-]{{0,63}}$` \
             (lowercase, start with a letter, contain only letters/digits/`-`/`_`, max \
             64 characters)."
        );
        std::process::exit(1);
    }

    let target = PathBuf::from(name);
    if target.exists() {
        eprintln!(
            "✗ `{}` already exists — delete it or pick another name.",
            target.display()
        );
        std::process::exit(1);
    }

    if let Some(tpl_name) = template {
        scaffold_from_named_template(&target, name, tpl_name, no_git);
    } else {
        scaffold_project(&target, name, http, no_git);
    }

    println!("✓ Fitz project created at `{}`", target.display());
    println!();
    println!("To try it out:");
    println!("  cd {}", target.display());
    println!("  fitz run src/main.fitz");
}

/// Scaffolds `<target>` from the named template in the built-in
/// registry (see `fitz::templates::resolve_template`).
///
/// Aborts with an actionable error if the template name is unknown or
/// the scaffolding fails. Runs `git init` at the end unless `no_git`.
fn scaffold_from_named_template(
    target: &std::path::Path,
    project_name: &str,
    template_name: &str,
    no_git: bool,
) {
    let source = match templates::resolve_template(template_name) {
        Some(s) => s,
        None => {
            eprintln!("✗ unknown template: `{template_name}`. Available: `liveviews`.");
            std::process::exit(1);
        }
    };

    // Create the target dir up front — the templates module expects a
    // writable destination.
    if let Err(e) = fs::create_dir_all(target) {
        eprintln!("✗ could not create `{}`: {e}", target.display());
        std::process::exit(1);
    }

    if let Err(e) = templates::scaffold_from_template(&source, target, project_name) {
        eprintln!("✗ {e}");
        std::process::exit(1);
    }

    if !no_git {
        init_git_in_new_project(target);
    }
}

/// Runs `git init --quiet` in `dir`. Never aborts — the project is
/// still valid without git; failure is a warning.
fn init_git_in_new_project(dir: &std::path::Path) {
    match std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(dir)
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "  (notice: `git init` exited with code {} — the project was created anyway. \
                 Pass `--no-git` to silence this notice.)",
                status.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            eprintln!(
                "  (notice: could not run `git init` ({e}). The project was created \
                 anyway. Pass `--no-git` to silence this notice.)"
            );
        }
    }
}

/// `fitz init [--name X] [--http] [--no-git]` — initializes a Fitz
/// project in the current directory. Fails if a `fitz.toml`
/// already exists.
fn init_project(name_override: Option<&str>, http: bool, no_git: bool) {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("✗ could not read the current directory: {e}");
        std::process::exit(1);
    });

    let name = match name_override {
        Some(n) => n.to_string(),
        None => match cwd.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => {
                eprintln!(
                    "✗ could not derive the name from the current directory. \
                     Pass it explicitly with `--name <name>`."
                );
                std::process::exit(1);
            }
        },
    };

    if !manifest::is_valid_package_name(&name) {
        eprintln!(
            "✗ invalid name: `{name}`. Must match `^[a-z][a-z0-9_-]{{0,63}}$`. \
             Pass `--name <valid-name>` if the directory does not match the format."
        );
        std::process::exit(1);
    }

    if cwd.join(manifest::MANIFEST_FILE).exists() {
        eprintln!(
            "✗ `{}` already exists in the current directory.",
            manifest::MANIFEST_FILE
        );
        std::process::exit(1);
    }

    scaffold_project(&cwd, &name, http, no_git);
    println!("✓ Fitz project `{name}` initialized at `{}`", cwd.display());
    println!();
    println!("To try it out:");
    println!("  fitz run src/main.fitz");
}

/// Common scaffolding: creates `<target>/fitz.toml`,
/// `<target>/src/main.fitz`, `<target>/.gitignore`, and (unless
/// `no_git`) runs `git init`.
///
/// Exits the process with code 1 on any I/O error.
fn scaffold_project(target: &std::path::Path, name: &str, http: bool, no_git: bool) {
    // Create directories.
    let src = target.join("src");
    if let Err(e) = fs::create_dir_all(&src) {
        eprintln!("✗ could not create `{}`: {e}", src.display());
        std::process::exit(1);
    }

    // Write fitz.toml.
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
        eprintln!("✗ could not write `{}`: {e}", toml_path.display());
        std::process::exit(1);
    }

    // Write src/main.fitz with the chosen template.
    let main_text = if http {
        template_http(name)
    } else {
        template_cli(name)
    };
    let main_path = src.join("main.fitz");
    if let Err(e) = fs::write(&main_path, main_text) {
        eprintln!("✗ could not write `{}`: {e}", main_path.display());
        std::process::exit(1);
    }

    // Write .gitignore.
    let gi_path = target.join(".gitignore");
    if let Err(e) = fs::write(&gi_path, template_gitignore()) {
        eprintln!("✗ could not write `{}`: {e}", gi_path.display());
        std::process::exit(1);
    }

    // git init (optional). We don't abort if it fails: the
    // project is still valid without git; we only note it as a
    // warning.
    if !no_git {
        init_git_in_new_project(target);
    }
}

// ---- Phase 9.y.4 — `fitz add` / `fitz remove` / `fitz update` ----

/// `fitz add <name> [--path <p>] [--git <url> --tag <t>|--rev <r>]`
/// — Phase 9.y.4. Modifies the current project's `fitz.toml`
/// `[dependencies]` (cwd or ancestors), preserves formatting with
/// `toml_edit`, and syncs `fitz.lock` by resolving all deps
/// including the new one. If the dep already existed, it's
/// overwritten.
fn add_dep_cmd(
    name: &str,
    path_opt: Option<&str>,
    git_opt: Option<&str>,
    tag_opt: Option<&str>,
    rev_opt: Option<&str>,
) {
    // Build the spec from the flags. clap already validated
    // conflicts_with / requires between path/git/tag/rev; we
    // double-check defensively anyway.
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
                        "✗ `--git` also requires `--tag <tag>` or `--rev <commit>` for \
                         reproducibility. `branch` is intentionally unsupported."
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
            // clap should have blocked this.
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
        eprintln!("✗ could not read `{}`: {e}", manifest_path.display());
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

    // Re-resolve + sync lockfile (manifest mode with file=None).
    // resolve_entry loads the updated manifest and resolves ALL
    // deps (the new one included). If resolution fails, the
    // manifest stays persisted — the user can run `fitz remove`
    // to revert.
    let resolved = resolve_entry(None);
    sync_lockfile_if_needed(&resolved);
}

/// `fitz remove <name>` — Phase 9.y.4. Removes the entry from the
/// manifest and re-syncs the lockfile. If the dep didn't exist,
/// clear error.
fn remove_dep_cmd(name: &str) {
    let manifest_path = find_local_manifest_or_exit();
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("✗ could not read `{}`: {e}", manifest_path.display());
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

    // Re-resolve so the lockfile reflects the new list of deps.
    // If the removed dep was the only one, sync_lockfile_if_needed
    // will detect empty deps and skip writing (but the old
    // lockfile is still there with the stale entry). We clean
    // that by hand:
    let resolved = resolve_entry(None);
    if let Some(ctx) = &resolved.manifest_ctx {
        if ctx.resolved_deps.is_empty() {
            let lock_path = lockfile::lockfile_path(&ctx.manifest_dir);
            if lock_path.exists() {
                if let Err(e) = std::fs::remove_file(&lock_path) {
                    eprintln!(
                        "  (notice: could not delete `{}`: {e})",
                        lock_path.display()
                    );
                } else {
                    println!("✓ deleted {} (empty deps)", lock_path.display());
                }
            }
        }
    }
    sync_lockfile_if_needed(&resolved);
}

/// `fitz update [name]` — Phase 9.y.4. Re-resolves deps; for git
/// deps, invalidates the local cache (deletes the dir) and forces
/// a re-clone with the most recent commit of the requested
/// tag/rev. For path deps it's a no-op (always fresh). Without
/// `name`, updates all of them; with `name`, only that dep.
fn update_deps_cmd(name_filter: Option<&str>) {
    let manifest_path = find_local_manifest_or_exit();

    // Parse the manifest without touching the resolver — we only
    // need the [dependencies] list to iterate.
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("✗ could not read `{}`: {e}", manifest_path.display());
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
        // Only git deps have a cache to invalidate; path deps are
        // no-op.
        if let manifest::Dependency::Detailed(d) = dep {
            if let Some(url) = &d.git {
                let gitref = match (&d.tag, &d.rev) {
                    (Some(t), None) => fitz::git_dep::GitRef::Tag(t.clone()),
                    (None, Some(r)) => fitz::git_dep::GitRef::Rev(r.clone()),
                    _ => continue, // invalid shape — the resolver will report
                };
                let cache_path = match fitz::git_dep::cache_path_for(url, &gitref) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("✗ dep `{dep_name}`: could not compute the cache path: {e}");
                        std::process::exit(1);
                    }
                };
                if cache_path.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&cache_path) {
                        eprintln!(
                            "✗ could not delete the cache of `{dep_name}` at `{}`: {e}",
                            cache_path.display()
                        );
                        std::process::exit(1);
                    }
                    busted.push(dep_name.clone());
                }
            }
        }
    }

    // Validate that the `--name` filter actually matched
    // something (UX: if the user typos the name, I don't want
    // silence).
    if let Some(filter) = name_filter {
        if !parsed.dependencies.contains_key(filter) {
            eprintln!(
                "✗ dep `{filter}` is not in `[dependencies]` of `{}`.",
                manifest_path.display()
            );
            std::process::exit(1);
        }
    }

    if busted.is_empty() {
        match name_filter {
            Some(_) => println!("(nothing to update — dep without cache)"),
            None => println!("(no git deps with cache to invalidate)"),
        }
    } else {
        println!("✓ cache invalidated for: {}", busted.join(", "));
    }

    // Re-resolve via manifest mode (which will re-clone the git
    // deps because their cache no longer exists) + sync
    // lockfile. We pass `None` so resolve_entry does
    // `find_manifest` from the cwd; we already know the manifest
    // exists (manifest_path above confirmed it).
    let _ = manifest_path; // keep the lifetime alive; resolve_entry does the discover
    let resolved = resolve_entry(None);
    sync_lockfile_if_needed(&resolved);
}

// ---- Phase 9.z.1 — `fitz fmt` ----

/// `fitz fmt [files...] [--check]` — Phase 9.z.1. Formats `.fitz`
/// files to the canonical style. Without `files`, formats the
/// whole project (discovers via manifest). With `--check`, does
/// not write — exit 1 if any file differs from its canonical form
/// (CI mode).
///
/// File discovery in project mode includes `src/main.fitz` (from
/// `[bin].main`), `src/lib.fitz` (from `[lib].entry`), and any
/// extra `.fitz` in `src/` (recursive walk). Excludes `target/`
/// and any hidden dir.
fn fmt_cmd(files: Vec<PathBuf>, check: bool) {
    let targets = if files.is_empty() {
        // Project mode — discover via manifest.
        discover_project_fitz_files()
    } else {
        files
    };

    if targets.is_empty() {
        eprintln!("✗ no se encontraron archivos `.fitz` para formatear.");
        std::process::exit(1);
    }

    // (Phase 9.z.1.b: the loud warning from 9.z.1.a was removed
    // because we now preserve the user's comments + blank
    // lines.)

    let mut any_diff = false;
    let mut errors = 0usize;
    for path in &targets {
        match fmt_one_file(path, check) {
            Ok(FmtResult::Unchanged) => {}
            Ok(FmtResult::Wrote) => {
                println!("✓ formatted {}", path.display());
            }
            Ok(FmtResult::WouldChange) => {
                println!("✗ {} is not in canonical format", path.display());
                any_diff = true;
            }
            Err(e) => {
                eprintln!("✗ {}: {e}", path.display());
                errors += 1;
            }
        }
    }

    if errors > 0 {
        eprintln!("\n{errors} file(s) with parsing errors — fmt could not process them.");
        std::process::exit(1);
    }
    if check && any_diff {
        eprintln!("\nuso `fitz fmt` (sin `--check`) para aplicar el formato.");
        std::process::exit(1);
    }
}

enum FmtResult {
    /// The file was already in canonical form.
    Unchanged,
    /// We wrote the file in canonical form.
    Wrote,
    /// `--check` mode: the file would change if formatted.
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

/// Discovers `.fitz` files in the current project via the
/// manifest. Reads `[bin].main` and `[lib].entry` (if they exist),
/// then recursive walks `src/`. Excludes `target/` and hidden
/// directories (`.git/`, etc.).
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
    // Dedup by canonicalized path to avoid formatting the same
    // file twice if it appears as `[bin].main` and also in the
    // walk of `src/`.
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
        // Skip hidden dirs (`.git`, `.fitz-cache`) and `target/`.
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

/// Shared helper for add/remove/update: finds the current
/// project's `fitz.toml` or exits with a clear error.
fn find_local_manifest_or_exit() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("✗ could not read the current directory: {e}");
        std::process::exit(1);
    });
    match manifest::find_manifest(&cwd) {
        Some(p) => p,
        None => {
            eprintln!(
                "✗ could not find `{}` in `{}` or in parent directories. \
                 Create a project with `fitz new <name>` / `fitz init` before \
                 using `add`/`remove`/`update`.",
                manifest::MANIFEST_FILE,
                cwd.display()
            );
            std::process::exit(1);
        }
    }
}

// ---- Phase 9.z.2.b — `fitz test` (built-in testing) ----

/// A source of tests for the runner. `path` is absolute (the
/// `fitz test file.fitz` invocation canonicalizes it). `label` is
/// the friendly name used to prefix the names in the output
/// (`<label>::<test>`); `None` means no prefix — typical case of
/// single-file mode.
struct TestSource {
    path: PathBuf,
    label: Option<String>,
}

/// Entry point of the `fitz test` sub-command (Phase 9.z.2.b).
///
/// - **Single-file mode** (`fitz test --file file.fitz [filter]`):
///   evaluates `file.fitz` with an active `TestRegistry`, then
///   runs the discovered tests.
/// - **Manifest mode** (`fitz test [filter]`): looks for
///   `fitz.toml`, evaluates the lib entry (or the bin if there's
///   no lib) + every top-level `tests/*.fitz` in the manifest
///   directory. Each file is evaluated with its path as
///   `source_label` so the output prefixes the names.
///
/// The filter is a case-sensitive substring on the test name
/// (without the file prefix). Cargo style.
fn test_cmd(filter: Option<String>, file_arg: Option<PathBuf>) {
    let (sources, dep_registry) = match file_arg {
        Some(p) => {
            // Single-file: the path as-is; no label in the
            // output. Empty dep registry (single-file does not
            // touch `fitz.toml`).
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
            "✗ no test files found.\n\
             In manifest mode we discover `[lib].entry` (or `[bin].main`) + \
             top-level `tests/*.fitz`. In single-file, pass `--file <file.fitz>`."
        );
        std::process::exit(1);
    }

    // Build a tokio current_thread runtime + block over the whole
    // operation: discovery (evaluate each file with the registry
    // active) + running the tests. A single runtime invocation
    // for everything, so the TestSpecs accumulate in the same
    // registry.
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

/// Discovers test sources in manifest mode. Reads the manifest
/// (it must exist), builds the `dep_registry` (path/git dep
/// resolution), then returns:
///
/// 1. `[lib].entry` if it exists; otherwise `[bin].main` if it
///    exists; otherwise no project source (only `tests/*.fitz`).
/// 2. All top-level `tests/<name>.fitz` in the manifest directory
///    (non-recursive — aligned with how Cargo discovers
///    integration tests).
///
/// Unlike `resolve_entry`, we do NOT require `[bin]` — a lib-only
/// project is valid (90% of the libraries case). If there's
/// neither lib nor bin nor `tests/`, we return an empty list and
/// the caller (`test_cmd`) emits the "no test files found"
/// message.
///
/// The `label`s are paths relative to `manifest_dir`
/// (`"src/lib.fitz"`, `"tests/math.fitz"`) so the output is
/// readable and portable across machines.
fn discover_test_sources_from_manifest() -> (Vec<TestSource>, manifest::DepRegistry) {
    let manifest_path = find_local_manifest_or_exit();
    let manifest_text = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("✗ could not read `{}`: {e}", manifest_path.display());
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

    // Eager dep resolution (fail-fast with the resolver's
    // message). Without deps, the dep_registry stays empty and
    // the loader will only use relative paths.
    let resolved_deps = manifest::resolve_dependencies(&parsed_manifest, &manifest_dir)
        .unwrap_or_else(|e| {
            eprintln!("✗ no se pudieron resolver las dependencias: {e}");
            std::process::exit(1);
        });

    // Sync lockfile (no-op if there are no deps or it's already
    // in sync).
    if !resolved_deps.is_empty() {
        let lock = lockfile::Lockfile::from_resolved(&resolved_deps);
        let lock_path = lockfile::lockfile_path(&manifest_dir);
        if let Err(e) = lockfile::write_lockfile_if_changed(&lock_path, &lock) {
            eprintln!("✗ no se pudo escribir `{}`: {e}", lock_path.display());
            std::process::exit(1);
        }
    }
    let mut dep_registry = manifest::build_dep_registry(&resolved_deps);

    // Auto-self-import: if the project declares `[lib].entry`,
    // we register the lib under the package name in the
    // `dep_registry`. This lets `tests/*.fitz` do
    // `from <pkg-name> import X` to access the lib's code —
    // parallel to Rust's `use my_crate::*` in integration tests.
    // Without this, tests would have to write fragmented paths
    // (`from ../src/lib import X`) that the current loader does
    // not support.
    if let Some(lib) = &parsed_manifest.lib {
        let lib_path = manifest_dir.join(&lib.entry);
        if lib_path.exists() {
            dep_registry.insert(parsed_manifest.package.name.clone(), lib_path);
        }
    }

    // First we collect the top-level `tests/*.fitz` of the
    // manifest dir (non-recursive). Alphabetical order for
    // reproducibility.
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
        // **"Integration tests" mode**: we ONLY load the
        // `tests/*.fitz`. The `[lib]` (or `[bin]`) is loaded
        // indirectly when a test does `from <pkg> import X` —
        // the dep_registry has the auto-self registered, and the
        // loader caches by canonical path, so an `@test`
        // declared in the lib is discovered ONCE even if several
        // tests import the lib. If nobody imports it, it's not
        // discovered (visible debt for the degenerate case).
        sources.extend(integration_sources);
    } else {
        // **"Inline tests only" mode**: we load the `[lib]` (or
        // `[bin]`) directly because it's the only place that
        // can have `@test`.
        // Phase 11.5.b — `bins` may hold zero, one, or many; we
        // pick the first bin as the discovery source for inline
        // tests when the manifest has no `[lib]`. Multi-bin
        // projects that want per-bin `@test` selection are a
        // deferred refinement (visible debt for `fitz test --bin`).
        let entry_rel: Option<String> = match (&parsed_manifest.lib, parsed_manifest.bins.first()) {
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

/// Evaluates a file with the `TestRegistry` (and possibly
/// `CURRENT_TEST_SOURCE`) already active in the caller. Does
/// lexer + parser + strict checker + eval. If the checker reports
/// errors, formats them and returns `Err` (the caller decides
/// whether to abort).
///
/// `base_dir` is derived from the file's directory — so relative
/// imports (`from utils import X`) resolve to the file's sibling,
/// parallel to `fitz run` single-file.
async fn eval_test_source(
    path: &std::path::Path,
    dep_registry: &manifest::DepRegistry,
) -> Result<(), String> {
    let source = fs::read_to_string(path).map_err(|e| format!("no se pudo leer: {e}"))?;

    let tokens = lexer::tokenize(&source).map_err(|e| format!("{e}"))?;
    let program = parser::parse(tokens).map_err(|e| format!("{e}"))?;

    // 8-pyi.B: loads adjacent .pyi stubs before the check (also
    // in tests — `tests/*.fitz` files that use `from python
    // import foo` see the types from the adjacent `foo.pyi`).
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

/// Runs `f` on a dedicated large-stack thread and returns its
/// result. Wraps the evaluation in `fitz test` and `fitz repl` (the
/// two budgeted contexts) so deep-but-bounded recursion hits the
/// depth budget cleanly instead of overflowing the native stack. The
/// interpreter's async functions compile to large state machines
/// (~100 KB+ of native stack per Fitz call frame), so the default OS
/// thread stack overflows at a shallow depth; the stack size here is
/// derived from the depth budget via `eval_thread_stack_size`.
/// `std::thread::scope` lets `f` borrow from the caller.
fn run_on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(evaluator::eval_thread_stack_size())
            .spawn_scoped(scope, f)
            .expect("could not spawn the evaluation thread")
            .join()
            .expect("the evaluation thread panicked")
    })
}

/// Runs every test in the registry, applies the optional
/// `filter`, reports cargo-style (`test <name> ... ok/FAILED`)
/// with a final summary, and returns the number of failed tests.
/// The caller uses that number to decide the exit code (`>0` →
/// 1).
///
/// Output:
/// - `running N tests` (or `N (M filtered out)` if there's a
///   filter).
/// - For each test: `test <full_name> ... <result>` with the
///   result colored (ok green, FAILED red) if stdout is a TTY.
/// - If there are failures, a `failures:` section with detail
///   for each one (FitzError or EvalSignal message).
/// - Summary: `test result: ok|FAILED. P passed; F failed;
///   finished in Ts`.
fn run_test_registry(registry: &testing::TestRegistry, filter: Option<&str>) -> usize {
    use std::io::IsTerminal;

    // Raw ANSI — use colors only if stdout is a TTY (not
    // redirected).
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

    // Apply filter. Excluded tests are counted as "filtered
    // out" in the output (cargo style).
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
        // We print "test <name> ..." and leave OK/FAILED
        // pending to print after running (cargo does it on the
        // same line with a buffer — here we use print! +
        // flush).
        print!("test {} ... ", full_name);
        std::io::Write::flush(&mut std::io::stdout()).ok();

        let outcome = run_on_big_stack(|| runtime.block_on(invoke_one_test(test)));
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

/// Invokes an individual test via `evaluator::run_test_handler`.
/// Any `FitzError` is returned as `Err(formatted_string)` — the
/// runner records it in the `failures:` section of the output.
async fn invoke_one_test(test: &testing::TestSpec) -> Result<(), String> {
    // Install a fresh resource budget for this test. An infinite loop
    // or infinite recursion inside a test is always a bug, and we want
    // it reported as a clean failure instead of hanging the runner.
    // (`fitz run` never installs a budget — its loops are legitimate.)
    evaluator::install_eval_budget();
    let result = evaluator::run_test_handler(test.handler.clone(), test.is_async, &test.name)
        .await
        .map_err(|e| format!("{e}"));
    evaluator::uninstall_eval_budget();
    result
}

// ---- Phase 9.z.3 — `fitz dev` (hot reload) ----

/// Resolved at the start of `dev_cmd`: which directory we watch
/// and which arguments we pass to the `fitz run` child.
/// Single-file mode uses the file's parent as the watch root +
/// `fitz run <file>`; manifest mode uses `manifest_dir` as the
/// root + `fitz run` (no args, so the child re-discovers the
/// manifest on each start and respects `[bin].main` changes in
/// `fitz.toml`).
struct DevTarget {
    /// Directory the watcher monitors recursively.
    watch_dir: PathBuf,
    /// Additional args for the `fitz run ...` child.
    child_args: Vec<String>,
    /// Short string for the UX banner ("`./my_app.fitz`" or
    /// "project `myapp`").
    display: String,
}

/// Entry point of the `fitz dev` sub-command (Phase 9.z.3).
///
/// Main loop: spawn a `fitz run <entry>` child, listen for
/// filesystem changes, and on a relevant one (`.fitz` file or
/// `fitz.toml`, not excluded), kill the child and respawn.
/// Ctrl+C kills the child before exiting to avoid zombie
/// processes.
///
/// All logic runs inside a tokio current_thread runtime because
/// it combines `tokio::process` (async kill of the child),
/// `tokio::signal::ctrl_c`, and an async channel for `notify`
/// events (which is sync; we forward them via
/// `std::thread::spawn` + `tokio::sync::mpsc::UnboundedSender`).
fn dev_cmd(file_arg: Option<PathBuf>, bin: Option<String>, port: u16) {
    // Phase 11.13 — wasm-client mode. Manifest mode only (needs
    // `mount` + the fixed output layout), so `--file` always takes
    // the classic respawn path. When the selected bin (`--bin`, or
    // the manifest's only bin) targets `wasm-client`, build the
    // bundle + serve it with live-reload instead of respawning
    // `fitz run`.
    if file_arg.is_none() && is_wasm_client_dev(bin.as_deref()) {
        let resolved = resolve_entry_with_bin(None, bin.as_deref(), None);
        let runtime = evaluator::build_runtime();
        runtime.block_on(async move {
            if let Err(e) = run_wasm_dev_loop(resolved, bin, port).await {
                eprintln!("✗ fitz dev: {e}");
                std::process::exit(1);
            }
        });
        return;
    }

    let target = resolve_dev_target(file_arg, bin);

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

/// Phase 11.13 — cheap manifest peek: does the selected bin (`--bin`,
/// or the manifest's only bin) target `wasm-client`? Returns `false`
/// (→ classic respawn mode) on any resolution error (missing manifest,
/// parse error, unknown bin, or multi-bin ambiguity when no `--bin`)
/// so the classic path keeps its exact behaviour and error messages.
/// With `--bin <name>` a `server` + `web` fullstack project resolves
/// its `web` (wasm) bin here.
fn is_wasm_client_dev(bin: Option<&str>) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    let Some(mp) = manifest::find_manifest(&cwd) else {
        return false;
    };
    let Ok(text) = fs::read_to_string(&mp) else {
        return false;
    };
    let Ok(m) = manifest::Manifest::parse(&text) else {
        return false;
    };
    matches!(
        m.select_bin(bin),
        Ok(Some(b)) if b.effective_target() == manifest::Target::WasmClient
    )
}

/// Phase 11.13 — `fitz dev` wasm-client loop (Approach C). Builds
/// the bundle (`wasm-pack --dev`, incremental), serves the project
/// on `127.0.0.1:<port>` with a live-reload WebSocket, and on each
/// relevant save rebuilds + pushes a reload to the browser. A build
/// failure prints and keeps serving the previous bundle. Ctrl+C
/// exits.
async fn run_wasm_dev_loop(
    mut resolved: ResolvedEntry,
    bin: Option<String>,
    port: u16,
) -> Result<(), String> {
    let watch_dir = resolved
        .manifest_ctx
        .as_ref()
        .map(|c| c.manifest_dir.clone())
        .ok_or_else(|| "wasm-client dev requires a manifest".to_string())?;

    eprintln!("🔄 fitz dev — wasm-client mode");
    eprintln!("   watching {}", watch_dir.display());

    // Initial build (dev profile). If it fails we still start the
    // server so the user can fix and re-save; the browser 404s the
    // bundle until a build succeeds. No client is connected yet, so
    // the initial outcome doesn't gate a reload.
    build_and_report(&resolved);

    // Server params come from the (valid) manifest, independent of
    // whether the first build succeeded.
    let (pkg_name, mount) = wasm_server_params(&resolved)?;
    let pkg_rel_js = format!("target/wasm/{pkg_name}/{pkg_name}.js");

    let server = dev_server::start(
        watch_dir.clone(),
        pkg_name.clone(),
        pkg_rel_js,
        mount.clone(),
        port,
    )
    .await?;
    eprintln!("   serving {}", server.url);
    eprintln!("   (Ctrl+C para salir)\n");

    // Phase 11.13 — server baseline params. The dev_server was started
    // once with these. A live re-resolve can change entry/deps/flags
    // (all picked up by the next build), but pkg_name/mount only change
    // on a bin rename / mount edit, which the running server cannot
    // adopt: the output dir `target/wasm/<pkg>/` and the user's
    // index.html would desync. We compare a re-resolve against these
    // and print a restart note instead of silently drifting (option a).
    let server_pkg = pkg_name;
    let server_mount = mount;

    // Watcher (kept alive by `_watcher` for the loop's lifetime).
    let (_watcher, mut rx) = setup_watcher(&watch_dir)?;

    loop {
        tokio::select! {
            change = wait_for_relevant_change(&mut rx, &watch_dir) => {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    drain_pending(&mut rx),
                )
                .await;
                eprintln!(
                    "\n↻ change in {} — rebuilding ...",
                    relative_to(&change, &watch_dir)
                );
                // Phase 11.13 — live manifest re-resolution. When the
                // saved file is `fitz.toml`, re-resolve so an edited
                // `[bin].main` (new entry), a new `[dependencies]` (new
                // dep_registry), or a changed `[flags]` is picked up
                // without restarting. A broken manifest (mid-edit parse
                // error) keeps the previous resolution and keeps
                // serving; the next valid save recovers.
                if change_is_manifest(&change) {
                    match try_resolve_entry_with_bin(None, bin.as_deref(), None) {
                        Ok(new_resolved) => {
                            if let Ok((new_pkg, new_mount)) = wasm_server_params(&new_resolved) {
                                if new_pkg != server_pkg || new_mount != server_mount {
                                    eprintln!(
                                        "↻ nota: el bin cambió de nombre/mount \
                                         (`{server_pkg}`/`{server_mount}` → `{new_pkg}`/`{new_mount}`).\n   \
                                         Reiniciá `fitz dev` para tomarlo (el bundle servido y la \
                                         host page siguen apuntando al anterior)."
                                    );
                                }
                            }
                            resolved = new_resolved;
                            eprintln!("↻ fitz.toml re-resolved");
                        }
                        Err(e) => {
                            eprintln!(
                                "✗ fitz.toml re-resolve failed:\n   {e}\n   \
                                 keeping the previous resolution — fix and re-save."
                            );
                        }
                    }
                }
                if build_and_report(&resolved) {
                    server.signal_reload();
                    eprintln!("✓ reloaded browser");
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\n👋 Ctrl+C received — exiting");
                return Ok(());
            }
        }
    }
}

/// Phase 11.13 — is this changed path the project's `fitz.toml`? Used
/// by the wasm dev loop to decide whether to re-resolve the manifest
/// (vs a plain `.fitzv`/`.fitz` save that only needs a rebuild).
fn change_is_manifest(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == manifest::MANIFEST_FILE)
        .unwrap_or(false)
}

/// Runs one wasm-client dev build (dev profile) and reports the
/// outcome. Returns `true` on success.
fn build_and_report(resolved: &ResolvedEntry) -> bool {
    match build_wasm_client(resolved, /*release=*/ false) {
        Ok(out) => {
            eprintln!("✓ built `{}` at {}", out.pkg_name, out.pkg_dir.display());
            true
        }
        Err(e) => {
            eprintln!("✗ build failed:\n{e}");
            false
        }
    }
}

/// Derives the dev-server params (pkg name + mount selector) from
/// the resolved manifest bin. Works even when the current build
/// failed, since it reads only the (valid) manifest.
fn wasm_server_params(resolved: &ResolvedEntry) -> Result<(String, String), String> {
    let ctx = resolved
        .manifest_ctx
        .as_ref()
        .ok_or_else(|| "internal: wasm dev without manifest ctx".to_string())?;
    let bin = ctx
        .selected_bin
        .as_ref()
        .ok_or_else(|| "internal: wasm dev without a selected bin".to_string())?;
    let pkg = view::sanitise_wasm_pkg_name(&bin.name);
    let mount = bin
        .mount
        .clone()
        .ok_or_else(|| "internal: wasm-client bin without `mount`".to_string())?;
    Ok((pkg, mount))
}

/// Sets up the `notify` file watcher over `watch_dir` and bridges
/// its sync events to a tokio channel (a `std::thread` forwards
/// them). The returned `RecommendedWatcher` MUST be kept alive by
/// the caller — dropping it stops the watch.
fn setup_watcher(
    watch_dir: &std::path::Path,
) -> Result<
    (
        notify::RecommendedWatcher,
        tokio::sync::mpsc::UnboundedReceiver<notify::Event>,
    ),
    String,
> {
    use notify::Watcher;
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(notify_tx)
        .map_err(|e| format!("could not create the file watcher: {e}"))?;
    watcher
        .watch(watch_dir, notify::RecursiveMode::Recursive)
        .map_err(|e| format!("could not watch `{}`: {e}", watch_dir.display()))?;

    let (tokio_tx, tokio_rx) = tokio::sync::mpsc::unbounded_channel::<notify::Event>();
    std::thread::spawn(move || {
        for event in notify_rx.into_iter().flatten() {
            if tokio_tx.send(event).is_err() {
                break;
            }
        }
    });
    Ok((watcher, tokio_rx))
}

/// Decides which directory to watch + which args to pass to the
/// child. `bin` (`--bin <name>`, Phase 11.13) selects a specific
/// `[[bin]]` in manifest mode — the child becomes `fitz run --bin
/// <name>`; ignored in single-file mode.
fn resolve_dev_target(file_arg: Option<PathBuf>, bin: Option<String>) -> DevTarget {
    if let Some(path) = file_arg {
        // Single-file mode: watch the file's parent, child with
        // `fitz run <file>` (absolute path to avoid issues if
        // the cwd changes).
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

    // Manifest mode: find fitz.toml, watch its directory. Child
    // `fitz run` (with `--bin <name>` if selected) so it
    // re-discovers the manifest on every start (if the user edits
    // `[bin].main`, it's respected).
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("✗ could not read the current directory: {e}");
        std::process::exit(1);
    });
    let manifest_path = match manifest::find_manifest(&cwd) {
        Some(p) => p,
        None => {
            eprintln!(
                "✗ could not find `{}` in `{}` or in parent directories.\n   \
                 Pass an explicit file (`fitz dev --file file.fitz`) or create \
                 a project with `fitz new <name>` / `fitz init`.",
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
    // Parse the package name for the banner.
    let display = match fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|t| manifest::Manifest::parse(&t).ok())
    {
        Some(m) => format!("project `{}`", m.package.name),
        None => format!("project at `{}`", manifest_dir.display()),
    };
    let mut child_args = vec!["run".to_string()];
    if let Some(name) = &bin {
        child_args.push("--bin".to_string());
        child_args.push(name.clone());
    }
    let display = match &bin {
        Some(name) => format!("{display} (bin `{name}`)"),
        None => display,
    };
    DevTarget {
        watch_dir: manifest_dir,
        child_args,
        display,
    }
}

/// Main dev loop: spawns child + listens for changes + Ctrl+C.
/// Each outer-loop iteration = one "run" of the program. When a
/// relevant file changes, kill+respawn. When Ctrl+C arrives, kill
/// the child and return Ok.
async fn run_dev_loop(target: DevTarget) -> Result<(), String> {
    // Sync → async channel for the watcher's events. notify is
    // sync; a std::thread forwards each event to the tokio
    // channel.
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(notify_tx)
        .map_err(|e| format!("could not create the file watcher: {e}"))?;
    use notify::Watcher;
    watcher
        .watch(&target.watch_dir, notify::RecursiveMode::Recursive)
        .map_err(|e| format!("could not watch `{}`: {e}", target.watch_dir.display()))?;

    let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::unbounded_channel::<notify::Event>();
    std::thread::spawn(move || {
        // We ignore watcher errors (`Err`): the OS sometimes
        // emits noise (ephemeral paths, transient permissions)
        // that doesn't affect us. If the tokio channel closes
        // (`send().is_err()`), the consumer died and we exit.
        for event in notify_rx.into_iter().flatten() {
            if tokio_tx.send(event).is_err() {
                break;
            }
        }
    });

    let bin = std::env::current_exe()
        .map_err(|e| format!("could not find the current `fitz` binary: {e}"))?;

    let mut run_count: u32 = 1;
    loop {
        clear_screen_and_banner(&target, run_count);

        // Spawn child with working dir = watch_dir so `fitz run`
        // (without args, manifest mode) finds the manifest.
        // Single-file mode uses the file's absolute path, so the
        // cwd doesn't matter.
        let mut child = match tokio::process::Command::new(&bin)
            .args(&target.child_args)
            .current_dir(&target.watch_dir)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("✗ could not spawn the child: {e}");
                // If we can't spawn, we still want to keep
                // listening in case the user fixes things
                // (missing path, permissions, etc.). Wait for a
                // change + retry.
                drain_until_change(&mut tokio_rx, &target.watch_dir).await;
                continue;
            }
        };

        // Inner loop: wait for a filesystem change, Ctrl+C, or
        // the child exiting.
        let restart = tokio::select! {
            change = wait_for_relevant_change(&mut tokio_rx, &target.watch_dir) => {
                let path = change;
                // Debounce: 100ms channel drain to collapse multiple saves.
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    drain_pending(&mut tokio_rx),
                )
                .await;
                eprintln!(
                    "\n↻ change detected in {} — restarting ...",
                    relative_to(&path, &target.watch_dir)
                );
                true
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\n👋 Ctrl+C received — killing child and exiting");
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Ok(());
            }
            status = child.wait() => {
                // The child finished on its own (short CLI
                // program, type error, etc.). We show the
                // status and wait for a change to restart.
                match status {
                    Ok(s) if s.success() => {
                        eprintln!("\n✓ program finished OK (exit 0) — waiting for changes ...");
                    }
                    Ok(s) => {
                        eprintln!(
                            "\n✗ program finished with error (exit {}) — waiting for changes ...",
                            s.code().unwrap_or(-1)
                        );
                    }
                    Err(e) => {
                        eprintln!("\n✗ error waiting for the child: {e}");
                    }
                }
                drain_until_change(&mut tokio_rx, &target.watch_dir).await;
                eprintln!("\n↻ restarting ...");
                false
            }
        };

        // Kill the child if it was still alive ("restart due to
        // change" case).
        if restart {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        run_count += 1;
    }
}

/// Waits for the next watcher event that touches a relevant file
/// (`.fitz` or `fitz.toml`, not excluded). Irrelevant events are
/// drained silently.
async fn wait_for_relevant_change(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<notify::Event>,
    watch_dir: &std::path::Path,
) -> PathBuf {
    loop {
        let Some(ev) = rx.recv().await else {
            // The channel closed (the watcher thread died). This
            // should NOT happen in normal use; we treat it as a
            // synthetic change so the loop exits.
            return watch_dir.to_path_buf();
        };
        for p in &ev.paths {
            if path_is_relevant(p, watch_dir) {
                return p.clone();
            }
        }
    }
}

/// Drains events from the channel without blocking (poll). Used
/// for debouncing: after detecting ONE event, we drain the ones
/// arriving in the next 100ms to collapse multiple saves.
async fn drain_pending(rx: &mut tokio::sync::mpsc::UnboundedReceiver<notify::Event>) {
    loop {
        match rx.try_recv() {
            Ok(_) => continue,
            Err(_) => {
                // Wait a bit for the next events of the
                // multi-save to arrive (typical in VSCode: write
                // tmp, rename, chmod). The outer timeout caps it
                // at 100ms total.
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                if rx.try_recv().is_err() {
                    return;
                }
            }
        }
    }
}

/// Blocks until a relevant change arrives. "Loop until something
/// happens" variant used when the child finished on its own and
/// we wait for the user's next save.
async fn drain_until_change(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<notify::Event>,
    watch_dir: &std::path::Path,
) {
    let _ = wait_for_relevant_change(rx, watch_dir).await;
    // Post-change debounce.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), drain_pending(rx)).await;
}

/// Decides whether an event path warrants a restart. Rules:
///
/// - Only `.fitz` / `.fitzv` or `fitz.toml` (other extensions are
///   ignored). Phase 11.13 added `.fitzv` so `fitz dev`'s
///   wasm-client mode reacts to single-file component edits (the
///   watcher used to ignore them entirely — `.fitzv` ≠ `.fitz`).
/// - Excludes paths under `target/`, `.git/`, `node_modules/`,
///   `.fitz/`, hidden files (`.something`).
fn path_is_relevant(path: &std::path::Path, watch_dir: &std::path::Path) -> bool {
    // Filename check first (cheaper).
    let is_fitz_file = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| ext == "fitz" || ext == "fitzv")
        .unwrap_or(false);
    let is_manifest = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == manifest::MANIFEST_FILE)
        .unwrap_or(false);
    if !is_fitz_file && !is_manifest {
        return false;
    }

    // Components excluded at any level.
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
        // Any other component starting with `.` is hidden.
        // Except the final file if it's a literal `.fitz`/`.fitzv`
        // — we already checked the extension, so a `.something.fitz`
        // file (hidden with the fitz extension) does trigger.
        // Reasonable.
        if s.starts_with('.')
            && s != "."
            && s != ".."
            && !s.ends_with(".fitz")
            && !s.ends_with(".fitzv")
        {
            return false;
        }
    }
    true
}

/// For UX messages: shows the path relative to watch_dir if it's
/// inside, or as-is if it's outside.
fn relative_to(path: &std::path::Path, base: &std::path::Path) -> String {
    path.strip_prefix(base)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

// ---- Phase 9.z.4 — `fitz repl` (interactive REPL) ----

/// Entry point of the `fitz repl` sub-command (Phase 9.z.4).
/// Opens an interactive prompt where each line is evaluated
/// against a shared env. Supports multi-line continuation when
/// `{`/`(`/`[` is open, special commands with `:` prefix,
/// persistent history in `~/.fitz/history`, and Ctrl+D to exit.
///
/// All logic runs inside a tokio current_thread runtime
/// (`evaluator::build_runtime`) because the evaluator has been
/// async since Phase 6.4 and we need to await `Value::Future` so
/// that `sleep(100).await` and similar things work from the
/// prompt.
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
        // If the file doesn't exist it's OK — first session.
        // Any other error (permissions, corrupt fs) is silently
        // ignored so we don't dirty the startup UX; rustyline
        // still handles the session without persistent history.
        let _ = editor.load_history(p);
    }

    let runtime = evaluator::build_runtime();
    run_on_big_stack(move || {
        runtime.block_on(async move {
            repl_loop(&mut editor, history_path.as_deref()).await;
        });
    });
}

/// Path to the REPL's history file: `~/.fitz/history`. If we
/// can't resolve the home dir (very rare case), we return `None`
/// and the session runs without persistent history.
fn repl_history_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let dir = PathBuf::from(home).join(".fitz");
    // Better try to create the directory here; if it fails, let
    // rustyline handle it in `save_history` (which will also
    // fail but silently).
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("history"))
}

/// Main REPL loop. Each iteration: read one line (or several if
/// it's incomplete), handle `:` special commands, parse,
/// evaluate against the shared env, print the value if it was a
/// top-level expression.
async fn repl_loop(editor: &mut rustyline::DefaultEditor, history_path: Option<&std::path::Path>) {
    let mut env = evaluator::new_repl_env();
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    loop {
        let buffer = match read_complete_input(editor) {
            Ok(b) => b,
            Err(ReplReadError::Interrupted) => {
                // Ctrl+C: clear the multi-line buffer if any, go
                // back to the prompt.
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
        // We add it to history only if it's non-empty.
        // rustyline automatically dedupes against the identical
        // previous line.
        let _ = editor.add_history_entry(buffer.as_str());

        // Special commands: `:help`, `:quit`, `:type`, `:env`,
        // `:reset`, `:load`. If the line starts with `:` (no
        // leading whitespace), we treat it as a command.
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

        // Evaluate as Fitz code. Lexer/parser/checker errors are
        // shown and we return to the prompt without aborting.
        eval_repl_input(&buffer, &mut env, &base_dir).await;
    }
}

/// Result of processing a line with `rustyline`: read OK,
/// Ctrl+C (cancels the multi-line buffer), Ctrl+D (exits), or an
/// unexpected error.
enum ReplReadError {
    Interrupted,
    Eof,
    Other(String),
}

/// Reads a COMPLETE user input: one or more lines until
/// brackets/parens/braces/strings are balanced. Returns the
/// concatenated buffer.
///
/// The prompt changes between lines: `fitz> ` for the first
/// line, `...   ` for continuations. Visually aligned with
/// `fitz>` (4 chars each).
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
                // If it's not complete, keep asking for more
                // lines.
            }
            Err(ReadlineError::Interrupted) => return Err(ReplReadError::Interrupted),
            Err(ReadlineError::Eof) => return Err(ReplReadError::Eof),
            Err(e) => return Err(ReplReadError::Other(format!("{e}"))),
        }
    }
}

/// "Input complete" heuristic: balanced `{`/`(`/`[` + no open
/// string literal. Used for multi-line continuation when the
/// user writes a block (`fn`, `if`, `match`) or complex
/// expression.
///
/// Handles:
/// - String literals `"..."` with `\"` escapes.
/// - Line comments `//` (ignore the rest up to `\n`).
/// - Multi-line comments `/* ... */`.
///
/// It's not a real parser — heuristic enough for multi-line
/// detection. The real parser may still fail with a different
/// syntax error; the REPL shows it and returns to the prompt.
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
                // Line comment: skip up to \n.
                for c2 in chars.by_ref() {
                    if c2 == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                // Block comment: skip up to `*/`.
                chars.next(); // consume the `*`
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

/// Result of a special command: `Continue` returns to the
/// prompt, `Quit` exits the REPL.
enum ReplCommandResult {
    Continue,
    Quit,
}

/// Handles a special command `:name [args]`. The line comes in
/// without the leading `:` (consumed by the caller).
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
                println!("usage: `:load <file.fitz>`");
            } else {
                load_into_repl_env(args, env, base_dir).await;
            }
        }
        other => {
            println!("unknown command `:{other}`. Type `:help` for the list.");
        }
    }
    ReplCommandResult::Continue
}

fn print_repl_help() {
    println!("REPL commands:");
    println!("  :help, :h       — this help");
    println!("  :quit, :q       — exit (also Ctrl+D)");
    println!("  :env            — list variables and fns defined in the scope");
    println!("  :reset          — clear the scope (you lose everything)");
    println!("  :type <expr>    — show the type of an expression");
    println!("  :load <file>    — evaluate a .fitz in the current scope");
}

/// Prints the root scope's variables, excluding builtins
/// (`print`/`len`/etc.) that aren't interesting for the user.
fn print_repl_env(env: &fitz::env::EnvRef) {
    let names = env.lock().local_names();
    let builtins: std::collections::HashSet<&str> =
        evaluator::builtin_names().iter().copied().collect();
    let user_names: Vec<String> = names
        .into_iter()
        .filter(|n| !builtins.contains(n.as_str()))
        .collect();
    if user_names.is_empty() {
        println!("(empty scope — you have not defined anything yet)");
        return;
    }
    println!("Defined in the scope:");
    for name in user_names {
        let value = env.lock().get(&name);
        match value {
            Some(v) => println!("  {} = {}  // {}", name, v, v.type_name()),
            None => println!("  {} = ?", name),
        }
    }
}

/// Implements `:type <expr>`. Parses the expression + checks
/// against existing names in the REPL env, then prints the
/// synthesized type.
///
/// Pragmatic: the checker runs on the whole program (a single
/// `Stmt::Expr`), not on the isolated expression — that lets
/// `:type x + 1` with a previous `x: Int` reflect that the
/// result is `Int`. Implementation: we synthesize a `let
/// __repl_type = <expr>` and ask the checker for the binding's
/// type. The REPL env only matters so the ident `x` isn't
/// missing; the checker rebuilds the bindings from scratch on
/// seeing the program, which is why a previous `let x = "hola"`
/// does not influence this path. Future improvement: feeding the
/// REPL env into the checker.
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
    // The last stmt is `Stmt::Assign` with value = the expr.
    // Its synthesized type is in TypeInfo under the value's
    // span.
    let last = program.last();
    if let Some(fitz::ast::Stmt::Assign { value, .. }) = last {
        let span = value.span();
        if let Some(t) = types.type_at(span) {
            println!(":: {}", t.display(&type_env));
        } else {
            println!(":: <unresolved> (debt: the checker did not record a span)");
        }
    } else {
        println!("✗ could not evaluate the expression");
    }
}

/// Implements `:load <file>`. Reads the file, parses + checks +
/// evaluates against the REPL env. `let`/`fn` defined in the
/// file stay available for subsequent prompt lines.
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
            // L3 (v0.40.0) — en Windows un path estilo Unix (`/tmp/...`) NO
            // es absoluto (le falta el drive), así que `join` lo resuelve
            // contra el drive actual y casi nunca existe. Apuntamos al
            // usuario a la forma portable en vez de dejarlo adivinando.
            #[cfg(windows)]
            if path_str.starts_with('/') && !path_str.starts_with("//") {
                println!(
                    "  nota: en Windows `{path_str}` se resolvió contra el drive \
                     actual (`{}`). Para un path absoluto usá `D:/...`; para que \
                     funcione igual en todos los OS, usá un path relativo al REPL.",
                    resolved.display()
                );
            }
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
    // Same budget as an interactive line: a `:load`ed file that loops
    // forever should abort cleanly instead of hanging the REPL.
    evaluator::install_eval_budget();
    let outcome = evaluator::eval_program_with_env(
        program,
        load_base,
        env.clone(),
        manifest::DepRegistry::new(),
    )
    .await;
    evaluator::uninstall_eval_budget();
    match outcome {
        Ok(_) => println!("✓ cargado {}", resolved.display()),
        Err(e) => println!("✗ {e}"),
    }
}

/// Evaluates the user's input as Fitz code. The program's last
/// stmt, if it's `Stmt::Expr`, is evaluated and returns a
/// `Value` that gets printed (parallel to Python's `_`). For the
/// rest of the stmts (let, fn, etc.) the output is silent.
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
    // Checker in warning mode (parallel to `fitz run
    // --no-typecheck`): the REPL is for experimenting; we prefer
    // the user to see the runtime result even when types are
    // ambiguous. Hard errors (syntax) already cut off above.
    //
    // We filter "variable desconocida" specifically because the
    // checker builds its scope from scratch per line — it
    // ignores vars the user defined on previous lines. The eval
    // against `env` does see them. Without this filter, every
    // `let x = 1; x + 1` emitted a spurious checker warning for
    // `x` on the second line. If the var really doesn't exist,
    // `eval_program_with_env` aborts below with its own error.
    //
    // We filter by substring of the message because every
    // checker error carries `ErrorKind::TypeError` (the
    // `UndefinedVariable` kind belongs to the evaluator). The
    // "variable desconocida" string is hardcoded in
    // `types::infer_expr` and is stable.
    let (_env, _types, _defs, type_errors) = fitz::types::check_program(&program);
    for e in &type_errors {
        if e.message.contains("variable desconocida") {
            continue;
        }
        println!("⚠ {e}");
    }

    // Detect whether the last stmt is `Stmt::Expr` to decide
    // whether to print the result (Python-style). Eval returns
    // the `Value` of the last stmt; we only show it when it came
    // from an expression and isn't Null (print/let/fn return
    // Null and we don't want visual noise).
    let last_is_expr = matches!(program.last(), Some(fitz::ast::Stmt::Expr(_, _)));
    // Fresh resource budget per REPL evaluation: an infinite loop or
    // infinite recursion typed at the prompt should abort with a clear
    // error, not hang the session.
    evaluator::install_eval_budget();
    let outcome = evaluator::eval_program_with_env(
        program,
        base_dir.to_path_buf(),
        env.clone(),
        manifest::DepRegistry::new(),
    )
    .await;
    evaluator::uninstall_eval_budget();
    match outcome {
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

// ---- Phase 9.z.5 — `fitz lint` (linter for patterns beyond types) ----

/// Entry point of the `fitz lint` sub-command. Discovers files
/// (single-file or manifest mode), runs the linter on each one,
/// prints findings in cargo-clippy style, and decides the exit
/// code based on `--deny`.
///
/// Default: exit 0 even with findings (warnings don't break the
/// build). If any finding matches a name listed in `--deny`,
/// exit 1.
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

    // Final summary.
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

/// Prints a finding cargo-clippy-style:
/// ```text
/// warning: variable `x` declared but never used
///   --> src/main.fitz:3:5
///   = note: if intentional, prefix it with `_` ...
/// ```
/// With `--deny <name>`, "error:" red is used instead of
/// "warning:" yellow.
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
            println!("  \x1b[2m= note:\x1b[0m {}", hint);
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
            println!("  = note: {}", hint);
        }
    }
}

/// UX banner when starting / restarting the child. Clears the
/// screen (ANSI `\x1b[2J\x1b[H`) if stdout is a TTY, otherwise
/// just separates with lines. Then prints the run number + the
/// target.
fn clear_screen_and_banner(target: &DevTarget, run_count: u32) {
    use std::io::IsTerminal;
    let use_ansi = std::io::stdout().is_terminal();
    if use_ansi {
        // `\x1b[2J` clears the screen, `\x1b[H` moves the cursor
        // to (1,1). Sufficient on modern terminals (cmd,
        // PowerShell, Windows Terminal, bash, zsh, fish).
        print!("\x1b[2J\x1b[H");
    } else {
        println!("\n----------------------------------------");
    }
    eprintln!("▶ fitz dev (run #{}) — {}", run_count, target.display);
    eprintln!();
}

// =================================================================
// Phase 12.4 — `fitz docker init` handler
// =================================================================

/// `fitz docker init [--force]` — generates Dockerfile +
/// .dockerignore + docker-compose.yml in the manifest directory.
/// Detects the program shape (HTTP port, DB usage) by reading the
/// AST of the entry point declared in `[bin].main`.
///
/// Exit codes:
///   - 0: success (at least one file written or everything
///     skipped because it exists + no `--force`).
///   - 1: I/O error / missing manifest / broken .fitz parse.
fn docker_init_cmd(force: bool) {
    // We reuse `resolve_entry(None)` which walks upwards looking
    // for `fitz.toml`. With no manifest, it aborts with a clear
    // message.
    let resolved = resolve_entry(None);
    let ctx = match resolved.manifest_ctx {
        Some(c) => c,
        None => {
            // `resolve_entry` only returns `None` when we pass
            // an explicit file, but `docker init` does not
            // accept a file arg — this branch is defensive.
            eprintln!(
                "✗ `fitz docker init` requires a Fitz project with `fitz.toml`. \
                 Create one with `fitz new <name>` or `fitz init` first."
            );
            std::process::exit(1);
        }
    };

    let source = fs::read_to_string(&resolved.entry).unwrap_or_else(|e| {
        eprintln!(
            "✗ could not read the entry point `{}`: {e}",
            resolved.entry.display(),
        );
        std::process::exit(1);
    });

    let tokens = lexer::tokenize(&source).unwrap_or_else(|e| {
        eprintln!("✗ `{}`: {e}", resolved.entry.display());
        std::process::exit(1);
    });

    let program = parser::parse(tokens).unwrap_or_else(|e| {
        eprintln!("✗ `{}`: {e}", resolved.entry.display());
        std::process::exit(1);
    });

    let shape = docker::detect_shape(&program, ctx.manifest.package.name.clone());

    let result = docker::init(&ctx.manifest_dir, &shape, force).unwrap_or_else(|e| {
        eprintln!("✗ could not generate the Docker files: {e}");
        std::process::exit(1);
    });

    println!(
        "▶ fitz docker init — project `{}` at `{}`",
        shape.package_name,
        ctx.manifest_dir.display(),
    );
    if let Some(port) = shape.server_port {
        println!("   detected: @server(port = {})", port);
    } else {
        println!("   detected: CLI program (no @server)");
    }
    if shape.uses_db {
        println!("   detected: DB usage (db.X(...)) → compose adds postgres:16-alpine");
    }
    if shape.uses_python {
        println!(
            "   detected: Python interop → runtime falls back to python:3.12-slim-bookworm \
             (libpython3.12 + wget)"
        );
    }
    if shape.uses_cron {
        println!("   detected: @cron → compose adds restart: unless-stopped");
    }
    println!();

    for path in &result.written {
        let rel = path
            .strip_prefix(&ctx.manifest_dir)
            .unwrap_or(path.as_path());
        println!("✓ wrote: {}", rel.display());
    }
    for path in &result.skipped {
        let rel = path
            .strip_prefix(&ctx.manifest_dir)
            .unwrap_or(path.as_path());
        println!(
            "- skipped (already exists, pass --force to overwrite): {}",
            rel.display()
        );
    }

    if !result.skipped.is_empty() && !force {
        println!();
        println!(
            "Tip: to replace existing files run \
             `fitz docker init --force`."
        );
    }
}

/// `fitz docker build [--tag X]` — thin wrapper over `docker
/// build` with the manifest's `package.name` as the default tag.
/// Aborts if there's no `fitz.toml` above cwd, if there's no
/// `Dockerfile` in `manifest_dir`, or if `docker build` fails
/// (propagates the exit code).
fn docker_build_cmd(tag: Option<String>) {
    let resolved = resolve_entry(None);
    let ctx = match resolved.manifest_ctx {
        Some(c) => c,
        None => {
            eprintln!(
                "✗ `fitz docker build` requires a Fitz project with `fitz.toml`. \
                 Create one with `fitz new <name>` or `fitz init` first."
            );
            std::process::exit(1);
        }
    };

    let dockerfile_path = ctx.manifest_dir.join("Dockerfile");
    if !dockerfile_path.is_file() {
        eprintln!(
            "✗ `Dockerfile` not found in `{}`. Run `fitz docker init` to \
             generate it.",
            ctx.manifest_dir.display(),
        );
        std::process::exit(1);
    }

    let tag = tag.unwrap_or_else(|| format!("{}:latest", ctx.manifest.package.name));

    println!(
        "▶ fitz docker build — tag `{}` at `{}`",
        tag,
        ctx.manifest_dir.display(),
    );

    let status = std::process::Command::new("docker")
        .arg("build")
        .arg("-t")
        .arg(&tag)
        .arg(".")
        .current_dir(&ctx.manifest_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("✓ build OK — `{}`", tag);
        }
        Ok(s) => {
            // `docker build` already wrote its error to stderr;
            // we exit with its exit code so CI captures it too.
            std::process::exit(s.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!(
                "✗ could not invoke `docker build`: {e}. Is Docker installed and \
                 running? (`docker --version` to verify)"
            );
            std::process::exit(1);
        }
    }
}

// =================================================================
// Phase 12.6 — `fitz deploy <subcommand>` handlers
// =================================================================

/// `fitz deploy docker [--tag X] [--no-push]` — Docker image
/// build + push. Thin wrapper over `docker build/push` that takes
/// the tag from `package.name` by default.
fn deploy_docker_cmd(tag: Option<String>, no_push: bool) {
    let resolved = resolve_entry(None);
    let ctx = match resolved.manifest_ctx {
        Some(c) => c,
        None => {
            eprintln!(
                "✗ `fitz deploy docker` requires a Fitz project with `fitz.toml`. \
                 Create one with `fitz new <name>` or `fitz init` first."
            );
            std::process::exit(1);
        }
    };

    let options = deploy::DeployOptions {
        tag,
        no_push,
        no_detach: false,
        no_build: false,
    };

    println!(
        "▶ fitz deploy docker — project `{}` at `{}`",
        ctx.manifest.package.name,
        ctx.manifest_dir.display(),
    );

    match deploy::run_deploy(
        deploy::DeployTarget::Docker,
        &ctx.manifest,
        &ctx.manifest_dir,
        &options,
    ) {
        Ok(result) => {
            println!();
            println!(
                "✓ deploy OK — {} command(s) executed",
                result.commands.len()
            );
            for cmd in &result.commands {
                println!("  - {} {}", cmd.bin, cmd.args.join(" "));
            }
        }
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    }
}

/// `fitz deploy compose [--no-detach] [--no-build]` — `docker
/// compose up` with configurable flags.
fn deploy_compose_cmd(no_detach: bool, no_build: bool) {
    let resolved = resolve_entry(None);
    let ctx = match resolved.manifest_ctx {
        Some(c) => c,
        None => {
            eprintln!(
                "✗ `fitz deploy compose` requires a Fitz project with `fitz.toml`. \
                 Create one with `fitz new <name>` or `fitz init` first."
            );
            std::process::exit(1);
        }
    };

    let options = deploy::DeployOptions {
        tag: None,
        no_push: false,
        no_detach,
        no_build,
    };

    println!(
        "▶ fitz deploy compose — project `{}` at `{}`",
        ctx.manifest.package.name,
        ctx.manifest_dir.display(),
    );

    match deploy::run_deploy(
        deploy::DeployTarget::Compose,
        &ctx.manifest,
        &ctx.manifest_dir,
        &options,
    ) {
        Ok(result) => {
            println!();
            println!(
                "✓ deploy OK — {} command(s) executed",
                result.commands.len()
            );
            for cmd in &result.commands {
                println!("  - {} {}", cmd.bin, cmd.args.join(" "));
            }
        }
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    }
}

// =================================================================
// Phase 10.6 — `fitz db <subcommand>` handlers
// =================================================================

/// Resolves the PG connection URL: explicit > `DATABASE_URL` env.
fn resolve_db_url(explicit: Option<String>) -> Result<String, String> {
    match explicit {
        Some(u) => Ok(u),
        None => std::env::var("DATABASE_URL").map_err(|_| {
            "Postgres URL not provided. Pass `--url postgres://...` or set the `DATABASE_URL` env var."
                .to_string()
        }),
    }
}

/// Resolves the migrations dir: explicit > `./migrations` (cwd).
fn resolve_migrations_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| PathBuf::from("migrations"))
}

/// Resolves the .fitz entry: explicit > manifest's `[bin].main`.
fn resolve_db_entry(file: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(f) = file {
        return Ok(f);
    }
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let manifest_path = manifest::find_manifest(&cwd).ok_or_else(|| {
        "could not find the entry — pass `<file.fitz>` or make sure you are in a project with `fitz.toml`".to_string()
    })?;
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading `{}`: {e}", manifest_path.display()))?;
    let m = manifest::Manifest::parse(&manifest_text).map_err(|e| format!("manifest: {e}"))?;
    // Phase 11.5.b — multi-bin manifests pass `--file` explicitly
    // in `fitz db`; here we accept the first `[[bin]]` entry as a
    // convenience for single-bin projects. If more granularity is
    // needed later (`--bin` for `fitz db`), it lands on demand.
    let bin_main = m.bins.first().map(|b| b.main.clone()).ok_or_else(|| {
        "the manifest has no `[bin].main` — pass an explicit `<file.fitz>`".to_string()
    })?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| "manifest has no parent".to_string())?
        .to_path_buf();
    Ok(manifest_dir.join(bin_main))
}

/// Reads + parses + checks a Fitz program, returns (Program,
/// TypeEnv). Used by `db diff` to build the expected schema.
fn load_program_for_db(entry: &std::path::Path) -> Result<(ast::Program, types::TypeEnv), String> {
    let src = std::fs::read_to_string(entry)
        .map_err(|e| format!("reading `{}`: {e}", entry.display()))?;
    let tokens = lexer::tokenize(&src).map_err(|e| format!("lexer: {e}"))?;
    let program = parser::parse(tokens).map_err(|e| format!("parser: {e}"))?;
    let (env, _type_info, _def_info, errs) = types::check_program(&program);
    if !errs.is_empty() {
        return Err(format!(
            "type checker ({} errors). Run `fitz check` for details.",
            errs.len()
        ));
    }
    Ok((program, env))
}

fn db_diff_cmd(
    file: Option<PathBuf>,
    url: Option<String>,
    out: Option<PathBuf>,
    check_destructive: bool,
    allow_destructive: bool,
) {
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
            eprintln!("✗ loading program: {e}");
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
    // A single tokio runtime for connect + introspect: the
    // connection sets up `tokio::spawn(health_check_task)`; if
    // we drop the runtime between connect and query, that task
    // dies and the next query breaks with "cstr is not UTF-8"
    // or similar.
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
        eprintln!("✓ schema in sync — no pending changes");
        return;
    }
    // v0.37.16 — probable-rename safety net. A rename WITHOUT
    // `@renamed_from` is emitted as DROP + ADD of the same type,
    // which loses the column's data silently. Surface it on EVERY
    // path (not just `--check-destructive`) so the user can annotate
    // before applying the SQL. Non-blocking (a genuine drop+add of
    // the same type is a legit — if noisy — false positive).
    let rename_hints = migrations::detect_probable_renames(&changes, &current);
    if !rename_hints.is_empty() {
        eprintln!(
            "⚠ possible column rename(s) — a DROP + ADD of the same type LOSES the column's data:"
        );
        for h in &rename_hints {
            eprintln!(
                "    • {}: `{}` dropped, `{}` added (both {}). If this is a rename, add \
                 `@renamed_from(\"{}\")` to the `{}` field for a safe \
                 `ALTER TABLE ... RENAME COLUMN` (preserves data).",
                h.table_display, h.old_column, h.new_column, h.sql_type, h.old_column, h.new_column
            );
        }
    }
    // v0.10.31 (Tier A.1) — classification + guard. Aborts if
    // there are destructive changes and `--allow-destructive`
    // was not passed.
    if check_destructive {
        let (safe, risky, destructive) = migrations::count_by_severity(&changes);
        eprintln!(
            "→ classification: {} safe, {} risky, {} destructive",
            safe, risky, destructive
        );
        if destructive > 0 && !allow_destructive {
            eprintln!(
                "✗ {} destructive change(s) detected — rejected by `--check-destructive`",
                destructive
            );
            eprintln!("  List of destructive changes:");
            for c in &changes {
                if c.severity() == migrations::Severity::Destructive {
                    eprintln!("    • {}", change_short_label_for_cli(c));
                }
            }
            eprintln!("  To emit anyway: re-run with `--allow-destructive`.");
            eprintln!("  For a safe refactor: mark renames with `@renamed_from(\"old\")`.");
            std::process::exit(1);
        }
    }
    // SQL: with --check-destructive we enrich with per-change comments.
    let sql = if check_destructive {
        migrations::changes_to_sql_with_severity(&changes)
    } else {
        migrations::changes_to_sql(&changes)
    };
    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &sql) {
                eprintln!("✗ writing `{}`: {e}", path.display());
                std::process::exit(1);
            }
            eprintln!("✓ {} change(s) → {}", changes.len(), path.display());
        }
        None => {
            print!("{}", sql);
            eprintln!("✓ {} change(s) emitted to stdout", changes.len());
        }
    }
}

/// v0.10.31 (Tier A.1) — short CLI label mirrored from
/// `change_short_label` in migrations.rs. Lives here because
/// migrations' fn is `pub(crate)` and we can't access it from
/// main.rs without re-exposing it. Keep in sync with
/// `change_short_label` in migrations.rs if it changes.
fn change_short_label_for_cli(c: &migrations::Change) -> String {
    use migrations::Change;
    match c {
        Change::DropTable(tr) => format!("DropTable {}", tr.name),
        Change::DropColumn { table, column } => {
            format!("DropColumn {} from {}", column, table.name)
        }
        // The guard only lists Destructive; nothing else should
        // fall here.
        other => format!("{:?}", other),
    }
}

/// v0.10.28 (Tier S, sub-step 1) — `fitz db inspect`. Connects
/// to the DB, runs `introspect_schema` and emits the report in
/// plain text (default) or JSON (with `--json`). Does NOT touch
/// the Fitz program — pure introspection of the DB's real state.
fn db_inspect_cmd(
    url: Option<String>,
    schema: Option<String>,
    table: Option<String>,
    json: bool,
    all_schemas: bool,
) {
    let url = match resolve_db_url(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let rt = evaluator::build_runtime();
    let result = rt.block_on(async {
        let conn = db::connect_url(&url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        migrations::introspect_schema(&conn)
            .await
            .map_err(|e| format!("introspect: {e}"))
    });
    let schema_introspected = match result {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if json {
        let result = if all_schemas {
            migrations::format_inspection_json_all_schemas(&schema_introspected, table.as_deref())
        } else {
            migrations::format_inspection_json(
                &schema_introspected,
                schema.as_deref(),
                table.as_deref(),
            )
        };
        match result {
            Ok(j) => {
                println!("{j}");
            }
            Err(e) => {
                eprintln!("✗ format json: {e}");
                std::process::exit(1);
            }
        }
    } else {
        let text = if all_schemas {
            migrations::format_inspection_text_all_schemas(&schema_introspected, table.as_deref())
        } else {
            migrations::format_inspection_text(
                &schema_introspected,
                schema.as_deref(),
                table.as_deref(),
            )
        };
        print!("{text}");
    }
}

fn db_migrate_cmd(url: Option<String>, dir: Option<PathBuf>, dry_run: bool, sql: bool) {
    if dry_run && sql {
        eprintln!("✗ `--dry-run` y `--sql` son mutuamente excluyentes");
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
    if migrations_list.is_empty() {
        eprintln!(
            "✓ no hay archivos `.sql`/`.fitz` en `{}` — nada para migrar",
            dir.display()
        );
        return;
    }
    let rt = evaluator::build_runtime();
    // v0.10.20 — Offline SQL mode: emits the pending SQL to
    // stdout instead of running it. Still connects to read
    // _fitz_migrations (what's applied) and skip those.
    if sql {
        let result = rt.block_on(async {
            let conn = db::connect_url(&url)
                .await
                .map_err(|e| format!("connect: {e}"))?;
            migrations::applied_versions(&conn)
                .await
                .map_err(|e| format!("applied_versions: {e}"))
        });
        let applied = match result {
            Ok(a) => a,
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        };
        let applied_set: std::collections::HashSet<&str> =
            applied.iter().map(|s| s.as_str()).collect();
        let mut emitted = 0;
        for m in &migrations_list {
            if applied_set.contains(m.version.as_str()) {
                continue;
            }
            match &m.kind {
                migrations::MigrationKind::Sql { up_sql, .. } => {
                    println!("-- migration {}: {}", m.version, m.filename);
                    print!("{}", up_sql);
                    if !up_sql.ends_with('\n') {
                        println!();
                    }
                    println!();
                    emitted += 1;
                }
                migrations::MigrationKind::Fitz { .. } => {
                    eprintln!(
                        "✗ `{}` es una `.fitz` data migration — NO se puede materializar \
                         como SQL offline. Ejecutala via `fitz db migrate` directo \
                         (sin --sql) contra la DB target.",
                        m.filename
                    );
                    std::process::exit(1);
                }
            }
        }
        eprintln!(
            "✓ {} migration(s) emitida(s) al stdout (no se aplicaron)",
            emitted
        );
        return;
    }
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
    // v0.10.19 — per-migration dispatch by kind: .sql via
    // `apply_migration` (raw SQL inside a tx), .fitz via
    // `apply_fitz_migration_blocking` (parses + invokes async
    // fn migrate(db) + INSERT into _fitz_migrations at the
    // end).
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
    // Timestamp `YYYYMMDDHHMMSS` UTC + sanitized name.
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
        eprintln!("✗ already exists: `{}`", path.display());
        std::process::exit(1);
    }
    let stub = format!(
        "-- Migration: {sanitized}\n\
         -- Created: {iso}\n\
         --\n\
         -- Edit this file with the SQL statements you need.\n\
         -- Tip: use `fitz db diff > {filename}` to generate the SQL\n\
         -- automatically from the program's `@table` types.\n\n\
         -- UP\n\
         \n\n\
         -- DOWN\n\
         \n",
        iso = now.to_rfc3339(),
    );
    if let Err(e) = std::fs::write(&path, &stub) {
        eprintln!("✗ writing `{}`: {e}", path.display());
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
    // v0.10.19 — per-migration dispatch by kind (parallel to migrate).
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
        eprintln!("✓ no applied migrations to revert");
    } else {
        eprintln!("✓ {} migration(s) reverted:", reverted.len());
        for v in &reverted {
            eprintln!("    {v}");
        }
    }
}

/// v0.10.19 — Variant of `rollback_n` with kind-dispatch: .sql
/// via `migrations::revert_migration` (DOWN SQL inside a tx),
/// .fitz via `revert_fitz_migration_async` (parses + invokes
/// async fn rollback(db) + DELETE from _fitz_migrations). Same
/// pre-flight as `rollback_n` (every target has a file + a
/// resolvable rollback path) before touching the DB.
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
        // Reusing applied_versions_desc would require exposing
        // it; instead we read applied (ASC) and reverse it.
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
    // Pre-flight: every target must have file + a resolvable path.
    for v in &target_versions {
        let m = by_version.get(v.as_str()).ok_or_else(|| {
            format!(
                "rollback: version `{v}` is applied in the DB but \
                 there is NO file in the migrations dir. Restore the \
                 file or stamp it manually."
            )
        })?;
        match &m.kind {
            migrations::MigrationKind::Sql { down_sql, .. } => {
                if down_sql.is_none() {
                    return Err(format!(
                        "rollback: migration `{}` has no `-- DOWN` section — \
                         it cannot be reverted. Edit the file adding \
                         `-- DOWN` with the inverse SQL and retry.",
                        m.filename
                    ));
                }
            }
            migrations::MigrationKind::Fitz { source, .. } => {
                if !fitz_migration_has_rollback(source) {
                    return Err(format!(
                        "rollback: migration `{}` (`.fitz`) does not declare \
                         `async fn rollback(db: DbConn) -> Result<Null>`. \
                         Add the fn to the file and retry.",
                        m.filename
                    ));
                }
            }
        }
    }
    let mut reverted = Vec::with_capacity(target_versions.len());
    for v in target_versions {
        let m = by_version.get(v.as_str()).expect("pre-flight validated");
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

/// v0.10.19 (10.6.d) — Runner for a migration's `.fitz` script.
/// File convention:
///
/// ```ignore
/// async fn migrate(db: DbConn) -> Result<Null> {
///     // back-fill, parsing old JSON, etc.
///     return Ok(null)
/// }
///
/// // Optional, only required if you want `fitz db rollback` to
/// // work:
/// async fn rollback(db: DbConn) -> Result<Null> {
///     return Ok(null)
/// }
/// ```
///
/// The runner:
/// 1. Parses the file and verifies it declares `migrate`.
/// 2. Injects `db` as a pre-bound var in the env.
/// 3. Appends a synthetic stmt to the program: `let
///    __fitz_mig_result = migrate(db).await`.
/// 4. Evaluates with `evaluator::eval_program_with_env`.
/// 5. If the last value is `Result::Ok(_)` → tracks the
///    migration as applied.
/// 6. If it's `Result::Err(msg)` → error without tracking.
///
/// **Atomicity**: wrapping in a tx is the user's responsibility
/// (typically with `return db.transaction(fn(tx) -> Result<Null>
/// { ... }).await`). The runner does NOT wrap automatically
/// because the `Value::DbConn` is passed raw; the user decides
/// the granularity.
async fn apply_fitz_migration_async(
    conn: &std::sync::Arc<db::DbConnHandle>,
    version: &str,
    filename: &str,
    path: &std::path::Path,
    source: &str,
) -> Result<(), String> {
    run_fitz_migration_callback(conn, path, source, "migrate").await?;
    // If the callback returned Ok, mark it as applied. This
    // INSERT is NOT inside the user's tx — if the user
    // committed their changes inside the `.fitz`, they
    // already persisted; this tracking is separate.
    migrations::track_fitz_migration_applied(conn, version)
        .await
        .map_err(|e| format!("track aplicada: {e}"))?;
    let _ = filename; // available for future messages if demand appears
    Ok(())
}

/// v0.10.19 — Analogue of `apply_fitz_migration_async` for
/// rollback: invokes `async fn rollback(db)` + deletes the
/// record.
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

/// Shared helper for `apply_fitz_migration_async` and
/// `revert_fitz_migration_async`. Parses the file, checks that
/// `fn_name` is declared as `async fn(db: DbConn) -> ...`,
/// creates an env with `db` pre-bound to `Value::DbConn(conn)`,
/// appends `let __fitz_mig_result = <fn_name>(db).await` and
/// evaluates with `evaluator::eval_program_with_env`. Inspects
/// the last value: `Result::Ok(_)` → Ok(()); `Result::Err(msg)`
/// → Err(msg).
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
    // 2. Check that `fn_name` is declared as an async fn.
    let has_callback = program.iter().any(|stmt| matches!(stmt, ast::Stmt::FnDef { name, is_async, .. } if name == fn_name && *is_async));
    if !has_callback {
        return Err(format!(
            "el archivo no declara `async fn {fn_name}(db: DbConn) -> Result<Null>`. \
             El runner de `.fitz` migrations espera esa fn como entry point."
        ));
    }
    // 3. Type-check (warning-only, parallel to `fitz run`
    //    permissive).
    let (_env, _ti, _di, _errs) = types::check_program(&program);
    // 4. Append synthetic stmt: `let __fitz_mig_result = <fn>(db).await`.
    let call_expr = ast::Expr::Call {
        callee: Box::new(ast::Expr::Ident(fn_name.to_string(), ast::Span::ZERO)),
        args: vec![ast::Expr::Ident("db".to_string(), ast::Span::ZERO)],
        span: ast::Span::ZERO,
    };
    let await_expr = ast::Expr::Await(Box::new(call_expr), ast::Span::ZERO);
    let assign_stmt = ast::Stmt::Assign {
        target: ast::AssignTarget::Ident("__fitz_mig_result".to_string(), ast::Span::ZERO),
        type_: None,
        value: await_expr,
        is_let: true,
        span: ast::Span::ZERO,
    };
    program.push(assign_stmt);
    // 5. Fresh env with builtins + db pre-bound. `new_repl_env()`
    //    builds an EnvRef with builtins registered — same scope
    //    as a typical `fitz run` script.
    let env = evaluator::new_repl_env();
    // Inject db as Value::DbConn(Arc<DbConnHandle>).
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
    // 7. Inspect `__fitz_mig_result`.
    // The last stmt is the `let __fitz_mig_result = ...`
    // whose return value from eval_program_with_env is Null
    // (assignment). We read the binding directly from the env
    // to get the Value.
    let _ = last;
    let result = env
        .lock()
        .get("__fitz_mig_result")
        .ok_or_else(|| "internal: __fitz_mig_result was not bound".to_string())?;
    match result {
        Value::Result(variant) => match variant {
            fitz::value::ResultVariant::Ok(_) => Ok(()),
            fitz::value::ResultVariant::Err(e) => Err(format!("`{fn_name}` returned Err: {}", *e)),
        },
        other => Err(format!(
            "`async fn {fn_name}` must return Result<Null>, received: {other}"
        )),
    }
}

/// Simple heuristic (based on parse) to detect whether the
/// `.fitz` migration declares `async fn rollback(db: DbConn)`.
/// Used by `rollback_n_dispatch` in the pre-flight check BEFORE
/// touching the DB, to fail fast with a clear message.
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
            eprintln!("✗ loading program: {e}");
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
        eprintln!("✓ schema in sync — declared schema matches the DB");
        std::process::exit(0);
    }
    // Drift detected: pending SQL to stderr (visible in CI
    // logs), count to stdout (parseable), exit 1.
    let sql = migrations::changes_to_sql(&changes);
    eprintln!("✗ drift detected — {} pending change(s):", changes.len());
    eprintln!();
    eprintln!("{}", sql);
    eprintln!(
        "💡 run `fitz db diff > migrations/<file>.sql` + `fitz db migrate` \
         to sync."
    );
    std::process::exit(1);
}

fn db_stamp_cmd(version: Option<String>, all: bool, url: Option<String>, dir: Option<PathBuf>) {
    // clap guarantees version XOR all (conflicts_with), but
    // we validate that at least ONE is present.
    if version.is_none() && !all {
        eprintln!(
            "✗ `fitz db stamp` requires `<version>` or `--all`. \
             Examples:\n  fitz db stamp 20260530120000\n  fitz db stamp --all"
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
    let version = version.unwrap(); // guaranteed by the check above
                                    // Warn if the version is not in the dir (can be
                                    // intentional — adopting legacy versions that
                                    // don't exist as files — but typically a typo).
    let in_dir = migrations_list.iter().any(|m| m.version == version);
    if !in_dir {
        eprintln!(
            "⚠ version `{version}` does NOT exist in `{}` — stamped anyway, \
             but make sure it is not a typo.",
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
        eprintln!("✓ no-op: version `{version}` was already applied");
    }
}

fn db_history_cmd(url: Option<String>, dir: Option<PathBuf>) {
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
        migrations::history(&conn, &dir)
            .await
            .map_err(|e| format!("history: {e}"))
    });
    let entries = match result {
        Ok(e) => e,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    if entries.is_empty() {
        eprintln!("✓ no migrations applied yet");
        return;
    }
    println!("{:<20} {:<32} filename", "version", "applied_at");
    println!("{:-<20} {:-<32} {:-<40}", "", "", "");
    for e in &entries {
        let filename = e.filename.as_deref().unwrap_or("(file removed)");
        println!("{:<20} {:<32} {}", e.version, e.applied_at, filename);
    }
    println!("\n{} migration(s) applied.", entries.len());
}

fn db_squash_cmd(
    from: String,
    to: String,
    url: Option<String>,
    dir: Option<PathBuf>,
    no_tracking: bool,
) {
    let dir = resolve_migrations_dir(dir);
    let migrations_list = match migrations::read_migrations_dir(&dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    // 1. Locate the migrations in the range (inclusive).
    let from_idx = migrations_list.iter().position(|m| m.version == from);
    let to_idx = migrations_list.iter().position(|m| m.version == to);
    let (from_idx, to_idx) = match (from_idx, to_idx) {
        (Some(f), Some(t)) => (f, t),
        _ => {
            eprintln!(
                "✗ squash: versions `{from}` and/or `{to}` not found in `{}`",
                dir.display()
            );
            std::process::exit(1);
        }
    };
    if from_idx > to_idx {
        eprintln!(
            "✗ squash: `from` (`{from}`) comes AFTER `to` (`{to}`) in chronological order. \
             Pass them in order."
        );
        std::process::exit(1);
    }
    let range = &migrations_list[from_idx..=to_idx];
    if range.len() < 2 {
        eprintln!("✗ squash: range has <2 migrations — nothing to combine");
        std::process::exit(1);
    }
    // 2. Reject .fitz in the range (squashing only SQL in MVP).
    for m in range {
        if m.is_fitz() {
            eprintln!(
                "✗ squash: `{}` is a `.fitz` data migration — squashing only supports `.sql` \
                 in MVP. Exclude the `.fitz` from the range or apply it manually.",
                m.filename
            );
            std::process::exit(1);
        }
    }
    // 3. Build concatenated UP + DOWN.
    let mut up_parts: Vec<String> = Vec::with_capacity(range.len());
    let mut down_parts: Vec<String> = Vec::with_capacity(range.len());
    let mut all_have_down = true;
    for m in range {
        match &m.kind {
            migrations::MigrationKind::Sql { up_sql, down_sql } => {
                up_parts.push(format!(
                    "-- ↓ from {} ({})\n{}",
                    m.version,
                    m.filename,
                    up_sql.trim_end()
                ));
                match down_sql {
                    Some(d) => down_parts.push(format!(
                        "-- ↑ from {} ({})\n{}",
                        m.version,
                        m.filename,
                        d.trim_end()
                    )),
                    None => all_have_down = false,
                }
            }
            migrations::MigrationKind::Fitz { .. } => unreachable!("filtered above"),
        }
    }
    // DOWN goes in reverse order (reverts newest first).
    down_parts.reverse();
    // 4. Build the squashed file.
    let squashed_filename = format!("{from}_squashed.sql");
    let squashed_path = dir.join(&squashed_filename);
    if squashed_path.exists() {
        eprintln!(
            "✗ squash: `{}` already exists. Delete it or use a different `from` before re-squashing.",
            squashed_path.display()
        );
        std::process::exit(1);
    }
    let now = chrono::Utc::now();
    let mut squashed = String::new();
    squashed.push_str(&format!(
        "-- Squashed: combination of {} migrations in range [{}, {}]\n",
        range.len(),
        from,
        to
    ));
    squashed.push_str(&format!("-- Created: {}\n", now.to_rfc3339()));
    squashed.push_str("-- Generated by `fitz db squash`. The original files were moved\n");
    squashed.push_str("-- to `migrations/squashed/`. Do NOT edit this file by hand unless\n");
    squashed.push_str("-- you know what you are doing — re-running `fitz db squash` is\n");
    squashed.push_str("-- not idempotent on an already-squashed dir.\n");
    squashed.push('\n');
    squashed.push_str("-- UP\n");
    squashed.push_str(&up_parts.join("\n\n"));
    squashed.push('\n');
    if all_have_down {
        squashed.push_str("\n-- DOWN\n");
        squashed.push_str(&down_parts.join("\n\n"));
        squashed.push('\n');
    } else {
        squashed.push_str("\n-- (no -- DOWN section because at least one migration in the range\n");
        squashed.push_str("-- had no DOWN. If you want rollback, add the section by hand.)\n");
    }
    // 5. Move the original files to `migrations/squashed/`.
    let squashed_dir = dir.join("squashed");
    if let Err(e) = std::fs::create_dir_all(&squashed_dir) {
        eprintln!("✗ squash: creating `{}`: {e}", squashed_dir.display());
        std::process::exit(1);
    }
    for m in range {
        let src = dir.join(&m.filename);
        let dst = squashed_dir.join(&m.filename);
        if let Err(e) = std::fs::rename(&src, &dst) {
            eprintln!(
                "✗ squash: moving `{}` → `{}`: {e}",
                src.display(),
                dst.display()
            );
            std::process::exit(1);
        }
    }
    // 6. Write the squashed file.
    if let Err(e) = std::fs::write(&squashed_path, &squashed) {
        eprintln!("✗ squash: writing `{}`: {e}", squashed_path.display());
        std::process::exit(1);
    }
    // 7. Tracking: if any in the range was applied, delete all
    // from the tracking and stamp only `from` (the new
    // squashed one).
    if !no_tracking {
        let url = match resolve_db_url(url) {
            Ok(u) => u,
            Err(e) => {
                eprintln!(
                    "⚠ squash: squashed file created but tracking could not be updated: {e}. \
                     Re-run with `--url <url>` or set DATABASE_URL to sync."
                );
                std::process::exit(1);
            }
        };
        let rt = evaluator::build_runtime();
        let target_versions: Vec<String> = range.iter().map(|m| m.version.clone()).collect();
        let result = rt.block_on(async {
            let conn = db::connect_url(&url)
                .await
                .map_err(|e| format!("connect: {e}"))?;
            let applied = migrations::applied_versions(&conn)
                .await
                .map_err(|e| format!("applied_versions: {e}"))?;
            let any_in_range = applied
                .iter()
                .any(|v| target_versions.iter().any(|tv| tv == v));
            if !any_in_range {
                return Ok::<_, String>(false); // nothing to update
            }
            // Delete every one in the range that's applied.
            for v in &target_versions {
                if applied.iter().any(|a| a == v) {
                    migrations::untrack_fitz_migration(&conn, v)
                        .await
                        .map_err(|e| format!("untrack {v}: {e}"))?;
                }
            }
            // Insert the squashed (points to the `from` version).
            migrations::stamp_version(&conn, &target_versions[0])
                .await
                .map_err(|e| format!("stamp {}: {e}", target_versions[0]))?;
            Ok(true)
        });
        match result {
            Ok(true) => eprintln!(
                "✓ tracking updated: {} versions removed, stamped `{}`",
                target_versions.len(),
                target_versions[0]
            ),
            Ok(false) => eprintln!("✓ tracking unchanged (none in range was applied)"),
            Err(e) => {
                eprintln!("⚠ squash: files OK but tracking failed: {e}");
                std::process::exit(1);
            }
        }
    }
    eprintln!(
        "✓ {} migration(s) squashed → `{}`. Originals in `{}`.",
        range.len(),
        squashed_path.display(),
        squashed_dir.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_11_13_change_is_manifest_only_for_fitz_toml() {
        assert!(change_is_manifest(std::path::Path::new("fitz.toml")));
        assert!(change_is_manifest(std::path::Path::new(
            "/some/project/fitz.toml"
        )));
        assert!(!change_is_manifest(std::path::Path::new("App.fitzv")));
        assert!(!change_is_manifest(std::path::Path::new("src/main.fitz")));
        assert!(!change_is_manifest(std::path::Path::new("Cargo.toml")));
        assert!(!change_is_manifest(std::path::Path::new("/proj")));
    }

    #[test]
    fn phase_11_13_try_resolve_single_file_is_ok_without_touching_manifest() {
        // Single-file mode returns Ok immediately (no cwd walk, no
        // manifest read), so the wasm dev loop's Result core never
        // exits the process — the whole point of the extraction.
        let r = try_resolve_entry_with_bin(
            Some(PathBuf::from("App.fitzv")),
            None,
            Some(manifest::Target::WasmClient),
        );
        let resolved = r.expect("single-file resolve should be Ok");
        assert_eq!(resolved.entry, PathBuf::from("App.fitzv"));
        assert!(resolved.manifest_ctx.is_none());
        assert_eq!(
            resolved.effective_target(),
            manifest::Target::WasmClient,
            "explicit --target override must win"
        );
    }

    #[test]
    fn pip_inputs_hash_is_deterministic() {
        let h1 = pip_inputs_hash(&["requests".to_string()], &[b"foo\n".to_vec()]);
        let h2 = pip_inputs_hash(&["requests".to_string()], &[b"foo\n".to_vec()]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_is_insensitive_to_positionals_order() {
        // Reordering `--bundle-pip` args must NOT invalidate
        // the cache: `--bundle-pip a --bundle-pip b` and
        // `--bundle-pip b --bundle-pip a` install the same set
        // of packages.
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
    fn pip_inputs_hash_changes_when_adding_a_package() {
        let h1 = pip_inputs_hash(&["requests".to_string()], &[]);
        let h2 = pip_inputs_hash(&["requests".to_string(), "httpx".to_string()], &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_changes_when_requirements_content_changes() {
        let h1 = pip_inputs_hash(&[], &[b"requests>=2.0\n".to_vec()]);
        let h2 = pip_inputs_hash(&[], &[b"requests>=3.0\n".to_vec()]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_positionals_vs_requirements_are_different() {
        // The `\n---\n` separator guarantees that ["foo", "bar"]
        // as positionals hashes differently from the same text
        // in a requirements file. Without the separator both
        // would produce the same bytes and collide.
        let h1 = pip_inputs_hash(&["foo".to_string(), "bar".to_string()], &[]);
        let h2 = pip_inputs_hash(&[], &[b"bar\nfoo\n".to_vec()]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn pip_inputs_hash_is_16_chars_hex() {
        // Inherited from tarball_hash_short (FNV-1a 64-bit).
        let h = pip_inputs_hash(&["requests".to_string()], &[]);
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn pip_inputs_hash_empty_returns_stable_hash() {
        // No packages nor requirements (degenerate case — should
        // not trigger at runtime because the pip block only
        // starts if there's something to install, but the helper
        // is still well-defined).
        let h1 = pip_inputs_hash(&[], &[]);
        let h2 = pip_inputs_hash(&[], &[]);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn pip_inputs_hash_requirements_order_invalidates_cache() {
        // Reordering requirements files DOES invalidate the
        // cache. Reason: pip processes them in order and two
        // files with conflicts/overrides can produce different
        // resolved packages depending on the order. We treat
        // each permutation as a distinct input, conservatively.
        let h1 = pip_inputs_hash(&[], &[b"requests\n".to_vec(), b"httpx\n".to_vec()]);
        let h2 = pip_inputs_hash(&[], &[b"httpx\n".to_vec(), b"requests\n".to_vec()]);
        assert_ne!(h1, h2);
    }
}
