//! Git deps — cloning + local cache (Phase 9.y.3.c).
//!
//! Enables `[dependencies] foo = { git = "https://...", tag = "v1.0.0" }`
//! in `fitz.toml`. The first access clones the repo to
//! `<cache>/git/<sanitized-url>@<ref>/` (global cache) and reuses the
//! dir on subsequent accesses.
//!
//! **Technical decisions made**:
//!
//! - **`git` subprocess** instead of a crate (`git2`/`gix`): zero
//!   additional deps, assumes `git` in the `PATH` (which is already
//!   the case for any Fitz dev). If it fails, clear error.
//! - **`tag` or `rev`**, NEVER `branch`: branches mutate upstream and
//!   break reproducibility. This restriction is validated here.
//! - **`tag` and `rev` mutually exclusive**: both specify a "fixed
//!   point"; mixing them generates ambiguity.
//! - **Cache directory naming**: sanitized URL + `@` + ref. No
//!   hashing, deterministic and human-readable
//!   (`github.com_foo_bar@v1.0.0/`). Trade-off: very long URLs or
//!   ones with exotic chars could collide; in the MVP we truncate to
//!   200 chars and accept the 99% case.
//! - **Cache reuse**: if the dir already exists, we assume a correct
//!   previous clone and only read the commit hash. No automatic
//!   re-clone. Manual invalidation (delete the dir or
//!   `fitz cache clean` post-MVP).
//! - **Cache override via env var `FITZ_CACHE_DIR`**: for tests
//!   (which need isolated tempdirs) and power users who want to
//!   share cache between machines or move it across disks.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Name of the env var that overrides the cache root.
pub const CACHE_DIR_ENV: &str = "FITZ_CACHE_DIR";

/// Returns the cache root: `$FITZ_CACHE_DIR` if set, otherwise
/// `~/.fitz/cache`. If there is no home either, fails — without a
/// cache root we cannot handle git deps.
pub fn cache_root() -> Result<PathBuf, GitDepError> {
    if let Ok(override_dir) = std::env::var(CACHE_DIR_ENV) {
        if !override_dir.is_empty() {
            return Ok(PathBuf::from(override_dir));
        }
    }
    let home = home_dir().ok_or(GitDepError::NoHomeDir)?;
    Ok(home.join(".fitz").join("cache"))
}

/// Cache subdirectory where the git clones live.
pub fn git_cache_root() -> Result<PathBuf, GitDepError> {
    Ok(cache_root()?.join("git"))
}

/// Returns the absolute path of the user's home directory. On Windows
/// uses `USERPROFILE`; on Unix `HOME`. No external dep (we do not use
/// `dirs`).
fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Ref requested in the manifest for a git dep. Mutually exclusive:
/// `Tag(s)` or `Rev(s)`, never both.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GitRef {
    Tag(String),
    Rev(String),
}

impl GitRef {
    /// String used in the cache dir and the lockfile. Same for tag
    /// and rev — the lockfile disambiguates by context (commit hash
    /// in `source`) if differentiation is needed.
    pub fn as_str(&self) -> &str {
        match self {
            GitRef::Tag(s) | GitRef::Rev(s) => s.as_str(),
        }
    }
}

/// Module errors. Independent of `ManifestError` — the caller does
/// `From` or wrap on integration.
#[derive(Debug)]
pub enum GitDepError {
    /// `git` is not in the `PATH` or could not be executed.
    GitNotFound(std::io::Error),
    /// `git clone` or `git checkout` or `git rev-parse` failed.
    /// Carries the command + the stderr for the message.
    GitCommandFailed { command: String, stderr: String },
    /// Could not determine the home directory (no `HOME` or
    /// `USERPROFILE`) and `FITZ_CACHE_DIR` is not set either.
    NoHomeDir,
    /// I/O error while manipulating the cache directory.
    Io(std::io::Error),
    /// Validation of the dep's shape failed: e.g., both `tag` and
    /// `rev` are present, or neither.
    InvalidGitDep(String),
}

impl std::fmt::Display for GitDepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitDepError::GitNotFound(e) => write!(
                f,
                "could not invoke `git` ({e}). Install it and make sure it is on the PATH."
            ),
            GitDepError::GitCommandFailed { command, stderr } => {
                let trimmed = stderr.trim();
                if trimmed.is_empty() {
                    write!(f, "command `{command}` failed with no output")
                } else {
                    write!(f, "command `{command}` failed:\n{trimmed}")
                }
            }
            GitDepError::NoHomeDir => write!(
                f,
                "could not determine the home directory to locate the cache. \
                 Set `FITZ_CACHE_DIR=<path>` to point to a writable directory."
            ),
            GitDepError::Io(e) => write!(f, "I/O error on the cache: {e}"),
            GitDepError::InvalidGitDep(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for GitDepError {}

/// Sanitizes a URL for use as a path component. Replaces problematic
/// characters with `_` and truncates to 200 chars so as not to exceed
/// the filesystem limit on Windows.
///
/// It is not a hash — it is a textual transformation. Theoretical
/// collisions exist (two very different URLs with a common prefix
/// could truncate to the same string) but they are irrelevant in the
/// 99% case.
pub fn sanitize_url(url: &str) -> String {
    // Strip the schema prefix so the cache does not fill up with
    // `https___...`. We accept http, https, git, ssh, and file.
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("git://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .or_else(|| url.strip_prefix("file://"))
        .unwrap_or(url);

    let sanitized: String = stripped
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect();

    if sanitized.len() > 200 {
        sanitized.chars().take(200).collect()
    } else {
        sanitized
    }
}

/// Builds the absolute cache directory path for a given (url, ref)
/// pair, without touching disk. Useful for tests + for reporting
/// errors that mention the expected location.
pub fn cache_path_for(url: &str, gitref: &GitRef) -> Result<PathBuf, GitDepError> {
    let dir_name = format!("{}@{}", sanitize_url(url), sanitize_url(gitref.as_str()));
    Ok(git_cache_root()?.join(dir_name))
}

/// Result of resolving a git dep against the cache.
#[derive(Debug, Clone, PartialEq)]
pub struct GitClonedRepo {
    /// Absolute path to the cloned repo's directory.
    pub abs_path: PathBuf,
    /// Exact commit hash (`git rev-parse HEAD` after checkout).
    /// Persisted in the lockfile as `source = "git+<url>#<commit>"`.
    pub commit_hash: String,
}

/// Guarantees the repo is cloned and checked out to the requested
/// `gitref`. If the cache already exists, assumes the previous clone
/// is valid and only reads the commit hash. If it does not exist,
/// clones from scratch.
///
/// The clone uses `--depth 1 --branch <tag-or-rev>`. For revs (commit
/// SHA), git accepts `--branch` only if it is pre-fetch resolvable;
/// if that fails, we fall back to a full clone + explicit checkout.
pub fn clone_or_use_cache(url: &str, gitref: &GitRef) -> Result<GitClonedRepo, GitDepError> {
    let target = cache_path_for(url, gitref)?;

    if !target.exists() {
        // Ensure the parent directory exists.
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(GitDepError::Io)?;
        }
        clone_fresh(url, gitref, &target)?;
    }

    let commit_hash = git_rev_parse_head(&target)?;
    Ok(GitClonedRepo {
        abs_path: target,
        commit_hash,
    })
}

/// Clone fresh: two strategies.
///
/// 1. If `gitref` is Tag: `git clone --depth 1 --branch <tag> <url> <target>`
///    works and is efficient.
/// 2. If `gitref` is Rev (commit SHA): `--branch` does not accept
///    SHAs. We do a full `git clone <url> <target>` + `git checkout
///    <sha>`. Wasteful but correct. Optimization with
///    `--filter=blob:none` stays as debt.
fn clone_fresh(url: &str, gitref: &GitRef, target: &Path) -> Result<(), GitDepError> {
    match gitref {
        GitRef::Tag(tag) => run_git(&[
            "clone",
            "--depth",
            "1",
            "--branch",
            tag,
            url,
            &target.to_string_lossy(),
        ]),
        GitRef::Rev(rev) => {
            run_git(&["clone", url, &target.to_string_lossy()])?;
            run_git_in(&["checkout", "--quiet", rev], target)
        }
    }
}

/// `git rev-parse HEAD` inside the repo at `path`. Returns the full
/// SHA (40 hex chars) without trailing newline.
pub fn git_rev_parse_head(path: &Path) -> Result<String, GitDepError> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .map_err(GitDepError::GitNotFound)?;

    if !output.status.success() {
        return Err(GitDepError::GitCommandFailed {
            command: "git rev-parse HEAD".to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Executes `git <args>` in the current cwd. Reports stderr on
/// errors.
fn run_git(args: &[&str]) -> Result<(), GitDepError> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(GitDepError::GitNotFound)?;
    if !output.status.success() {
        return Err(GitDepError::GitCommandFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Executes `git <args>` from `cwd`. Reports stderr on errors.
fn run_git_in(args: &[&str], cwd: &Path) -> Result<(), GitDepError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(GitDepError::GitNotFound)?;
    if !output.status.success() {
        return Err(GitDepError::GitCommandFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Builds the string that goes in the lockfile's `source` for a git
/// dep already resolved. Cargo-style format: `git+<url>#<commit-hash>`.
pub fn lockfile_source_string(url: &str, commit_hash: &str) -> String {
    format!("git+{url}#{commit_hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_url_strips_https_prefix() {
        assert_eq!(
            sanitize_url("https://github.com/foo/bar"),
            "github.com_foo_bar"
        );
    }

    #[test]
    fn sanitize_url_strips_other_schemes() {
        assert_eq!(sanitize_url("http://x.com/r"), "x.com_r");
        assert_eq!(sanitize_url("git://example.org/p"), "example.org_p");
        assert_eq!(sanitize_url("ssh://user@host/r.git"), "user_host_r.git");
        assert_eq!(sanitize_url("file:///tmp/r"), "_tmp_r");
    }

    #[test]
    fn sanitize_url_preserves_letters_numbers_dot_hyphen_underscore() {
        assert_eq!(
            sanitize_url("https://github.com/some-user_name/proj.v1"),
            "github.com_some-user_name_proj.v1"
        );
    }

    #[test]
    fn sanitize_url_truncates_to_200_chars() {
        let very_long = format!("https://github.com/{}", "a".repeat(300));
        let s = sanitize_url(&very_long);
        assert!(s.len() <= 200);
    }

    #[test]
    fn sanitize_url_without_prefix_accepts_raw_input() {
        // URLs without scheme (rare) are accepted as-is.
        assert_eq!(sanitize_url("just/a/path"), "just_a_path");
    }

    #[test]
    fn gitref_as_str_works_for_both_variants() {
        assert_eq!(GitRef::Tag("v1.0".to_string()).as_str(), "v1.0");
        assert_eq!(GitRef::Rev("abc123".to_string()).as_str(), "abc123");
    }

    #[test]
    fn cache_path_for_combines_sanitized_url_and_ref() {
        let tmp = tempfile::tempdir().unwrap();
        // Override the cache so as not to touch the real home during tests.
        let prev = std::env::var(CACHE_DIR_ENV).ok();
        std::env::set_var(CACHE_DIR_ENV, tmp.path());

        let p = cache_path_for(
            "https://github.com/foo/bar",
            &GitRef::Tag("v1.0.0".to_string()),
        )
        .unwrap();
        // The path must live under the override + /git/<sanitized>@<ref>.
        assert!(p.starts_with(tmp.path()));
        assert!(p.ends_with("github.com_foo_bar@v1.0.0"));

        // Restore the env var (other tests might run in parallel with
        // different env vars — note, see the comment below).
        match prev {
            Some(v) => std::env::set_var(CACHE_DIR_ENV, v),
            None => std::env::remove_var(CACHE_DIR_ENV),
        }
    }

    #[test]
    fn lockfile_source_string_cargo_style_format() {
        let s = lockfile_source_string(
            "https://github.com/foo/bar",
            "abc123def456789012345678901234567890abcd",
        );
        assert_eq!(
            s,
            "git+https://github.com/foo/bar#abc123def456789012345678901234567890abcd"
        );
    }

    // NOTE: the clone_or_use_cache tests (which invoke real git
    // against local bare repos) live in tests/cli_e2e.rs because they
    // need more elaborate setup (create bare repo + commits + tag)
    // and benefit from isolated tempdirs that do not compete for the
    // FITZ_CACHE_DIR env var.
}
