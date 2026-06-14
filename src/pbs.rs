//! python-build-standalone (PBS) — download + local cache of the
//! CPython tarball for `fitz build --bundle-python` (Phase 8.b).
//!
//! Downloads the `install_only_stripped` tarball of the pinned PBS
//! release for the target triple and saves it to
//! `<cache>/pbs/<tarball-name>` (global cache shared across builds).
//!
//! **Technical decisions made**:
//!
//! - **Pinned release** (constant `PBS_RELEASE`): reproducible builds.
//!   Manual bump every 3-6 months. Same policy as `Cargo.lock`.
//! - **CPython 3.14.x**: the most recent stable version available on
//!   PBS and inside the `abi3-py310` range (3.10-3.14+). PyO3 with
//!   `abi3-py310` + `auto-initialize` dynamic-links against the
//!   builder-specific libpython; bundling PBS 3.14 requires a builder
//!   with Python 3.14.x. When `R.bug-pyo3-abi3-portable-link` closes,
//!   the constraint disappears (it links against `libpython3.so`
//!   stable ABI).
//! - **`install_only_stripped` flavor**: drop-in portable Python
//!   directory (extracts to a temp dir + runs `python.exe` directly),
//!   without debug symbols. ~70% smaller than `install_only` (Linux
//!   x64: 33 MB vs 117 MB compressed).
//! - **`curl` subprocess** for download: zero new Rust deps, curl is
//!   guaranteed on modern Windows 11/macOS/Linux. Trade-off: no
//!   progress bar. If pressure arises we add `ureq` or `reqwest`
//!   later.
//! - **Cache override via `FITZ_CACHE_DIR`**: parallel to
//!   `git_dep.rs`. For tests (isolated tempdirs) and power users.
//! - **Cache key by tarball name** (not by release+triple separately):
//!   lets multiple Python versions coexist in the cache without
//!   overwrites (useful for future supported versions).

use std::path::{Path, PathBuf};
use std::process::Command;

// === Pinned constants ===

/// Pinned PBS release (YYYYMMDD format). Manual bump.
pub const PBS_RELEASE: &str = "20260510";

/// Embedded CPython version (must be available in `PBS_RELEASE`).
pub const PYTHON_VERSION: &str = "3.14.5";

/// Tarball flavor. `install_only_stripped` has the same files as
/// `install_only` but without debug symbols.
pub const TARBALL_FLAVOR: &str = "install_only_stripped";

/// Download file extension.
pub const TARBALL_EXTENSION: &str = "tar.gz";

/// Name of the env var that overrides the cache root (shared with
/// `git_dep.rs`).
pub const CACHE_DIR_ENV: &str = "FITZ_CACHE_DIR";

// === Supported triples ===

/// List of Rust triples for which PBS publishes tarballs and that we
/// support for bundling. If the user is on a different triple,
/// `host_triple()` fails with a clear error.
pub fn supported_triples() -> &'static [&'static str] {
    &[
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
    ]
}

/// Current host triple, detected via `cfg!`. Exhaustive match against
/// the 5 triples PBS publishes. If none match (e.g. musl, freebsd,
/// riscv), returns `PbsError::UnsupportedTriple`.
pub fn host_triple() -> Result<&'static str, PbsError> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("x86_64-pc-windows-msvc")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("x86_64-apple-darwin")
    } else {
        Err(PbsError::UnsupportedHostTriple)
    }
}

// === URL + naming ===

/// Tarball file name for a given triple. Canonical format used by
/// PBS: `cpython-<version>+<release>-<triple>-<flavor>.<ext>`.
pub fn tarball_name(triple: &str) -> String {
    format!(
        "cpython-{}+{}-{}-{}.{}",
        PYTHON_VERSION, PBS_RELEASE, triple, TARBALL_FLAVOR, TARBALL_EXTENSION
    )
}

/// Download URL of the tarball on PBS's GitHub Releases.
pub fn download_url(triple: &str) -> String {
    format!(
        "https://github.com/astral-sh/python-build-standalone/releases/download/{}/{}",
        PBS_RELEASE,
        tarball_name(triple)
    )
}

// === Cache ===

/// Returns the cache root: `$FITZ_CACHE_DIR` if set, otherwise
/// `~/.fitz/cache`. Shares the convention with `git_dep.rs`.
pub fn cache_root() -> Result<PathBuf, PbsError> {
    if let Ok(override_dir) = std::env::var(CACHE_DIR_ENV) {
        if !override_dir.is_empty() {
            return Ok(PathBuf::from(override_dir));
        }
    }
    let home = home_dir().ok_or(PbsError::NoHomeDir)?;
    Ok(home.join(".fitz").join("cache"))
}

/// Cache sub-directory where the downloaded PBS tarballs live.
pub fn pbs_cache_root() -> Result<PathBuf, PbsError> {
    Ok(cache_root()?.join("pbs"))
}

/// Absolute path of the cached tarball for a triple. Does not touch
/// disk.
pub fn cache_path_for(triple: &str) -> Result<PathBuf, PbsError> {
    Ok(pbs_cache_root()?.join(tarball_name(triple)))
}

/// Ensures the PBS tarball for `triple` is in cache. If it already
/// exists, returns its path. Otherwise downloads it via `curl` and
/// returns the resulting path. The download is atomic: downloads to
/// `<path>.tmp` + rename at the end (avoids partial state if
/// interrupted).
pub fn ensure_tarball(triple: &str) -> Result<PathBuf, PbsError> {
    let target = cache_path_for(triple)?;

    if target.exists() {
        return Ok(target);
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(PbsError::Io)?;
    }

    download_tarball(triple, &target)?;
    Ok(target)
}

/// Downloads the tarball to `target` via `curl`. Uses a `.tmp` file
/// then renames it so the cache is never left with partial files if
/// the download is interrupted.
fn download_tarball(triple: &str, target: &Path) -> Result<(), PbsError> {
    let url = download_url(triple);
    let tmp = target.with_extension("tar.gz.tmp");

    // Remove previous `.tmp` if it was left from an interrupted run.
    if tmp.exists() {
        std::fs::remove_file(&tmp).map_err(PbsError::Io)?;
    }

    // -s: silent (no progress bar — the output would be noise in CI).
    // -L: follow redirects (GitHub redirects to fastly).
    // --fail: exit code != 0 on HTTP 4xx/5xx (without this, curl
    //         "successfully" downloads the GitHub 404 page).
    // -o: destination file.
    let output = Command::new("curl")
        .args(["-sL", "--fail", "-o", &tmp.to_string_lossy(), url.as_str()])
        .output()
        .map_err(PbsError::CurlNotFound)?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(PbsError::DownloadFailed {
            url,
            triple: triple.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    // Atomic rename (cross-platform). If target appeared while we
    // were downloading (race with another concurrent run), rename
    // overwrites it — not a problem because the content is the same
    // (pinned release).
    std::fs::rename(&tmp, target).map_err(PbsError::Io)?;
    Ok(())
}

// === Helpers ===

fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

// === Errors ===

#[derive(Debug)]
pub enum PbsError {
    /// The host triple is not among the 5 that PBS publishes and we
    /// support.
    UnsupportedHostTriple,
    /// `curl` is not in `PATH` or could not be executed.
    CurlNotFound(std::io::Error),
    /// The HTTP download failed (404, timeout, etc.).
    DownloadFailed {
        url: String,
        triple: String,
        stderr: String,
    },
    /// Could not determine the home directory.
    NoHomeDir,
    /// I/O error while manipulating the cache.
    Io(std::io::Error),
}

impl std::fmt::Display for PbsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PbsError::UnsupportedHostTriple => write!(
                f,
                "el host actual no está entre los triples soportados por \
                 `--bundle-python`. Triples soportados: {}",
                supported_triples().join(", ")
            ),
            PbsError::CurlNotFound(e) => write!(
                f,
                "no se pudo invocar `curl` ({e}). Instalalo y asegurate que esté en el PATH."
            ),
            PbsError::DownloadFailed {
                url,
                triple,
                stderr,
            } => {
                let trimmed = stderr.trim();
                if trimmed.is_empty() {
                    write!(
                        f,
                        "falló la descarga del tarball PBS para `{triple}` desde {url}"
                    )
                } else {
                    write!(
                        f,
                        "falló la descarga del tarball PBS para `{triple}` desde {url}:\n{trimmed}"
                    )
                }
            }
            PbsError::NoHomeDir => write!(
                f,
                "no se pudo determinar el home directory para ubicar el cache. \
                 Seteá `FITZ_CACHE_DIR=<path>` para apuntar a un directorio escribible."
            ),
            PbsError::Io(e) => write!(f, "error de I/O sobre el cache PBS: {e}"),
        }
    }
}

impl std::error::Error for PbsError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that mutate the global `FITZ_CACHE_DIR`
    /// env var. `cargo test` runs in parallel by default and the env
    /// is process-global; without a lock, one test could read while
    /// another was doing `remove_var` and return the default path
    /// `~/.fitz/cache` instead of the `tempdir` override. The Windows
    /// CI caught it flaking. `parking_lot::Mutex` (vs
    /// `std::sync::Mutex`) does not poison on panic — if a test
    /// `assert!` inside the guard panics, the others waiting continue
    /// without propagating the poison.
    static ENV_VAR_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn pbs_release_is_yyyymmdd_string() {
        // Sanity check on the format of the pinned release.
        assert_eq!(PBS_RELEASE.len(), 8, "release debería ser YYYYMMDD");
        assert!(PBS_RELEASE.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn python_version_is_3_14_x() {
        // Sanity check: we are pinned to CPython 3.14.x (latest
        // stable inside the `abi3-py310` range that PyO3 supports).
        assert!(PYTHON_VERSION.starts_with("3.14."));
    }

    #[test]
    fn tarball_name_canonical_format() {
        let name = tarball_name("x86_64-pc-windows-msvc");
        assert_eq!(
            name,
            format!(
                "cpython-{}+{}-x86_64-pc-windows-msvc-install_only_stripped.tar.gz",
                PYTHON_VERSION, PBS_RELEASE
            )
        );
    }

    #[test]
    fn download_url_points_to_github_releases() {
        let url = download_url("x86_64-unknown-linux-gnu");
        assert!(url.starts_with(
            "https://github.com/astral-sh/python-build-standalone/releases/download/"
        ));
        assert!(url.contains(PBS_RELEASE));
        assert!(url.contains("x86_64-unknown-linux-gnu"));
        assert!(url.contains("install_only_stripped"));
        assert!(url.ends_with(".tar.gz"));
    }

    #[test]
    fn supported_triples_covers_5_platforms() {
        let triples = supported_triples();
        assert_eq!(triples.len(), 5);
        assert!(triples.contains(&"x86_64-unknown-linux-gnu"));
        assert!(triples.contains(&"aarch64-unknown-linux-gnu"));
        assert!(triples.contains(&"x86_64-pc-windows-msvc"));
        assert!(triples.contains(&"aarch64-apple-darwin"));
        assert!(triples.contains(&"x86_64-apple-darwin"));
    }

    #[test]
    fn host_triple_detects_current_host() {
        // On any supported platform this must return Ok. If the test
        // runs on an unsupported platform (e.g. musl), it returns Err
        // — acceptable, it is not our target.
        let detected = host_triple();
        if cfg!(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
        )) {
            let t = detected.expect("host triple debe detectarse en plataformas soportadas");
            assert!(supported_triples().contains(&t));
        }
    }

    #[test]
    fn cache_path_for_combines_root_and_tarball_name() {
        let _guard = ENV_VAR_LOCK.lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var(CACHE_DIR_ENV).ok();
        std::env::set_var(CACHE_DIR_ENV, tmp.path());

        let p = cache_path_for("x86_64-pc-windows-msvc").unwrap();
        assert!(p.starts_with(tmp.path()));
        assert!(p.to_string_lossy().contains("pbs"));
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".tar.gz"));

        match prev {
            Some(v) => std::env::set_var(CACHE_DIR_ENV, v),
            None => std::env::remove_var(CACHE_DIR_ENV),
        }
    }

    #[test]
    fn cache_root_uses_env_override() {
        let _guard = ENV_VAR_LOCK.lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var(CACHE_DIR_ENV).ok();
        std::env::set_var(CACHE_DIR_ENV, tmp.path());

        let root = cache_root().unwrap();
        assert_eq!(root, tmp.path());

        let pbs_root = pbs_cache_root().unwrap();
        assert_eq!(pbs_root, tmp.path().join("pbs"));

        match prev {
            Some(v) => std::env::set_var(CACHE_DIR_ENV, v),
            None => std::env::remove_var(CACHE_DIR_ENV),
        }
    }

    #[test]
    fn pbs_error_display_unsupported_triple_lists_supported() {
        let s = format!("{}", PbsError::UnsupportedHostTriple);
        for triple in supported_triples() {
            assert!(
                s.contains(triple),
                "el mensaje debería mencionar el triple {triple}"
            );
        }
    }

    #[test]
    fn pbs_error_display_download_failed_includes_url_and_stderr() {
        let err = PbsError::DownloadFailed {
            url: "https://example.com/x.tar.gz".to_string(),
            triple: "x86_64-unknown-linux-gnu".to_string(),
            stderr: "curl: (22) HTTP 404".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("x86_64-unknown-linux-gnu"));
        assert!(s.contains("example.com"));
        assert!(s.contains("404"));
    }

    // NOTE: real download tests (ensure_tarball / download_tarball
    // against GitHub) live in `tests/cli_e2e.rs` because they are
    // slow (~5-30s), require network, and benefit from isolated
    // tempdirs.
}
