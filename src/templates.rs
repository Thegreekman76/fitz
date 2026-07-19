//! Project templates for `fitz new --template <name>` (Phase 4 Y-B,
//! Session 1.c).
//!
//! A template is a git repo (or a subpath inside one) that ships a
//! pre-made project structure. `fitz new my-app --template liveviews`
//! clones the template, copies its contents into `my-app/`, and
//! substitutes `{{name}}` placeholders in text files with the project
//! name the user picked.
//!
//! **Technical decisions**:
//!
//! - **Named registry, hardcoded**: this MVP ships one template
//!   (`liveviews`). A `fitz.toml`-based user registry is future work.
//! - **`git` subprocess** (same as `git_dep.rs`): zero extra deps, `git`
//!   assumed on the PATH.
//! - **No cache**: template scaffolding is a one-shot; we clone fresh
//!   into a tempdir and delete it after. If someone regenerates
//!   frequently, they can `--template-git <url>` (a future flag).
//! - **`{{name}}` substitution**: only in files we can decode as UTF-8.
//!   Binary files copy through untouched. Simple, deterministic, no
//!   config.
//! - **Env var overrides per template** (`FITZ_TEMPLATE_<NAME>_URL/
//!   SUBPATH/REF`): used by tests to point at local `file://` git repos
//!   and by power users who fork a template. Names are uppercased with
//!   `-` and `_` collapsed to `_`.
//!
//! **Public API**:
//! - `resolve_template(name) -> Option<TemplateSource>`
//! - `scaffold_from_template(source, target_dir, project_name) -> Result<(), TemplateError>`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Env var prefix for per-template overrides. See module docs.
const TEMPLATE_ENV_PREFIX: &str = "FITZ_TEMPLATE_";

/// Where a template lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSource {
    /// Clone a git repo (shallow when possible) and copy a subpath.
    Git {
        /// Repo URL. Any transport `git` understands: `https://`,
        /// `git://`, `ssh://`, `file:///`.
        url: String,
        /// Path inside the repo to copy from. Empty string means the
        /// repo root.
        subpath: String,
        /// Tag, branch, or commit SHA to check out.
        ref_: String,
    },
}

/// Errors emitted by the templates module.
#[derive(Debug)]
pub enum TemplateError {
    /// `git` is not in the PATH or could not be executed.
    GitNotFound(std::io::Error),
    /// `git clone` or `git checkout` failed. Carries the command and
    /// stderr for the message.
    GitCommandFailed { command: String, stderr: String },
    /// The `subpath` requested does not exist in the cloned repo.
    SubpathNotFound { subpath: String, repo_url: String },
    /// I/O error while copying files or substituting placeholders.
    Io(std::io::Error),
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::GitNotFound(e) => write!(
                f,
                "could not invoke `git` ({e}). Install it and make sure it is on the PATH."
            ),
            TemplateError::GitCommandFailed { command, stderr } => {
                let trimmed = stderr.trim();
                if trimmed.is_empty() {
                    write!(f, "command `{command}` failed with no output")
                } else {
                    write!(f, "command `{command}` failed:\n{trimmed}")
                }
            }
            TemplateError::SubpathNotFound { subpath, repo_url } => write!(
                f,
                "subpath `{subpath}` not found in the cloned template repo `{repo_url}`"
            ),
            TemplateError::Io(e) => write!(f, "I/O error while scaffolding template: {e}"),
        }
    }
}

impl std::error::Error for TemplateError {}

/// Looks up the built-in template registry for `name`, honouring per-
/// template env var overrides (`FITZ_TEMPLATE_<NAME>_URL/SUBPATH/REF`).
///
/// Returns `Some(TemplateSource)` if the name is known, `None`
/// otherwise. Env var overrides only apply when the name is known;
/// they cannot introduce new templates on their own.
pub fn resolve_template(name: &str) -> Option<TemplateSource> {
    let base = builtin_template(name)?;
    Some(apply_env_overrides(name, base))
}

/// Hardcoded registry. Add new templates here.
fn builtin_template(name: &str) -> Option<TemplateSource> {
    match name {
        "liveviews" => Some(TemplateSource::Git {
            // fitz-liveviews (Phase 4 Y-B, Session 1.c). The starter
            // template subpath (`templates/basic`) does not exist in the
            // repo yet; it lands in a follow-up session together with
            // the framework layer. Until then the env var override lets
            // tests + power users point at any subpath (e.g.
            // `examples/counter`).
            url: "https://github.com/Thegreekman76/fitz-liveviews".to_string(),
            subpath: "templates/basic".to_string(),
            ref_: "main".to_string(),
        }),
        _ => None,
    }
}

/// Reads `FITZ_TEMPLATE_<UPPER_NAME>_URL/SUBPATH/REF` env vars and
/// overrides matching fields of `base` when set (non-empty).
fn apply_env_overrides(name: &str, base: TemplateSource) -> TemplateSource {
    let key = env_key_for(name);
    let TemplateSource::Git {
        mut url,
        mut subpath,
        mut ref_,
    } = base;
    if let Ok(v) = std::env::var(format!("{TEMPLATE_ENV_PREFIX}{key}_URL")) {
        if !v.is_empty() {
            url = v;
        }
    }
    if let Ok(v) = std::env::var(format!("{TEMPLATE_ENV_PREFIX}{key}_SUBPATH")) {
        // Empty override is meaningful: means "use the repo root".
        subpath = v;
    }
    if let Ok(v) = std::env::var(format!("{TEMPLATE_ENV_PREFIX}{key}_REF")) {
        if !v.is_empty() {
            ref_ = v;
        }
    }
    TemplateSource::Git { url, subpath, ref_ }
}

/// Normalises a template name into the env var suffix: uppercase,
/// `-`/`_` collapsed to `_`, anything else stripped.
fn env_key_for(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' => Some(c.to_ascii_uppercase()),
            '-' | '_' => Some('_'),
            _ => None,
        })
        .collect()
}

/// Clones `source` into a fresh tempdir, copies the `subpath` contents
/// into `target_dir`, and substitutes `{{name}}` placeholders in every
/// UTF-8-decodable file with `project_name`.
///
/// `target_dir` should already exist and be writable. Existing files
/// are overwritten (the caller is expected to have checked emptiness).
///
/// The tempdir cleans itself up when the `TempDir` guard drops.
pub fn scaffold_from_template(
    source: &TemplateSource,
    target_dir: &Path,
    project_name: &str,
) -> Result<(), TemplateError> {
    let TemplateSource::Git { url, subpath, ref_ } = source;

    // 1. Clone to an isolated tempdir (auto-cleaned on drop).
    let tempdir = TempDir::new().map_err(TemplateError::Io)?;
    let clone_path = tempdir.path().join("repo");
    clone_repo(url, ref_, &clone_path)?;

    // 2. Resolve the subpath and confirm it exists.
    let source_root = if subpath.is_empty() {
        clone_path.clone()
    } else {
        clone_path.join(subpath)
    };
    if !source_root.is_dir() {
        return Err(TemplateError::SubpathNotFound {
            subpath: subpath.clone(),
            repo_url: url.clone(),
        });
    }

    // 3. Recursively copy, substituting `{{name}}` in decodable files.
    copy_with_substitutions(&source_root, target_dir, project_name)?;

    // tempdir drops here → cleanup.
    Ok(())
}

/// Minimal in-house tempdir: creates a fresh directory under
/// `std::env::temp_dir()` and best-effort removes it on drop. We avoid
/// promoting the `tempfile` crate from dev-dep to runtime dep just for
/// the templates flow.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates `<sys_tmp>/fitz-tpl-<pid>-<counter>/`. Fails only if the
    /// filesystem refuses to create the directory after several tries.
    fn new() -> std::io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let pid = std::process::id();
        // Try a small number of names in case two calls collide.
        for _ in 0..64 {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let candidate = base.join(format!("fitz-tpl-{pid}-{n}"));
            match std::fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self { path: candidate }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique tempdir under the system temp",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best-effort: if cleanup fails (e.g. Windows file lock), leave
        // the dir behind — the OS temp reaper will handle it eventually.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// `git clone` + `git checkout <ref>`. Two strategies parallel to
/// `git_dep::clone_fresh`: if `ref_` looks like a tag/branch, we try
/// `--depth 1 --branch <ref>`; if that fails (e.g. `ref_` is a commit
/// SHA) we fall back to a full clone + explicit `git checkout`.
fn clone_repo(url: &str, ref_: &str, target: &Path) -> Result<(), TemplateError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(TemplateError::Io)?;
    }

    // Try shallow clone first — works for tags/branches.
    let shallow = run_git(&[
        "clone",
        "--depth",
        "1",
        "--branch",
        ref_,
        url,
        &target.to_string_lossy(),
    ]);
    if shallow.is_ok() {
        return Ok(());
    }

    // Fallback: full clone + explicit checkout for commit SHAs and
    // repos where the shallow strategy fails.
    // Remove any partial state from the failed attempt.
    if target.exists() {
        let _ = std::fs::remove_dir_all(target);
    }
    run_git(&["clone", url, &target.to_string_lossy()])?;
    run_git_in(&["checkout", "--quiet", ref_], target)
}

/// Executes `git <args>`. Reports stderr on failure.
fn run_git(args: &[&str]) -> Result<(), TemplateError> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(TemplateError::GitNotFound)?;
    if !output.status.success() {
        return Err(TemplateError::GitCommandFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Executes `git <args>` from `cwd`. Reports stderr on failure.
fn run_git_in(args: &[&str], cwd: &Path) -> Result<(), TemplateError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(TemplateError::GitNotFound)?;
    if !output.status.success() {
        return Err(TemplateError::GitCommandFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Recursively copies files from `source_root` into `target_root`,
/// substituting `{{name}}` with `project_name` in every UTF-8-decodable
/// file. Skips the top-level `.git/` directory (would carry the whole
/// clone history — the user gets a fresh repo).
fn copy_with_substitutions(
    source_root: &Path,
    target_root: &Path,
    project_name: &str,
) -> Result<(), TemplateError> {
    copy_recursive(source_root, source_root, target_root, project_name)
}

fn copy_recursive(
    src_root: &Path,
    src_dir: &Path,
    dst_root: &Path,
    project_name: &str,
) -> Result<(), TemplateError> {
    for entry in std::fs::read_dir(src_dir).map_err(TemplateError::Io)? {
        let entry = entry.map_err(TemplateError::Io)?;
        let src_path = entry.path();

        // Skip the .git directory at any level — the template is being
        // consumed as source, not as a git repo.
        if src_path.file_name().and_then(|n| n.to_str()) == Some(".git") {
            continue;
        }

        // Compute the destination path preserving the relative subtree.
        let rel = src_path
            .strip_prefix(src_root)
            .map_err(|_| {
                TemplateError::Io(std::io::Error::other("unexpected path outside source root"))
            })?
            .to_path_buf();
        let dst_path = dst_root.join(&rel);

        let file_type = entry.file_type().map_err(TemplateError::Io)?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&dst_path).map_err(TemplateError::Io)?;
            copy_recursive(src_root, &src_path, dst_root, project_name)?;
        } else if file_type.is_file() {
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent).map_err(TemplateError::Io)?;
            }
            copy_file_with_subs(&src_path, &dst_path, project_name)?;
        }
        // Symlinks and other types are skipped silently — templates in
        // the wild rarely use them; if needed, add explicit handling.
    }
    Ok(())
}

/// Copies a single file. If the file is decodable as UTF-8, substitutes
/// `{{name}}` before writing; otherwise copies bytes verbatim.
fn copy_file_with_subs(src: &Path, dst: &Path, project_name: &str) -> Result<(), TemplateError> {
    let bytes = std::fs::read(src).map_err(TemplateError::Io)?;
    match std::str::from_utf8(&bytes) {
        Ok(text) => {
            let replaced = text.replace("{{name}}", project_name);
            std::fs::write(dst, replaced).map_err(TemplateError::Io)?;
        }
        Err(_) => {
            std::fs::write(dst, &bytes).map_err(TemplateError::Io)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that mutate the process-global
    /// `FITZ_TEMPLATE_*` env vars. Without this, `cargo test`'s
    /// multi-threaded runner lets them race — one test clears a var
    /// another just set, so an override read races the default — which
    /// flakes CI intermittently (seen on ubuntu-latest, exit 101).
    /// Zero-dep alternative to the `serial_test` crate. `Mutex::new`
    /// is `const` since Rust 1.63, so it works in a `static`.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolve_template_liveviews_default_matches_registry() {
        // Recover from a poisoned lock (a prior test panicked while
        // holding it) so one failure doesn't cascade to the others.
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Guard: ensure no test env var interferes. Save + clear + restore.
        let key = env_key_for("liveviews");
        let vars = [
            format!("{TEMPLATE_ENV_PREFIX}{key}_URL"),
            format!("{TEMPLATE_ENV_PREFIX}{key}_SUBPATH"),
            format!("{TEMPLATE_ENV_PREFIX}{key}_REF"),
        ];
        let prev: Vec<Option<String>> = vars.iter().map(|v| std::env::var(v).ok()).collect();
        for v in &vars {
            std::env::remove_var(v);
        }

        let src = resolve_template("liveviews").expect("liveviews must resolve");
        let TemplateSource::Git { url, subpath, ref_ } = src;
        assert!(url.contains("fitz-liveviews"), "url: {url}");
        assert_eq!(subpath, "templates/basic");
        assert_eq!(ref_, "main");

        // Restore.
        for (v, p) in vars.iter().zip(prev.iter()) {
            match p {
                Some(x) => std::env::set_var(v, x),
                None => std::env::remove_var(v),
            }
        }
    }

    #[test]
    fn resolve_template_unknown_name_returns_none() {
        assert!(resolve_template("does-not-exist").is_none());
        assert!(resolve_template("").is_none());
    }

    #[test]
    fn env_key_for_uppercases_and_collapses_separators() {
        assert_eq!(env_key_for("liveviews"), "LIVEVIEWS");
        assert_eq!(env_key_for("my-template"), "MY_TEMPLATE");
        assert_eq!(env_key_for("Foo_Bar-Baz"), "FOO_BAR_BAZ");
        // Non-alnum non-separator chars are stripped.
        assert_eq!(env_key_for("foo.bar/baz"), "FOOBARBAZ");
    }

    #[test]
    fn resolve_template_env_var_url_override_replaces_registry_url() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let key = env_key_for("liveviews");
        let var = format!("{TEMPLATE_ENV_PREFIX}{key}_URL");
        let prev = std::env::var(&var).ok();
        std::env::set_var(&var, "file:///tmp/local-template");

        let src = resolve_template("liveviews").expect("liveviews must resolve");
        let TemplateSource::Git { url, .. } = src;
        assert_eq!(url, "file:///tmp/local-template");

        match prev {
            Some(v) => std::env::set_var(&var, v),
            None => std::env::remove_var(&var),
        }
    }

    #[test]
    fn resolve_template_env_var_subpath_override_can_be_empty_string() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Empty override is meaningful: means "use the repo root".
        let key = env_key_for("liveviews");
        let var = format!("{TEMPLATE_ENV_PREFIX}{key}_SUBPATH");
        let prev = std::env::var(&var).ok();
        std::env::set_var(&var, "");

        let src = resolve_template("liveviews").expect("liveviews must resolve");
        let TemplateSource::Git { subpath, .. } = src;
        assert_eq!(subpath, "", "empty override should yield empty subpath");

        match prev {
            Some(v) => std::env::set_var(&var, v),
            None => std::env::remove_var(&var),
        }
    }

    #[test]
    fn copy_file_with_subs_replaces_placeholder_in_utf8_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("in.toml");
        let dst = tmp.path().join("out.toml");
        std::fs::write(&src, "name = \"{{name}}\"\nvalue = 1\n").unwrap();

        copy_file_with_subs(&src, &dst, "my-app").unwrap();

        let out = std::fs::read_to_string(&dst).unwrap();
        assert_eq!(out, "name = \"my-app\"\nvalue = 1\n");
    }

    #[test]
    fn copy_file_with_subs_leaves_binary_bytes_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob.bin");
        let dst = tmp.path().join("out.bin");
        // Invalid UTF-8 (isolated continuation byte).
        let bytes = vec![0xFFu8, 0xFE, 0x00, 0x01, 0x02];
        std::fs::write(&src, &bytes).unwrap();

        copy_file_with_subs(&src, &dst, "my-app").unwrap();

        let out = std::fs::read(&dst).unwrap();
        assert_eq!(out, bytes);
    }

    #[test]
    fn copy_recursive_preserves_directory_structure_and_substitutes() {
        let tmp = tempfile::tempdir().unwrap();
        let src_root = tmp.path().join("src");
        let dst_root = tmp.path().join("dst");

        std::fs::create_dir_all(src_root.join("sub")).unwrap();
        std::fs::write(
            src_root.join("fitz.toml"),
            "[package]\nname = \"{{name}}\"\n",
        )
        .unwrap();
        std::fs::write(
            src_root.join("sub").join("main.fitz"),
            "print(\"Hola desde {{name}}\")\n",
        )
        .unwrap();
        // Skip: .git/ directory at any level.
        std::fs::create_dir_all(src_root.join(".git")).unwrap();
        std::fs::write(src_root.join(".git").join("HEAD"), "ref: master\n").unwrap();

        std::fs::create_dir_all(&dst_root).unwrap();
        copy_with_substitutions(&src_root, &dst_root, "cool-app").unwrap();

        let toml = std::fs::read_to_string(dst_root.join("fitz.toml")).unwrap();
        assert!(toml.contains("name = \"cool-app\""));

        let main = std::fs::read_to_string(dst_root.join("sub").join("main.fitz")).unwrap();
        assert_eq!(main, "print(\"Hola desde cool-app\")\n");

        // .git/ is not copied.
        assert!(!dst_root.join(".git").exists(), ".git/ should be skipped");
    }
}
