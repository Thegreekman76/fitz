//! Project manifest (`fitz.toml`) — Phase 9.y.1.
//!
//! First piece of the package manager. Defines the file format that
//! describes a Fitz project (name, version, entry point, deps) and the
//! minimal API to read it, write it, and resolve it from an arbitrary
//! directory.
//!
//! In 9.y.1 the manifest does not yet affect `fitz run`/`build`/`check`
//! — those consumers land in 9.y.2. Here we only deliver the format
//! + `fitz new`/`fitz init` that create it.
//!
//! Conventions closed in 9.y.1 (see `docs/roadmap.md` → 9.y):
//! - Format: TOML (`fitz.toml`).
//! - Structure: `src/main.fitz` as the default entry point.
//! - Versioned field: `edition = "2026"` (Cargo-style year).
//! - Single bin in MVP (`[bin] main = "..."`). Multi-bin with `[[bin]]`
//!   stays as debt 9.y.8+.
//! - Name validation: `^[a-z][a-z0-9_-]{0,63}$` (crates.io policy:
//!   lowercase + alphanumeric + `-`/`_`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Current language edition for `fitz new`. Cargo-style year; lets us
/// break compatibility without touching the compiler's semantic
/// version.
pub const CURRENT_EDITION: &str = "2026";

/// Manifest file name.
pub const MANIFEST_FILE: &str = "fitz.toml";

/// Full manifest of a Fitz project.
///
/// **About `[bin]` vs `[lib]`** (Phase 9.y.3.a): a project can be
/// executable (`[bin]`), library (`[lib]`), or both. `fitz run`/
/// `build` require `[bin]`. Path deps from OTHER projects require
/// `[lib]` (a bin-only project cannot be imported from another).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Bin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lib: Option<Lib>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,
    /// Phase 12.8 — Feature flags declared in the manifest. Section
    /// `[flags]` maps `flag-name = bool` for compile-time defaults.
    /// Runtime override via env vars `FITZ_FLAG_<UPPERCASE>`.
    /// TOML example:
    /// ```toml
    /// [flags]
    /// new-checkout = false
    /// dark-mode = true
    /// ```
    /// The binary loaded by `fitz run`/`build` reads this section at
    /// boot and passes it to `evaluator::set_flag_defaults`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bin {
    pub main: String,
}

/// Section `[lib]` of the manifest. Marks the project as a library
/// importable from other projects via path/git deps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lib {
    /// Path relative to the manifest of the library's entry. By
    /// convention `src/lib.fitz`, parallel to `[bin].main`.
    pub entry: String,
}

/// Declaration of a dependency in `[dependencies]`. Accepts two TOML
/// forms thanks to `serde(untagged)`:
///
/// - **Plain version** `foo = "1.2.3"` → `Dependency::Version("1.2.3")`.
///   Reserved for 9.y.5 (registry). When resolving today → clear error.
/// - **Detailed** `foo = { path = "../foo" }` → `Dependency::Detailed`.
///   Supports `path` (9.y.3.a) and the `git`/`tag`/`rev` fields we
///   reserve for 9.y.3.c (controlled rejection at resolve time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Version(String),
    Detailed(DetailedDependency),
}

/// Detailed form of a dependency. All fields are optional to allow the
/// incremental roadmap:
///
/// - 9.y.3.a uses `path`.
/// - 9.y.3.c adds `git` + `tag`/`rev`.
/// - 9.y.4 may add `version` to combine with git/registry.
///
/// Cross-validation (disallowing `path` + `git`, etc.) happens at
/// resolve time, not at parse — the manifest accepts any combination
/// and the resolver emits specific errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetailedDependency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Reserved for 9.y.3.c (git deps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Reserved for 9.y.3.c — tag to check out in the git dep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Reserved for 9.y.3.c — commit sha to check out in the git dep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

/// Manifest errors. Not integrated with `FitzError` (which is from the
/// language pipeline) — this is project tooling.
#[derive(Debug)]
pub enum ManifestError {
    /// Name does not match `^[a-z][a-z0-9_-]{0,63}$`.
    InvalidName(String),
    /// Failure parsing the TOML.
    Parse(toml::de::Error),
    /// Failure serializing to TOML.
    Serialize(toml::ser::Error),
    /// Dep that requires something not yet implemented (version without
    /// registry, git, etc.). Carries the dep name and the future
    /// sub-step that enables it, so the message is actionable.
    DepNotImplemented { name: String, reason: String },
    /// Dep `{path = "..."}` whose path does not exist on disk.
    DepPathNotFound { name: String, path: PathBuf },
    /// Dep `{path = "..."}` whose manifest exists but does not parse.
    DepManifestInvalid {
        name: String,
        path: PathBuf,
        source: Box<ManifestError>,
    },
    /// Dep `{path = "..."}` whose manifest has no `[lib]` section.
    /// Path deps are libraries by definition — if it only has `[bin]`,
    /// it cannot be imported.
    DepMissingLib { name: String, path: PathBuf },
    /// Dep with invalid shape: neither `path`, nor `git`, nor anything
    /// resolvable.
    DepInvalidShape { name: String },
    /// `git` dep with invalid shape: forbidden combination or missing
    /// tag/rev. `reason` is already formatted for the user.
    DepInvalidGitShape { name: String, reason: String },
    /// Clone/checkout/rev-parse of a git dep failed. Wraps the error
    /// from the `git_dep` module with the dep name so the message is
    /// actionable.
    DepGitError {
        name: String,
        source: crate::git_dep::GitDepError,
    },
    /// Error parsing the manifest with `toml_edit` (format-preserving
    /// edit path — `fitz add`/`remove`). Separate from `Parse` because
    /// that one comes from the serde-toml flow.
    EditParse(toml_edit::TomlError),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::InvalidName(n) => write!(
                f,
                "nombre de paquete inválido: `{n}`. Debe matchear \
                 `^[a-z][a-z0-9_-]{{0,63}}$` (lowercase, empezar con \
                 letra, contener solo letras/dígitos/`-`/`_`, máx 64 \
                 caracteres)."
            ),
            ManifestError::Parse(e) => write!(f, "error parseando manifest: {e}"),
            ManifestError::Serialize(e) => {
                write!(f, "error serializando manifest: {e}")
            }
            ManifestError::DepNotImplemented { name, reason } => {
                write!(f, "dep `{name}`: {reason}")
            }
            ManifestError::DepPathNotFound { name, path } => {
                write!(f, "dep `{name}`: el path `{}` no existe.", path.display())
            }
            ManifestError::DepManifestInvalid { name, path, source } => write!(
                f,
                "dep `{name}`: manifest en `{}` inválido — {source}",
                path.display()
            ),
            ManifestError::DepMissingLib { name, path } => write!(
                f,
                "dep `{name}`: `{}` no tiene sección `[lib]`. Las path \
                 deps son librerías; agregá:\n\n[lib]\nentry = \"src/lib.fitz\"\n",
                path.display()
            ),
            ManifestError::DepInvalidShape { name } => write!(
                f,
                "dep `{name}`: debe especificar `path = \"...\"` o \
                 `git = \"...\"` con `tag`/`rev` (versiones sueltas \
                 tipo `\"1.0.0\"` llegan con el registry en 9.y.5)."
            ),
            ManifestError::DepInvalidGitShape { name, reason } => {
                write!(f, "dep `{name}` (git): {reason}")
            }
            ManifestError::DepGitError { name, source } => {
                write!(f, "dep `{name}` (git): {source}")
            }
            ManifestError::EditParse(e) => write!(f, "error parseando manifest: {e}"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    /// Builds a default manifest for a new project:
    /// - version `0.1.0`
    /// - current edition
    /// - bin with entry `src/main.fitz`
    /// - no deps
    ///
    /// Validates the name.
    pub fn new_default(name: &str) -> Result<Self, ManifestError> {
        if !is_valid_package_name(name) {
            return Err(ManifestError::InvalidName(name.to_string()));
        }
        Ok(Manifest {
            package: Package {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                edition: CURRENT_EDITION.to_string(),
                authors: Vec::new(),
                description: None,
                license: None,
            },
            bin: Some(Bin {
                main: "src/main.fitz".to_string(),
            }),
            lib: None,
            dependencies: BTreeMap::new(),
            flags: BTreeMap::new(),
        })
    }

    /// Parses a manifest from TOML text.
    pub fn parse(input: &str) -> Result<Self, ManifestError> {
        toml::from_str(input).map_err(ManifestError::Parse)
    }

    /// Serializes the manifest to TOML.
    pub fn to_toml_string(&self) -> Result<String, ManifestError> {
        toml::to_string(self).map_err(ManifestError::Serialize)
    }
}

/// Validates a package name against the crates.io-style policy:
/// `^[a-z][a-z0-9_-]{0,63}$`. Hand-written implementation to avoid a
/// `regex` dep (it's small; doesn't justify the weight right now).
pub fn is_valid_package_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Searches for the nearest `fitz.toml` walking up from `start`. Cargo-style:
/// the manifest may live in the current directory or an ancestor.
/// Returns the absolute path to the file if found.
///
/// Consumed by `resolve_entry` in `main.rs` since Phase 9.y.2 (when
/// `fitz run`/`build`/`check` accept being invoked without a file).
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    if current.is_relative() {
        if let Ok(abs) = std::fs::canonicalize(&current) {
            current = abs;
        }
    }
    loop {
        let candidate = current.join(MANIFEST_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

// ---- Phase 9.y.4 — format-preserving manifest editing ----

/// Spec of a dep for `fitz add`. Alias of the shape the CLI parses
/// from flags (`--path`, `--git`, `--tag`, `--rev`).
#[derive(Debug, Clone, PartialEq)]
pub enum AddDepSpec {
    Path {
        path: String,
    },
    Git {
        url: String,
        gitref: crate::git_dep::GitRef,
    },
}

/// Adds a dep to `fitz.toml` (text). Preserves user comments and
/// formatting thanks to `toml_edit`. If an entry with the same name
/// already existed, it overwrites it (cargo-style). If `[dependencies]`
/// did not exist, it creates it.
///
/// Returns the updated TOML text. Does not persist to disk — that is
/// the caller's responsibility (typically `main.rs::add_dep`).
pub fn add_dep_to_manifest(
    existing_text: &str,
    name: &str,
    spec: &AddDepSpec,
) -> Result<String, ManifestError> {
    let mut doc: toml_edit::DocumentMut =
        existing_text.parse().map_err(ManifestError::EditParse)?;

    // Ensure [dependencies] exists as a table.
    if !doc.contains_key("dependencies") {
        doc["dependencies"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let deps = doc["dependencies"]
        .as_table_mut()
        .ok_or_else(|| ManifestError::InvalidName("[dependencies] no es una tabla".to_string()))?;

    // Build the inline table for the new dep.
    let inline = build_inline_dep_table(spec);
    deps.insert(
        name,
        toml_edit::Item::Value(toml_edit::Value::InlineTable(inline)),
    );

    Ok(doc.to_string())
}

/// Removes a dep from `fitz.toml` (text). If the dep does not exist in
/// `[dependencies]`, returns `false` (no error — the caller decides
/// whether to report it as warning or error depending on the command).
/// If `[dependencies]` ends up empty after removing the entry, the
/// section is deleted entirely so as not to leave noise.
pub fn remove_dep_from_manifest(
    existing_text: &str,
    name: &str,
) -> Result<(String, bool), ManifestError> {
    let mut doc: toml_edit::DocumentMut =
        existing_text.parse().map_err(ManifestError::EditParse)?;

    let Some(deps_item) = doc.get_mut("dependencies") else {
        return Ok((doc.to_string(), false));
    };
    let Some(deps) = deps_item.as_table_mut() else {
        return Ok((doc.to_string(), false));
    };

    let removed = deps.remove(name).is_some();
    let is_now_empty = deps.is_empty();
    if is_now_empty {
        doc.remove("dependencies");
    }
    Ok((doc.to_string(), removed))
}

/// Builds the TOML inline table for a dep added by `fitz add`. The
/// shape is `{ path = "..." }` or `{ git = "...", tag = "..." }` /
/// `{ git = "...", rev = "..." }`.
fn build_inline_dep_table(spec: &AddDepSpec) -> toml_edit::InlineTable {
    let mut table = toml_edit::InlineTable::new();
    match spec {
        AddDepSpec::Path { path } => {
            table.insert("path", toml_edit::Value::from(path.as_str()));
        }
        AddDepSpec::Git { url, gitref } => {
            table.insert("git", toml_edit::Value::from(url.as_str()));
            match gitref {
                crate::git_dep::GitRef::Tag(t) => {
                    table.insert("tag", toml_edit::Value::from(t.as_str()));
                }
                crate::git_dep::GitRef::Rev(r) => {
                    table.insert("rev", toml_edit::Value::from(r.as_str()));
                }
            }
        }
    }
    table
}

// ---- Phase 9.y.3.b — dep registry consumed by evaluator + codegen ----

/// Lightweight map `dep-name → absolute-lib_entry` that we pass to the
/// evaluator's loader (`fitz run`) and the codegen's loader (`fitz build`).
///
/// Phase 9.y.3.b: the loader checks this BEFORE falling back to paths
/// relative to the importer. When `from utils_lib import X` appears and
/// `utils_lib` is here, the loader loads directly from `lib_entry`.
///
/// Hyphens in package names: accepted in the manifest, but CANNOT
/// appear in Fitz imports because the parser does not accept `-` in
/// identifiers. A dep `utils-lib` ends up in the registry but
/// `from utils-lib import X` does not parse. Convention until 9.y.4:
/// name deps with `_` or no separator if you plan to import them.
pub type DepRegistry = std::collections::HashMap<String, PathBuf>;

/// Helper: builds the registry from a list of `ResolvedDep`s. Each dep
/// goes in once by its `name` (the name with which it appears in the
/// importer's `[dependencies]`).
pub fn build_dep_registry(deps: &[ResolvedDep]) -> DepRegistry {
    deps.iter()
        .map(|d| (d.name.clone(), d.lib_entry.clone()))
        .collect()
}

// ---- Phase 9.y.3.a — dependency resolution ----

/// A dep resolved to an absolute path + metadata needed by the lockfile
/// and (eventually, 9.y.3.b) the loader to locate the code.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedDep {
    /// Name under which the dep appears in the importer's
    /// `[dependencies]`. May differ from the `package.name` of the
    /// dep's manifest if the user uses aliasing (future 9.y.4); for
    /// now they are equal.
    pub name: String,
    /// `[package].version` from the dep's manifest. For path deps this
    /// is purely informational (not validated against ranges — the
    /// roadmap for semver constraints lands with 9.y.5).
    pub version: String,
    /// Absolute path to the dep's root directory (where its `fitz.toml`
    /// lives).
    pub abs_path: PathBuf,
    /// Absolute path to the library's entry (`[lib].entry` of the dep,
    /// resolved against `abs_path`).
    pub lib_entry: PathBuf,
    /// Source kind, to distinguish in the lockfile and (future 9.y.3.b)
    /// in the cache. For now only `Path`.
    pub source: ResolvedDepSource,
}

/// Origin kind of a resolved dep.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedDepSource {
    /// Path dep. The field stores the path exactly as it appears in
    /// the importer's manifest (not canonicalized) to preserve the
    /// user's intent in messages and lockfile diffs.
    Path { declared: String },
    /// Git dep (9.y.3.c). `url` is the URL exactly as the user
    /// declared it; `requested` distinguishes Tag/Rev; `commit_hash`
    /// is the exact SHA we ended up checking out (consumed by the
    /// lockfile as `source = "git+<url>#<commit>"`).
    Git {
        url: String,
        requested: crate::git_dep::GitRef,
        commit_hash: String,
    },
    // `Registry { url, version }` lands in 9.y.5.
}

/// Resolves ALL deps from the manifest against the filesystem.
/// Returns `ResolvedDep`s sorted by name (deterministic, important
/// for lockfile diffs).
///
/// `manifest_dir` is the directory that contains the importer's
/// `fitz.toml`; it is used to resolve relative `path`s.
///
/// Errors: stops at the first dep that fails. If you want to
/// accumulate all errors, a refactor is needed (future sub-step).
pub fn resolve_dependencies(
    manifest: &Manifest,
    manifest_dir: &Path,
) -> Result<Vec<ResolvedDep>, ManifestError> {
    let mut resolved = Vec::with_capacity(manifest.dependencies.len());
    for (name, dep) in &manifest.dependencies {
        resolved.push(resolve_single_dep(name, dep, manifest_dir)?);
    }
    Ok(resolved)
}

fn resolve_single_dep(
    name: &str,
    dep: &Dependency,
    manifest_dir: &Path,
) -> Result<ResolvedDep, ManifestError> {
    match dep {
        Dependency::Version(_) => Err(ManifestError::DepNotImplemented {
            name: name.to_string(),
            reason: "las deps con versión suelta (`foo = \"1.0.0\"`) requieren \
                     el registry, que llega en 9.y.5. Por ahora usá \
                     `foo = { path = \"...\" }` o `foo = { git = \"...\", tag = \"...\" }`."
                .to_string(),
        }),
        Dependency::Detailed(d) => {
            let has_path = d.path.is_some();
            let has_git = d.git.is_some();

            // path + git: invalid combination (which priority?).
            if has_path && has_git {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "no se puede combinar `path` con `git` en la misma dep. \
                             Usá uno u otro."
                        .to_string(),
                });
            }

            if has_path {
                // path-only deps (9.y.3.a) — the remaining git-related
                // fields are silently ignored. We do not error out so
                // as not to break the case where the user is iterating
                // between `path` and `git`.
                let path_str = d.path.as_ref().expect("has_path checked");
                return resolve_path_dep(name, path_str, manifest_dir);
            }

            if has_git {
                let url = d.git.as_ref().expect("has_git checked");
                let gitref = parse_git_ref(name, d.tag.as_deref(), d.rev.as_deref())?;
                return resolve_git_dep(name, url, gitref);
            }

            // Neither path nor git but one of tag/rev — user forgot
            // the url. Specific message.
            if d.tag.is_some() || d.rev.is_some() {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "`tag`/`rev` requieren también `git = \"<url>\"`.".to_string(),
                });
            }

            Err(ManifestError::DepInvalidShape {
                name: name.to_string(),
            })
        }
    }
}

/// Validates tag/rev: exactly one of the two must be present.
fn parse_git_ref(
    name: &str,
    tag: Option<&str>,
    rev: Option<&str>,
) -> Result<crate::git_dep::GitRef, ManifestError> {
    match (tag, rev) {
        (Some(_), Some(_)) => Err(ManifestError::DepInvalidGitShape {
            name: name.to_string(),
            reason: "`tag` y `rev` son mutuamente exclusivos — elegí uno.".to_string(),
        }),
        (Some(t), None) => {
            if t.trim().is_empty() {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "`tag` no puede ser vacío.".to_string(),
                });
            }
            Ok(crate::git_dep::GitRef::Tag(t.to_string()))
        }
        (None, Some(r)) => {
            if r.trim().is_empty() {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "`rev` no puede ser vacío.".to_string(),
                });
            }
            Ok(crate::git_dep::GitRef::Rev(r.to_string()))
        }
        (None, None) => Err(ManifestError::DepInvalidGitShape {
            name: name.to_string(),
            reason: "git deps requieren `tag = \"...\"` o `rev = \"...\"` para reproducibilidad. \
                     `branch` no se soporta intencionalmente (mutables → builds no reproducibles)."
                .to_string(),
        }),
    }
}

/// Resolution of git deps (9.y.3.c). Clones or reuses the cache, reads
/// the dep's manifest, validates `[lib]`, and returns a `ResolvedDep`
/// with `source = ResolvedDepSource::Git { ... }`.
fn resolve_git_dep(
    name: &str,
    url: &str,
    gitref: crate::git_dep::GitRef,
) -> Result<ResolvedDep, ManifestError> {
    let cloned = crate::git_dep::clone_or_use_cache(url, &gitref).map_err(|e| {
        ManifestError::DepGitError {
            name: name.to_string(),
            source: e,
        }
    })?;

    let dep_manifest_path = cloned.abs_path.join(MANIFEST_FILE);
    if !dep_manifest_path.is_file() {
        return Err(ManifestError::DepPathNotFound {
            name: name.to_string(),
            path: dep_manifest_path,
        });
    }

    let dep_manifest_text = std::fs::read_to_string(&dep_manifest_path).map_err(|_| {
        ManifestError::DepPathNotFound {
            name: name.to_string(),
            path: dep_manifest_path.clone(),
        }
    })?;
    let dep_manifest =
        Manifest::parse(&dep_manifest_text).map_err(|e| ManifestError::DepManifestInvalid {
            name: name.to_string(),
            path: dep_manifest_path.clone(),
            source: Box::new(e),
        })?;

    let lib = match dep_manifest.lib {
        Some(l) => l,
        None => {
            return Err(ManifestError::DepMissingLib {
                name: name.to_string(),
                path: dep_manifest_path,
            })
        }
    };

    let lib_entry = cloned.abs_path.join(&lib.entry);
    if !lib_entry.is_file() {
        return Err(ManifestError::DepPathNotFound {
            name: name.to_string(),
            path: lib_entry,
        });
    }

    Ok(ResolvedDep {
        name: name.to_string(),
        version: dep_manifest.package.version,
        abs_path: cloned.abs_path,
        lib_entry,
        source: ResolvedDepSource::Git {
            url: url.to_string(),
            requested: gitref,
            commit_hash: cloned.commit_hash,
        },
    })
}

fn resolve_path_dep(
    name: &str,
    path_str: &str,
    manifest_dir: &Path,
) -> Result<ResolvedDep, ManifestError> {
    let raw_path = manifest_dir.join(path_str);
    let abs_path =
        std::fs::canonicalize(&raw_path).map_err(|_| ManifestError::DepPathNotFound {
            name: name.to_string(),
            path: raw_path.clone(),
        })?;

    let dep_manifest_path = abs_path.join(MANIFEST_FILE);
    if !dep_manifest_path.is_file() {
        return Err(ManifestError::DepPathNotFound {
            name: name.to_string(),
            path: dep_manifest_path,
        });
    }

    let dep_manifest_text = std::fs::read_to_string(&dep_manifest_path).map_err(|_| {
        ManifestError::DepPathNotFound {
            name: name.to_string(),
            path: dep_manifest_path.clone(),
        }
    })?;
    let dep_manifest =
        Manifest::parse(&dep_manifest_text).map_err(|e| ManifestError::DepManifestInvalid {
            name: name.to_string(),
            path: dep_manifest_path.clone(),
            source: Box::new(e),
        })?;

    let lib = match dep_manifest.lib {
        Some(l) => l,
        None => {
            return Err(ManifestError::DepMissingLib {
                name: name.to_string(),
                path: dep_manifest_path,
            })
        }
    };

    let lib_entry = abs_path.join(&lib.entry);
    if !lib_entry.is_file() {
        return Err(ManifestError::DepPathNotFound {
            name: name.to_string(),
            path: lib_entry,
        });
    }

    Ok(ResolvedDep {
        name: name.to_string(),
        version: dep_manifest.package.version,
        abs_path,
        lib_entry,
        source: ResolvedDepSource::Path {
            declared: path_str.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombres_validos_pasan() {
        assert!(is_valid_package_name("fitz-uuid"));
        assert!(is_valid_package_name("a"));
        assert!(is_valid_package_name("mi-app"));
        assert!(is_valid_package_name("my_lib"));
        assert!(is_valid_package_name("http2"));
        assert!(is_valid_package_name("a-b-c-d-e"));
    }

    #[test]
    fn nombres_invalidos_fallan() {
        assert!(!is_valid_package_name(""), "vacío");
        assert!(!is_valid_package_name("Foo"), "mayúscula inicial");
        assert!(!is_valid_package_name("FOO"), "todas mayúsculas");
        assert!(!is_valid_package_name("1foo"), "empieza con dígito");
        assert!(!is_valid_package_name("-foo"), "empieza con guión");
        assert!(!is_valid_package_name("_foo"), "empieza con guión bajo");
        assert!(!is_valid_package_name("foo bar"), "espacio");
        assert!(!is_valid_package_name("foo.bar"), "punto");
        assert!(!is_valid_package_name("foo/bar"), "slash");
        assert!(!is_valid_package_name("foo@bar"), "arroba");
        assert!(!is_valid_package_name(&"a".repeat(65)), "más de 64 chars");
    }

    #[test]
    fn nombre_de_64_caracteres_es_valido() {
        // Cota inclusiva: 64 OK, 65 no (test arriba).
        assert!(is_valid_package_name(&"a".repeat(64)));
    }

    #[test]
    fn new_default_emite_manifest_consistente() {
        let m = Manifest::new_default("mi-app").unwrap();
        assert_eq!(m.package.name, "mi-app");
        assert_eq!(m.package.version, "0.1.0");
        assert_eq!(m.package.edition, CURRENT_EDITION);
        assert!(m.package.authors.is_empty());
        assert_eq!(m.bin.as_ref().unwrap().main, "src/main.fitz");
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn new_default_rechaza_nombre_invalido() {
        let err = Manifest::new_default("Foo").unwrap_err();
        match err {
            ManifestError::InvalidName(n) => assert_eq!(n, "Foo"),
            other => panic!("se esperaba InvalidName, fue {other:?}"),
        }
    }

    #[test]
    fn round_trip_preserva_fields_basicos() {
        let original = Manifest::new_default("test-app").unwrap();
        let toml = original.to_toml_string().unwrap();
        let parsed = Manifest::parse(&toml).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serializacion_omite_fields_opcionales_vacios() {
        let m = Manifest::new_default("mi-app").unwrap();
        let toml = m.to_toml_string().unwrap();
        // `authors`, `description`, `license`, `dependencies` must be
        // omitted when empty (skip_serializing_if).
        assert!(!toml.contains("authors"));
        assert!(!toml.contains("description"));
        assert!(!toml.contains("license"));
        assert!(!toml.contains("[dependencies]"));
    }

    #[test]
    fn parse_acepta_manifest_minimo() {
        let toml_text = r#"
[package]
name = "mi-app"
version = "0.1.0"
edition = "2026"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.package.name, "mi-app");
        assert!(m.bin.is_none());
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn parse_acepta_manifest_completo() {
        let toml_text = r#"
[package]
name = "mi-app"
version = "0.2.1"
edition = "2026"
authors = ["Ada <ada@example.com>", "Linus"]
description = "una app de prueba"
license = "MIT"

[bin]
main = "src/main.fitz"

[dependencies]
fitz-uuid = "1.0.0"
http-helpers = "0.3.2"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.package.name, "mi-app");
        assert_eq!(m.package.version, "0.2.1");
        assert_eq!(m.package.authors.len(), 2);
        assert_eq!(m.package.description.as_deref(), Some("una app de prueba"));
        assert_eq!(m.package.license.as_deref(), Some("MIT"));
        assert_eq!(m.bin.unwrap().main, "src/main.fitz");
        assert_eq!(m.dependencies.len(), 2);
        match m.dependencies.get("fitz-uuid").unwrap() {
            Dependency::Version(v) => assert_eq!(v, "1.0.0"),
            other => panic!("se esperaba Version, fue {other:?}"),
        }
    }

    // ---- Phase 12.8 — [flags] section ----

    #[test]
    fn flags_seccion_vacia_o_ausente_default_empty_map() {
        let toml_text = r#"
[package]
name = "mi-app"
version = "0.1.0"
edition = "2026"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert!(m.flags.is_empty());
    }

    #[test]
    fn flags_seccion_parsea_pares_bool() {
        let toml_text = r#"
[package]
name = "mi-app"
version = "0.1.0"
edition = "2026"

[flags]
new-checkout = false
dark-mode = true
beta_feature = true
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.flags.len(), 3);
        assert_eq!(m.flags.get("new-checkout").copied(), Some(false));
        assert_eq!(m.flags.get("dark-mode").copied(), Some(true));
        assert_eq!(m.flags.get("beta_feature").copied(), Some(true));
    }

    #[test]
    fn flags_no_bool_value_es_error_de_parse() {
        // TOML strict: the `[flags]` field expects bool; if the value
        // is a string it fails with a clear parse error.
        let toml_text = r#"
[package]
name = "mi-app"
version = "0.1.0"
edition = "2026"

[flags]
foo = "yes"
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse(_)),
            "esperaba ManifestError::Parse, fue {err:?}"
        );
    }

    #[test]
    fn parse_falla_con_field_faltante_obligatorio() {
        let toml_text = r#"
[package]
name = "mi-app"
# falta version y edition
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn find_manifest_encuentra_en_dir_actual() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest_path = tmp.path().join(MANIFEST_FILE);
        std::fs::write(
            &manifest_path,
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .unwrap();
        let found = find_manifest(tmp.path()).unwrap();
        // Canonicalize both sides — on Windows the tmp can expand
        // `~/AppData/Local/Temp` to a different path than tempfile's.
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&manifest_path).unwrap()
        );
    }

    #[test]
    fn find_manifest_camina_hacia_arriba() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest_path = tmp.path().join(MANIFEST_FILE);
        std::fs::write(
            &manifest_path,
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        let found = find_manifest(&nested).unwrap();
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&manifest_path).unwrap()
        );
    }

    // ---- Phase 9.y.3.a — Dependency / Lib / resolve_dependencies ----

    #[test]
    fn parse_dependency_version_corta() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[dependencies]
foo = "1.0.0"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        match m.dependencies.get("foo").unwrap() {
            Dependency::Version(v) => assert_eq!(v, "1.0.0"),
            other => panic!("se esperaba Version, fue {other:?}"),
        }
    }

    #[test]
    fn parse_dependency_path() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[dependencies]
utils = { path = "../utils" }
"#;
        let m = Manifest::parse(toml_text).unwrap();
        match m.dependencies.get("utils").unwrap() {
            Dependency::Detailed(d) => {
                assert_eq!(d.path.as_deref(), Some("../utils"));
                assert!(d.git.is_none());
            }
            other => panic!("se esperaba Detailed, fue {other:?}"),
        }
    }

    #[test]
    fn parse_dependency_git_reservada_se_acepta_a_nivel_parse() {
        // El parser acepta la forma; el resolver es quien rechaza.
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[dependencies]
helpers = { git = "https://github.com/foo/bar", tag = "v1.0.0" }
"#;
        let m = Manifest::parse(toml_text).unwrap();
        match m.dependencies.get("helpers").unwrap() {
            Dependency::Detailed(d) => {
                assert_eq!(d.git.as_deref(), Some("https://github.com/foo/bar"));
                assert_eq!(d.tag.as_deref(), Some("v1.0.0"));
                assert!(d.path.is_none());
            }
            other => panic!("se esperaba Detailed, fue {other:?}"),
        }
    }

    #[test]
    fn parse_seccion_lib() {
        let toml_text = r#"
[package]
name = "mi-lib"
version = "0.1.0"
edition = "2026"

[lib]
entry = "src/lib.fitz"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.lib.as_ref().unwrap().entry, "src/lib.fitz");
        assert!(m.bin.is_none());
    }

    /// Helper: creates a candidate path-dep project (manifest with
    /// `[lib]` + empty entry file) in `target/`. Returns the absolute
    /// path to the created directory.
    fn scaffold_lib_dep(target: &Path, name: &str, version: &str) -> PathBuf {
        let dir = target.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("fitz.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2026\"\n\n[lib]\nentry = \"src/lib.fitz\"\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.fitz"), "// lib vacía\n").unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn resolve_dependencies_path_dep_devuelve_resolved_dep() {
        let tmp = tempfile::tempdir().unwrap();
        let _abs_utils = scaffold_lib_dep(tmp.path(), "utils", "0.1.0");

        let importer_dir = tmp.path().join("importer");
        std::fs::create_dir_all(&importer_dir).unwrap();
        let manifest = {
            let mut m = Manifest::new_default("importer").unwrap();
            m.dependencies.insert(
                "utils".to_string(),
                Dependency::Detailed(DetailedDependency {
                    path: Some("../utils".to_string()),
                    git: None,
                    tag: None,
                    rev: None,
                }),
            );
            m
        };

        let resolved = resolve_dependencies(&manifest, &importer_dir).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "utils");
        assert_eq!(resolved[0].version, "0.1.0");
        assert!(
            resolved[0].lib_entry.ends_with("src/lib.fitz")
                || resolved[0].lib_entry.ends_with("src\\lib.fitz")
        );
        match &resolved[0].source {
            ResolvedDepSource::Path { declared } => assert_eq!(declared, "../utils"),
            other => panic!("se esperaba ResolvedDepSource::Path, fue {other:?}"),
        }
    }

    #[test]
    fn resolve_dependencies_version_corta_aborta_citando_9y5() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies
            .insert("foo".to_string(), Dependency::Version("1.0.0".to_string()));
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("9.y.5"), "msg: {msg}");
        assert!(msg.contains("foo"), "msg: {msg}");
    }

    // (The old test `resolve_dependencies_git_aborta_citando_9y3c`
    // was removed when 9.y.3.c closed — git deps are now supported.
    // Shape validation is covered by the `resolve_git_dep_*` tests
    // above.)

    // ---- Phase 9.y.4 — manifest editing (add/remove preserve format) ----

    #[test]
    fn add_dep_a_manifest_sin_dependencies_crea_la_seccion() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n";
        let spec = AddDepSpec::Path {
            path: "../utils".to_string(),
        };
        let updated = add_dep_to_manifest(original, "utils", &spec).unwrap();
        assert!(updated.contains("[dependencies]"));
        assert!(updated.contains("utils = { path = \"../utils\" }"));
        // The rest of the manifest stays intact.
        assert!(updated.contains("[package]"));
        assert!(updated.contains("[bin]"));
    }

    #[test]
    fn add_dep_path_emite_inline_table() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nya = { path = \"../ya\" }\n";
        let spec = AddDepSpec::Path {
            path: "../nuevo".to_string(),
        };
        let updated = add_dep_to_manifest(original, "nuevo", &spec).unwrap();
        assert!(updated.contains("nuevo = { path = \"../nuevo\" }"));
        // The previous dep is still there.
        assert!(updated.contains("ya = { path = \"../ya\" }"));
    }

    #[test]
    fn add_dep_git_con_tag_emite_inline_table() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let spec = AddDepSpec::Git {
            url: "https://github.com/foo/bar".to_string(),
            gitref: crate::git_dep::GitRef::Tag("v1.0.0".to_string()),
        };
        let updated = add_dep_to_manifest(original, "bar", &spec).unwrap();
        assert!(
            updated.contains("bar = { git = \"https://github.com/foo/bar\", tag = \"v1.0.0\" }"),
            "manifest:\n{updated}"
        );
    }

    #[test]
    fn add_dep_git_con_rev_emite_inline_table() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let spec = AddDepSpec::Git {
            url: "https://x.com/r".to_string(),
            gitref: crate::git_dep::GitRef::Rev("abc123".to_string()),
        };
        let updated = add_dep_to_manifest(original, "r", &spec).unwrap();
        assert!(
            updated.contains("r = { git = \"https://x.com/r\", rev = \"abc123\" }"),
            "manifest:\n{updated}"
        );
    }

    #[test]
    fn add_dep_sobreescribe_si_ya_existia() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nfoo = { path = \"../viejo\" }\n";
        let spec = AddDepSpec::Path {
            path: "../nuevo".to_string(),
        };
        let updated = add_dep_to_manifest(original, "foo", &spec).unwrap();
        assert!(updated.contains("foo = { path = \"../nuevo\" }"));
        assert!(
            !updated.contains("../viejo"),
            "el path viejo no debió persistir:\n{updated}"
        );
    }

    #[test]
    fn add_dep_preserva_comentarios_del_usuario() {
        let original = "# Mi proyecto Fitz\n[package]\nname = \"x\"  # NOTA: cambiar antes de publicar\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"  # entry CLI\n";
        let spec = AddDepSpec::Path {
            path: "../u".to_string(),
        };
        let updated = add_dep_to_manifest(original, "u", &spec).unwrap();
        assert!(updated.contains("# Mi proyecto Fitz"));
        assert!(updated.contains("# NOTA: cambiar antes de publicar"));
        assert!(updated.contains("# entry CLI"));
    }

    #[test]
    fn remove_dep_quita_entry_y_reporta_true() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nuno = { path = \"../uno\" }\ndos = { path = \"../dos\" }\n";
        let (updated, removed) = remove_dep_from_manifest(original, "uno").unwrap();
        assert!(removed);
        assert!(!updated.contains("uno = "));
        assert!(updated.contains("dos = { path = \"../dos\" }"));
    }

    #[test]
    fn remove_dep_reporta_false_si_no_existia() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nuno = { path = \"../uno\" }\n";
        let (updated, removed) = remove_dep_from_manifest(original, "no-existe").unwrap();
        assert!(!removed);
        // The manifest stays intact.
        assert!(updated.contains("uno = { path = \"../uno\" }"));
    }

    #[test]
    fn remove_dep_borra_seccion_si_queda_vacia() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nuno = { path = \"../uno\" }\n";
        let (updated, removed) = remove_dep_from_manifest(original, "uno").unwrap();
        assert!(removed);
        assert!(
            !updated.contains("[dependencies]"),
            "[dependencies] debió borrarse al quedar vacío:\n{updated}"
        );
    }

    #[test]
    fn remove_dep_sin_seccion_dependencies_es_no_op() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let (updated, removed) = remove_dep_from_manifest(original, "foo").unwrap();
        assert!(!removed);
        assert_eq!(updated, original);
    }

    #[test]
    fn add_y_remove_son_inverso_aproximadamente() {
        // After add+remove of the SAME dep, the manifest must end up
        // semantically equivalent to the original. We accept small
        // formatting differences (toml_edit may normalize whitespace)
        // but the shape is the same.
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let after_add = add_dep_to_manifest(
            original,
            "tmp",
            &AddDepSpec::Path {
                path: "../t".to_string(),
            },
        )
        .unwrap();
        let (after_remove, removed) = remove_dep_from_manifest(&after_add, "tmp").unwrap();
        assert!(removed);
        // The parser must see the same manifest semantically.
        let m1 = Manifest::parse(original).unwrap();
        let m2 = Manifest::parse(&after_remove).unwrap();
        assert_eq!(m1, m2);
    }

    #[test]
    fn resolve_dependencies_path_inexistente_aborta() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "ghost".to_string(),
            Dependency::Detailed(DetailedDependency {
                path: Some("../no-existe".to_string()),
                git: None,
                tag: None,
                rev: None,
            }),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::DepPathNotFound { .. }));
    }

    #[test]
    fn resolve_dependencies_path_sin_lib_aborta() {
        let tmp = tempfile::tempdir().unwrap();
        let dep_dir = tmp.path().join("solo-bin");
        std::fs::create_dir_all(dep_dir.join("src")).unwrap();
        // Manifest with only [bin], no [lib] — cannot be imported.
        std::fs::write(
            dep_dir.join("fitz.toml"),
            "[package]\nname = \"solo-bin\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n",
        )
        .unwrap();
        std::fs::write(dep_dir.join("src/main.fitz"), "print(\"x\")\n").unwrap();

        let importer_dir = tmp.path().join("importer");
        std::fs::create_dir_all(&importer_dir).unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "solo-bin".to_string(),
            Dependency::Detailed(DetailedDependency {
                path: Some("../solo-bin".to_string()),
                git: None,
                tag: None,
                rev: None,
            }),
        );
        let err = resolve_dependencies(&m, &importer_dir).unwrap_err();
        assert!(matches!(err, ManifestError::DepMissingLib { .. }));
        // The message suggests adding [lib].
        let msg = err.to_string();
        assert!(msg.contains("[lib]"), "msg: {msg}");
        assert!(msg.contains("entry"), "msg: {msg}");
    }

    // ---- Phase 9.y.3.c — git deps shape validations ----

    fn git_dep(
        path: Option<&str>,
        git: Option<&str>,
        tag: Option<&str>,
        rev: Option<&str>,
    ) -> Dependency {
        Dependency::Detailed(DetailedDependency {
            path: path.map(|s| s.to_string()),
            git: git.map(|s| s.to_string()),
            tag: tag.map(|s| s.to_string()),
            rev: rev.map(|s| s.to_string()),
        })
    }

    #[test]
    fn resolve_git_dep_sin_tag_ni_rev_aborta_pidiendo_uno() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "helpers".to_string(),
            git_dep(None, Some("https://example.com/r"), None, None),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("tag") && msg.contains("rev"), "msg: {msg}");
        assert!(
            msg.contains("reproducibilidad"),
            "msg debería citar reproducibilidad: {msg}"
        );
    }

    #[test]
    fn resolve_git_dep_con_tag_y_rev_juntos_aborta() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "helpers".to_string(),
            git_dep(None, Some("https://example.com/r"), Some("v1"), Some("abc")),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mutuamente exclusivos"), "msg: {msg}");
    }

    #[test]
    fn resolve_git_dep_tag_vacio_aborta() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "helpers".to_string(),
            git_dep(None, Some("https://example.com/r"), Some("  "), None),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`tag` no puede ser vacío"), "msg: {msg}");
    }

    #[test]
    fn resolve_path_y_git_juntos_aborta_combinacion_invalida() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "x".to_string(),
            git_dep(
                Some("../x"),
                Some("https://example.com/r"),
                Some("v1"),
                None,
            ),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no se puede combinar"), "msg: {msg}");
    }

    #[test]
    fn resolve_tag_sin_git_aborta_pidiendo_url() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies
            .insert("x".to_string(), git_dep(None, None, Some("v1"), None));
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("requieren también `git"), "msg: {msg}");
    }

    #[test]
    fn parse_git_ref_devuelve_tag_o_rev_correcto() {
        let t = parse_git_ref("x", Some("v1.0.0"), None).unwrap();
        assert_eq!(t, crate::git_dep::GitRef::Tag("v1.0.0".to_string()));

        let r = parse_git_ref("x", None, Some("abc123")).unwrap();
        assert_eq!(r, crate::git_dep::GitRef::Rev("abc123".to_string()));
    }

    #[test]
    fn resolve_dependencies_detailed_vacia_aborta_invalid_shape() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "vacia".to_string(),
            Dependency::Detailed(DetailedDependency {
                path: None,
                git: None,
                tag: None,
                rev: None,
            }),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::DepInvalidShape { .. }));
    }

    #[test]
    fn find_manifest_devuelve_none_si_no_hay() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        // We do not create fitz.toml. Walks up to the FS root without
        // finding it (assuming there is no fitz.toml at the system
        // root, which is reasonable).
        assert!(find_manifest(&nested).is_none());
    }
}
