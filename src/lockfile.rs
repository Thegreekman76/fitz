//! Lockfile (`fitz.lock`) — Phase 9.y.3.a.
//!
//! File parallel to the manifest that records the resolved deps with
//! the exact information used (absolute path, commit hash for git,
//! version for registry). It is committed alongside `fitz.toml` — it
//! is the build reproducibility guarantee.
//!
//! The format symbolically follows **Cargo.lock**: a list of
//! `[[package]]` with `name`, `version`, and optional `source`. For
//! path deps we do not emit `source` (following the Cargo convention:
//! path deps are implicit and do not require a source field).
//!
//! Schema versioned via the top-level `version = N` field. v1
//! (9.y.3.a) only supports path deps; extensions for git (9.y.3.c)
//! and registry (9.y.5) may maintain compatibility with v1 or bump
//! as needed.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

use crate::manifest::{ResolvedDep, ResolvedDepSource};

/// Lockfile file name, adjacent to `fitz.toml`.
pub const LOCKFILE_FILE: &str = "fitz.lock";

/// Lockfile schema version. Bump if we break compat with previous
/// versions (for now only v1).
pub const CURRENT_LOCKFILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lockfile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "package")]
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    /// String-encoded source Cargo-style. `None` for path deps
    /// (the Cargo convention: path deps are implicit and carry no
    /// source field in the lockfile). For future git/registry deps
    /// it will be `Some("git+url#sha")` or `Some("registry+url")`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug)]
pub enum LockfileError {
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    /// The lockfile on disk uses a version we do not understand.
    /// `found` carries the version read so it can be cited in the
    /// message.
    UnsupportedVersion {
        found: u32,
    },
}

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockfileError::Parse(e) => write!(f, "error parsing lockfile: {e}"),
            LockfileError::Serialize(e) => {
                write!(f, "error serializing lockfile: {e}")
            }
            LockfileError::UnsupportedVersion { found } => write!(
                f,
                "lockfile version `{found}` is not supported by this \
                 binary (understands up to v{CURRENT_LOCKFILE_VERSION}). \
                 Delete it and let `fitz run`/`build`/`check` regenerate \
                 it, or upgrade `fitz`."
            ),
        }
    }
}

impl std::error::Error for LockfileError {}

impl Lockfile {
    /// Parses a lockfile from TOML text. Validates that `version` is
    /// one we understand.
    pub fn parse(input: &str) -> Result<Self, LockfileError> {
        let l: Lockfile = toml::from_str(input).map_err(LockfileError::Parse)?;
        if l.version > CURRENT_LOCKFILE_VERSION {
            return Err(LockfileError::UnsupportedVersion { found: l.version });
        }
        Ok(l)
    }

    /// Serializes to TOML.
    pub fn to_toml_string(&self) -> Result<String, LockfileError> {
        toml::to_string(self).map_err(LockfileError::Serialize)
    }

    /// Builds a fresh lockfile from the resolved deps. Sorts by
    /// `name` so the file is deterministic (important for git diffs).
    pub fn from_resolved(deps: &[ResolvedDep]) -> Self {
        let mut packages: Vec<LockedPackage> = deps
            .iter()
            .map(|d| LockedPackage {
                name: d.name.clone(),
                version: d.version.clone(),
                source: match &d.source {
                    // Path deps carry no `source` in the lockfile
                    // (Cargo convention: they are implicit).
                    ResolvedDepSource::Path { .. } => None,
                    // Git deps emit `git+<url>#<commit-hash>` —
                    // Phase 9.y.3.c. The exact commit hash allows
                    // reproducing the build even if the upstream tag
                    // mutates.
                    ResolvedDepSource::Git {
                        url, commit_hash, ..
                    } => Some(crate::git_dep::lockfile_source_string(url, commit_hash)),
                },
            })
            .collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        Lockfile {
            version: CURRENT_LOCKFILE_VERSION,
            packages,
        }
    }
}

/// Determines whether the lockfile at `path` already has exactly the
/// same content as `new`, comparing structurally (not byte-by-byte —
/// allows normalizing TOML serialization between versions).
///
/// Returns `true` if writing is NOT necessary (same content), `false`
/// if writing is required.
pub fn lockfile_matches(path: &std::path::Path, new: &Lockfile) -> bool {
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    match Lockfile::parse(&existing) {
        Ok(parsed) => parsed == *new,
        Err(_) => false,
    }
}

/// Writes the lockfile to `path` if its content differs from what is
/// already there (or there is nothing). No-op if the content matches.
/// Returns `Ok(true)` if it wrote, `Ok(false)` if it did nothing.
pub fn write_lockfile_if_changed(
    path: &std::path::Path,
    lockfile: &Lockfile,
) -> Result<bool, std::io::Error> {
    if lockfile_matches(path, lockfile) {
        return Ok(false);
    }
    let text = lockfile
        .to_toml_string()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(true)
}

/// Helper to build the absolute lockfile path given the manifest's
/// directory.
pub fn lockfile_path(manifest_dir: &std::path::Path) -> PathBuf {
    manifest_dir.join(LOCKFILE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ResolvedDepSource;

    fn dep(name: &str, version: &str, declared: &str) -> ResolvedDep {
        ResolvedDep {
            name: name.to_string(),
            version: version.to_string(),
            abs_path: PathBuf::from("/fake/abs/path"),
            lib_entry: PathBuf::from("/fake/abs/path/src/lib.fitz"),
            source: ResolvedDepSource::Path {
                declared: declared.to_string(),
            },
        }
    }

    #[test]
    fn from_resolved_empty_emits_lockfile_v1_without_packages() {
        let l = Lockfile::from_resolved(&[]);
        assert_eq!(l.version, CURRENT_LOCKFILE_VERSION);
        assert!(l.packages.is_empty());
    }

    #[test]
    fn from_resolved_path_dep_does_not_emit_source() {
        let l = Lockfile::from_resolved(&[dep("utils", "0.1.0", "../utils")]);
        assert_eq!(l.packages.len(), 1);
        assert_eq!(l.packages[0].name, "utils");
        assert_eq!(l.packages[0].version, "0.1.0");
        assert!(
            l.packages[0].source.is_none(),
            "path deps no llevan source: {:?}",
            l.packages[0].source
        );
    }

    #[test]
    fn from_resolved_sorts_alphabetically_by_name() {
        let l = Lockfile::from_resolved(&[
            dep("zeta", "1.0.0", "../zeta"),
            dep("alfa", "0.1.0", "../alfa"),
            dep("medio", "0.5.0", "../medio"),
        ]);
        let names: Vec<&str> = l.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["alfa", "medio", "zeta"]);
    }

    #[test]
    fn serializes_omits_empty_packages_and_source() {
        let l = Lockfile::from_resolved(&[]);
        let toml_text = l.to_toml_string().unwrap();
        // Only the version header, no [[package]].
        assert!(toml_text.contains("version = 1"));
        assert!(!toml_text.contains("[[package]]"));
        assert!(!toml_text.contains("source"));
    }

    #[test]
    fn round_trip_preserves_structure() {
        let original =
            Lockfile::from_resolved(&[dep("a", "0.1.0", "../a"), dep("b", "0.2.0", "../b")]);
        let toml_text = original.to_toml_string().unwrap();
        let parsed = Lockfile::parse(&toml_text).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn parse_accepts_empty_lockfile() {
        let text = "version = 1\n";
        let l = Lockfile::parse(text).unwrap();
        assert_eq!(l.version, 1);
        assert!(l.packages.is_empty());
    }

    #[test]
    fn parse_rejects_future_version() {
        let text = "version = 999\n";
        let err = Lockfile::parse(text).unwrap_err();
        match err {
            LockfileError::UnsupportedVersion { found } => assert_eq!(found, 999),
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn parse_accepts_lockfile_with_packages() {
        let text = r#"
version = 1

[[package]]
name = "utils"
version = "0.1.0"

[[package]]
name = "http-helpers"
version = "0.3.0"
source = "git+https://github.com/foo/bar#abc123"
"#;
        let l = Lockfile::parse(text).unwrap();
        assert_eq!(l.packages.len(), 2);
        assert_eq!(l.packages[0].name, "utils");
        assert!(l.packages[0].source.is_none());
        assert_eq!(
            l.packages[1].source.as_deref(),
            Some("git+https://github.com/foo/bar#abc123")
        );
    }

    #[test]
    fn lockfile_matches_detects_structural_equality() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let l = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, l.to_toml_string().unwrap()).unwrap();
        assert!(lockfile_matches(&path, &l));
    }

    #[test]
    fn lockfile_matches_detects_difference() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let original = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, original.to_toml_string().unwrap()).unwrap();
        let other = Lockfile::from_resolved(&[dep("x", "2.0.0", "../x")]);
        assert!(!lockfile_matches(&path, &other));
    }

    #[test]
    fn lockfile_matches_false_if_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("no-existe.lock");
        let l = Lockfile::from_resolved(&[]);
        assert!(!lockfile_matches(&path, &l));
    }

    #[test]
    fn write_lockfile_does_not_write_if_equal() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let l = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, l.to_toml_string().unwrap()).unwrap();

        let mtime_before = std::fs::metadata(&path).unwrap().modified().unwrap();
        // Wait a moment so that an eventual write produces a different mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));

        let wrote = write_lockfile_if_changed(&path, &l).unwrap();
        assert!(!wrote, "should not have written because contents match");

        let mtime_after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "should not have touched the file"
        );
    }

    #[test]
    fn write_lockfile_writes_if_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let old = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, old.to_toml_string().unwrap()).unwrap();

        let new = Lockfile::from_resolved(&[dep("x", "2.0.0", "../x")]);
        let wrote = write_lockfile_if_changed(&path, &new).unwrap();
        assert!(wrote, "should have written because contents changed");

        let on_disk = Lockfile::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, new);
    }

    #[test]
    fn write_lockfile_writes_if_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let l = Lockfile::from_resolved(&[]);
        let wrote = write_lockfile_if_changed(&path, &l).unwrap();
        assert!(wrote);
        assert!(path.is_file());
    }
}
