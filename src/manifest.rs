//! Project manifest (`fitz.toml`) — Phase 9.y.1 + Phase 11.5.b.
//!
//! First piece of the package manager. Defines the file format that
//! describes a Fitz project (name, version, entry point, deps) and the
//! minimal API to read it, write it, and resolve it from an arbitrary
//! directory.
//!
//! Conventions closed in 9.y.1 (see `docs/roadmap.md` → 9.y):
//! - Format: TOML (`fitz.toml`).
//! - Structure: `src/main.fitz` as the default entry point.
//! - Versioned field: `edition = "2026"` (Cargo-style year).
//! - Name validation: `^[a-z][a-z0-9_-]{0,63}$` (crates.io policy:
//!   lowercase + alphanumeric + `-`/`_`).
//!
//! Phase 11.5.b (2026-07-15) — closed debt 9.y.8+ (multi-bin):
//! - `[[bin]]` array-of-tables is now the canonical shape.
//! - Legacy `[bin]` singular auto-migrates to a single-entry
//!   `[[bin]]` — zero breaking change for the ~40 boilerplates +
//!   course examples that use it today.
//! - Each entry gains `name` / `target` / `mount` fields.
//! - `target` accepts `"native"` (default), `"wasm-client"`, or
//!   `"ssr"` (reserved for 11.6+, accepted at parse but rejected
//!   at build with a targeted message).

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
///
/// **Multi-bin** (Phase 11.5.b): `bins` is `Vec<ManifestBin>`. The
/// TOML shape accepts BOTH legacy `[bin]` singular AND `[[bin]]`
/// array-of-tables — see `parse` for the migration.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub package: Package,
    /// Zero, one, or many bins. Legacy `[bin]` singular parses to a
    /// single-entry Vec (with `name` inferred from `package.name` if
    /// omitted). Multi-bin `[[bin]]` requires `name` on each entry.
    pub bins: Vec<ManifestBin>,
    pub lib: Option<Lib>,
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

/// A `[[bin]]` entry (or the sole `[bin]` after auto-migration).
///
/// Phase 11.5.b introduced this shape. `name` is always populated
/// after parse — legacy `[bin]` singular omits it and the parser
/// fills in `package.name`; `[[bin]]` requires it explicitly.
///
/// `target` distinguishes what artefact the codegen emits:
/// - `Native` — default. Rust source via `codegen.rs` +
///   `cargo build --release` → native binary.
/// - `WasmClient` — SFC (`.fitzv`) or composing `.fitz` compiled
///   to a WASM bundle for the browser. Requires `mount`.
/// - `Ssr` — reserved for 11.6+. Parse accepts; build rejects.
///
/// See `docs/fase-11-plan.md` §9.p for the full decision.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestBin {
    /// Bin identifier used by `fitz build --bin <name>`. Unique per
    /// manifest in multi-bin projects. In legacy `[bin]` singular
    /// without an explicit `name`, filled from `package.name`.
    pub name: String,
    /// Path to the entry, relative to the manifest dir.
    pub main: String,
    /// Compilation target. `None` means "not set explicitly, treat
    /// as `Native`". `Target::Native` (explicit) is equivalent.
    pub target: Option<Target>,
    /// CSS selector where the WASM bundle mounts its root
    /// component. Required when `target = Target::WasmClient`;
    /// ignored otherwise. Convention: `"#app"`.
    pub mount: Option<String>,
}

impl ManifestBin {
    /// Effective target: `Native` when `target` is `None`.
    pub fn effective_target(&self) -> Target {
        self.target.unwrap_or(Target::Native)
    }
}

/// Compilation target of a `[[bin]]` entry. Introduced in Phase
/// 11.5.b. TOML serialisation is kebab-case:
///
/// - `Native` → `"native"` (default)
/// - `WasmClient` → `"wasm-client"`
/// - `Ssr` → `"ssr"` (reserved for 11.6+)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    #[default]
    Native,
    WasmClient,
    Ssr,
}

impl Target {
    /// Human-readable name matching the TOML rendering.
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Native => "native",
            Target::WasmClient => "wasm-client",
            Target::Ssr => "ssr",
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Non-fatal notice emitted by `Manifest::warnings()`. The manifest
/// parses successfully; the caller (typically the CLI at build
/// time) decides whether to show, log, or escalate.
#[derive(Debug, Clone, PartialEq)]
pub enum ManifestWarning {
    /// A `[[bin]]` entry declares `target = "ssr"`. Reserved in
    /// the vocabulary since Phase 11.5.a but the SSR emitter
    /// arrives in 11.6+. `fitz build` on this bin rejects with a
    /// targeted "coming in 11.6+" message; other commands ignore.
    SsrTargetReserved { bin_name: String },
}

impl fmt::Display for ManifestWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestWarning::SsrTargetReserved { bin_name } => write!(
                f,
                "bin `{bin_name}` declares `target = \"ssr\"` — reserved \
                 vocabulary in Phase 11.5. The SSR emitter arrives in \
                 Phase 11.6+; `fitz build --bin {bin_name}` will reject \
                 it with a targeted message until then."
            ),
        }
    }
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
    /// A `[[bin]]` entry has no `name`. Only legacy `[bin]` singular
    /// allows omitting `name` (it defaults to `package.name`);
    /// `[[bin]]` array-of-tables must set it explicitly.
    BinMissingName { index: usize },
    /// Two or more `[[bin]]` entries share the same `name`.
    BinDuplicateName { name: String },
    /// A bin's fields form an incoherent shape. `reason` is already
    /// formatted for the user (mentions the specific mismatch and
    /// suggests a fix — e.g. `mount` missing for `wasm-client`, or
    /// `.fitzv` entry with `target = "native"`).
    BinInvalidShape { name: String, reason: String },
    /// `fitz build --bin <name>` did not match any declared bin.
    /// Available names are provided for the message.
    BinNotFound {
        requested: String,
        available: Vec<String>,
    },
    /// The manifest declares more than one `[[bin]]` and the
    /// command did not select one. The list of available names
    /// is included so the message can list them explicitly.
    BinAmbiguous { available: Vec<String> },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::InvalidName(n) => write!(
                f,
                "invalid package name: `{n}`. Must match \
                 `^[a-z][a-z0-9_-]{{0,63}}$` (lowercase, must start with \
                 a letter, contain only letters/digits/`-`/`_`, max 64 \
                 characters)."
            ),
            ManifestError::Parse(e) => write!(f, "error parsing manifest: {e}"),
            ManifestError::Serialize(e) => {
                write!(f, "error serializing manifest: {e}")
            }
            ManifestError::DepNotImplemented { name, reason } => {
                write!(f, "dep `{name}`: {reason}")
            }
            ManifestError::DepPathNotFound { name, path } => {
                write!(f, "dep `{name}`: path `{}` does not exist.", path.display())
            }
            ManifestError::DepManifestInvalid { name, path, source } => write!(
                f,
                "dep `{name}`: invalid manifest at `{}` — {source}",
                path.display()
            ),
            ManifestError::DepMissingLib { name, path } => write!(
                f,
                "dep `{name}`: `{}` has no `[lib]` section. Path \
                 deps are libraries; add:\n\n[lib]\nentry = \"src/lib.fitz\"\n",
                path.display()
            ),
            ManifestError::DepInvalidShape { name } => write!(
                f,
                "dep `{name}`: must specify `path = \"...\"` or \
                 `git = \"...\"` with `tag`/`rev` (loose versions \
                 like `\"1.0.0\"` arrive with the registry in 9.y.5)."
            ),
            ManifestError::DepInvalidGitShape { name, reason } => {
                write!(f, "dep `{name}` (git): {reason}")
            }
            ManifestError::DepGitError { name, source } => {
                write!(f, "dep `{name}` (git): {source}")
            }
            ManifestError::EditParse(e) => write!(f, "error parseando manifest: {e}"),
            ManifestError::BinMissingName { index } => write!(
                f,
                "the `[[bin]]` entry at index {index} has no `name`. \
                 Multi-bin projects require a unique `name` per entry \
                 (only the legacy `[bin]` singular can omit it and \
                 default to `package.name`)."
            ),
            ManifestError::BinDuplicateName { name } => write!(
                f,
                "two `[[bin]]` entries share the name `{name}`. Every \
                 bin must have a unique `name` — `fitz build --bin \
                 <name>` selects by it."
            ),
            ManifestError::BinInvalidShape { name, reason } => {
                write!(f, "bin `{name}`: {reason}")
            }
            ManifestError::BinNotFound {
                requested,
                available,
            } => {
                let list = if available.is_empty() {
                    "(the manifest declares no bins)".to_string()
                } else {
                    available
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                write!(
                    f,
                    "no bin named `{requested}` in the manifest. \
                     Available: {list}."
                )
            }
            ManifestError::BinAmbiguous { available } => {
                let list = available
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "this project declares more than one `[[bin]]` \
                     ({list}). Pass `--bin <name>` to pick one."
                )
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    /// Builds a default manifest for a new project:
    /// - version `0.1.0`
    /// - current edition
    /// - bin with entry `src/main.fitz` (name = `package.name`)
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
            bins: vec![ManifestBin {
                name: name.to_string(),
                main: "src/main.fitz".to_string(),
                target: None,
                mount: None,
            }],
            lib: None,
            dependencies: BTreeMap::new(),
            flags: BTreeMap::new(),
        })
    }

    /// Parses a manifest from TOML text.
    ///
    /// Phase 11.5.b: accepts BOTH legacy `[bin]` singular AND
    /// `[[bin]]` array-of-tables. Legacy singular fills `name` from
    /// `package.name` when omitted; array requires it explicitly.
    /// Runs cross-field validation on each bin (rejects
    /// `.fitzv` + `target = "native"`, `wasm-client` without
    /// `mount`, etc.). Accepts `target = "ssr"` at parse; the
    /// warning surfaces via `warnings()` for the caller.
    pub fn parse(input: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest = toml::from_str(input).map_err(ManifestError::Parse)?;
        let bins = normalize_bins(raw.bin, &raw.package.name)?;
        Ok(Manifest {
            package: raw.package,
            bins,
            lib: raw.lib,
            dependencies: raw.dependencies,
            flags: raw.flags,
        })
    }

    /// Serializes the manifest to TOML.
    ///
    /// Preserves legacy `[bin]` singular output for the common case
    /// (one bin, `name = package.name`, no target override, no
    /// mount) — that keeps `fitz new` / `fitz init` scaffolds
    /// visually identical to pre-11.5.b. All other shapes emit as
    /// `[[bin]]` array-of-tables.
    pub fn to_toml_string(&self) -> Result<String, ManifestError> {
        let bin_wire: Option<BinFieldOut<'_>> = if self.bins.is_empty() {
            None
        } else if self.bins.len() == 1
            && self.bins[0].name == self.package.name
            && self.bins[0].target.is_none()
            && self.bins[0].mount.is_none()
        {
            Some(BinFieldOut::Legacy(LegacyBinOut {
                main: &self.bins[0].main,
            }))
        } else {
            Some(BinFieldOut::Multi(&self.bins))
        };

        let wire = ManifestWire {
            package: &self.package,
            bin: bin_wire,
            lib: self.lib.as_ref(),
            dependencies: &self.dependencies,
            flags: &self.flags,
        };
        toml::to_string(&wire).map_err(ManifestError::Serialize)
    }

    /// Non-fatal warnings for this manifest. Consumed by the CLI at
    /// build time (see `fitz build` in `src/main.rs`). Empty when
    /// the manifest is fully in-scope for the current MVP.
    pub fn warnings(&self) -> Vec<ManifestWarning> {
        let mut out = Vec::new();
        for b in &self.bins {
            if b.target == Some(Target::Ssr) {
                out.push(ManifestWarning::SsrTargetReserved {
                    bin_name: b.name.clone(),
                });
            }
        }
        out
    }

    /// Finds a bin by name. Returns `None` if no bin has that name.
    pub fn bin_by_name(&self, name: &str) -> Option<&ManifestBin> {
        self.bins.iter().find(|b| b.name == name)
    }

    /// Selects a bin according to a `--bin <name>` selector. If
    /// `selector` is `Some`, must match a declared bin. If `None`,
    /// picks the sole bin when there is exactly one; ambiguous when
    /// there are more; `Ok(None)` when the manifest declares no
    /// bins (caller decides whether that is an error).
    pub fn select_bin(
        &self,
        selector: Option<&str>,
    ) -> Result<Option<&ManifestBin>, ManifestError> {
        match (selector, self.bins.len()) {
            (Some(name), _) => {
                self.bin_by_name(name)
                    .map(Some)
                    .ok_or_else(|| ManifestError::BinNotFound {
                        requested: name.to_string(),
                        available: self.bins.iter().map(|b| b.name.clone()).collect(),
                    })
            }
            (None, 0) => Ok(None),
            (None, 1) => Ok(Some(&self.bins[0])),
            (None, _) => Err(ManifestError::BinAmbiguous {
                available: self.bins.iter().map(|b| b.name.clone()).collect(),
            }),
        }
    }
}

// ---- Phase 11.5.b — internal wire structs for parse + serialize ----

/// Wire representation used by `Manifest::parse`. Deserialised by
/// serde-toml; the caller (`Manifest::parse`) normalises `bin` into
/// the public `Vec<ManifestBin>` shape.
#[derive(Deserialize)]
struct RawManifest {
    package: Package,
    #[serde(default)]
    bin: Option<RawBinField>,
    #[serde(default)]
    lib: Option<Lib>,
    #[serde(default)]
    dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    flags: BTreeMap<String, bool>,
}

/// Discriminates the two TOML shapes for the `bin` field:
/// - Legacy `[bin] main = "..."` → `Single(...)`
/// - `[[bin]] main = "..."` (array-of-tables) → `Multiple(...)`
#[derive(Deserialize)]
#[serde(untagged)]
enum RawBinField {
    Single(RawManifestBin),
    Multiple(Vec<RawManifestBin>),
}

/// Raw shape of a bin entry as it lives in TOML before normalisation.
/// Deserialised with all fields optional so that legacy `[bin]`
/// without `name` still parses; the normalisation step (see
/// `normalize_bins`) fills the default `name` and validates.
#[derive(Deserialize)]
struct RawManifestBin {
    #[serde(default)]
    name: Option<String>,
    main: String,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    mount: Option<String>,
}

fn normalize_bins(
    field: Option<RawBinField>,
    package_name: &str,
) -> Result<Vec<ManifestBin>, ManifestError> {
    match field {
        None => Ok(Vec::new()),
        Some(RawBinField::Single(raw)) => {
            // Legacy [bin]: name defaults to package.name.
            let bin = ManifestBin {
                name: raw.name.unwrap_or_else(|| package_name.to_string()),
                main: raw.main,
                target: raw.target,
                mount: raw.mount,
            };
            validate_bin_cross_fields(&bin)?;
            Ok(vec![bin])
        }
        Some(RawBinField::Multiple(raws)) => {
            let mut bins: Vec<ManifestBin> = Vec::with_capacity(raws.len());
            for (i, raw) in raws.into_iter().enumerate() {
                let name = raw.name.ok_or(ManifestError::BinMissingName { index: i })?;
                let bin = ManifestBin {
                    name,
                    main: raw.main,
                    target: raw.target,
                    mount: raw.mount,
                };
                validate_bin_cross_fields(&bin)?;
                bins.push(bin);
            }
            // Unique names.
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for b in &bins {
                if !seen.insert(b.name.as_str()) {
                    return Err(ManifestError::BinDuplicateName {
                        name: b.name.clone(),
                    });
                }
            }
            Ok(bins)
        }
    }
}

/// Enforces cross-field invariants on a single bin:
/// - `.fitzv` entry rejects `target = Native` (both explicit and
///   the default) — the classic codegen cannot process view files.
/// - `target = WasmClient` requires `mount` (the WASM bundle needs
///   a DOM selector to mount its root component).
/// - `target = Ssr` parses OK (reserved for 11.6+); no error here,
///   the caller pulls `Manifest::warnings()` if it cares.
fn validate_bin_cross_fields(bin: &ManifestBin) -> Result<(), ManifestError> {
    let main_lower = bin.main.to_lowercase();
    let is_view = main_lower.ends_with(".fitzv");
    let effective = bin.effective_target();

    match effective {
        Target::Native => {
            if is_view {
                return Err(ManifestError::BinInvalidShape {
                    name: bin.name.clone(),
                    reason: format!(
                        "`main = \"{}\"` is a `.fitzv` (view file) but \
                         `target = \"native\"`. Native binaries compile \
                         classic `.fitz` sources. Set `target = \
                         \"wasm-client\"` and `mount = \"#app\"` to \
                         emit a browser WASM bundle instead.",
                        bin.main
                    ),
                });
            }
        }
        Target::WasmClient => {
            if bin.mount.is_none() {
                return Err(ManifestError::BinInvalidShape {
                    name: bin.name.clone(),
                    reason: "`target = \"wasm-client\"` requires \
                             `mount = \"<selector>\"` (typically \
                             `mount = \"#app\"`) to know where to \
                             mount the root component in the DOM."
                        .to_string(),
                });
            }
        }
        Target::Ssr => {
            // Reserved vocabulary. Warning is surfaced via
            // `Manifest::warnings()`; parse succeeds.
        }
    }

    Ok(())
}

/// Wire representation used by `Manifest::to_toml_string`. Mirrors
/// the shape of the manifest but with the flexible `bin` field
/// (legacy singular vs multi array).
#[derive(Serialize)]
struct ManifestWire<'a> {
    package: &'a Package,
    #[serde(skip_serializing_if = "Option::is_none")]
    bin: Option<BinFieldOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lib: Option<&'a Lib>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    dependencies: &'a BTreeMap<String, Dependency>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    flags: &'a BTreeMap<String, bool>,
}

/// Two-shaped serialisation of the `bin` field. `untagged` picks by
/// the variant we hand it. `Legacy` renders as `[bin] main = "..."`;
/// `Multi` renders as `[[bin]] ...` array-of-tables.
#[derive(Serialize)]
#[serde(untagged)]
enum BinFieldOut<'a> {
    Legacy(LegacyBinOut<'a>),
    Multi(&'a Vec<ManifestBin>),
}

#[derive(Serialize)]
struct LegacyBinOut<'a> {
    main: &'a str,
}

impl Serialize for ManifestBin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        // Field count: name + main + (target?) + (mount?).
        let mut extra = 0;
        if self.target.is_some() {
            extra += 1;
        }
        if self.mount.is_some() {
            extra += 1;
        }
        let mut s = serializer.serialize_struct("ManifestBin", 2 + extra)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("main", &self.main)?;
        if let Some(t) = &self.target {
            s.serialize_field("target", t)?;
        }
        if let Some(m) = &self.mount {
            s.serialize_field("mount", m)?;
        }
        s.end()
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
        .ok_or_else(|| ManifestError::InvalidName("[dependencies] is not a table".to_string()))?;

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
            reason: "loose-version deps (`foo = \"1.0.0\"`) require \
                     the registry, which arrives in 9.y.5. For now use \
                     `foo = { path = \"...\" }` or `foo = { git = \"...\", tag = \"...\" }`."
                .to_string(),
        }),
        Dependency::Detailed(d) => {
            let has_path = d.path.is_some();
            let has_git = d.git.is_some();

            // path + git: invalid combination (which priority?).
            if has_path && has_git {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "cannot combine `path` with `git` in the same dep. \
                             Use one or the other."
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
                    reason: "`tag`/`rev` also require `git = \"<url>\"`.".to_string(),
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
            reason: "`tag` and `rev` are mutually exclusive — pick one.".to_string(),
        }),
        (Some(t), None) => {
            if t.trim().is_empty() {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "`tag` cannot be empty.".to_string(),
                });
            }
            Ok(crate::git_dep::GitRef::Tag(t.to_string()))
        }
        (None, Some(r)) => {
            if r.trim().is_empty() {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "`rev` cannot be empty.".to_string(),
                });
            }
            Ok(crate::git_dep::GitRef::Rev(r.to_string()))
        }
        (None, None) => Err(ManifestError::DepInvalidGitShape {
            name: name.to_string(),
            reason: "git deps require `tag = \"...\"` or `rev = \"...\"` for reproducibility. \
                     `branch` is intentionally unsupported (mutable → non-reproducible builds)."
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
    fn valid_names_pass() {
        assert!(is_valid_package_name("fitz-uuid"));
        assert!(is_valid_package_name("a"));
        assert!(is_valid_package_name("mi-app"));
        assert!(is_valid_package_name("my_lib"));
        assert!(is_valid_package_name("http2"));
        assert!(is_valid_package_name("a-b-c-d-e"));
    }

    #[test]
    fn invalid_names_fail() {
        assert!(!is_valid_package_name(""), "empty");
        assert!(!is_valid_package_name("Foo"), "leading uppercase");
        assert!(!is_valid_package_name("FOO"), "all uppercase");
        assert!(!is_valid_package_name("1foo"), "starts with digit");
        assert!(!is_valid_package_name("-foo"), "starts with hyphen");
        assert!(!is_valid_package_name("_foo"), "starts with underscore");
        assert!(!is_valid_package_name("foo bar"), "space");
        assert!(!is_valid_package_name("foo.bar"), "dot");
        assert!(!is_valid_package_name("foo/bar"), "slash");
        assert!(!is_valid_package_name("foo@bar"), "at sign");
        assert!(
            !is_valid_package_name(&"a".repeat(65)),
            "more than 64 chars"
        );
    }

    #[test]
    fn name_with_64_characters_is_valid() {
        // Cota inclusiva: 64 OK, 65 no (test arriba).
        assert!(is_valid_package_name(&"a".repeat(64)));
    }

    #[test]
    fn new_default_emits_consistent_manifest() {
        let m = Manifest::new_default("mi-app").unwrap();
        assert_eq!(m.package.name, "mi-app");
        assert_eq!(m.package.version, "0.1.0");
        assert_eq!(m.package.edition, CURRENT_EDITION);
        assert!(m.package.authors.is_empty());
        assert_eq!(m.bins.len(), 1);
        assert_eq!(m.bins[0].name, "mi-app");
        assert_eq!(m.bins[0].main, "src/main.fitz");
        assert!(m.bins[0].target.is_none());
        assert!(m.bins[0].mount.is_none());
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn new_default_rejects_invalid_name() {
        let err = Manifest::new_default("Foo").unwrap_err();
        match err {
            ManifestError::InvalidName(n) => assert_eq!(n, "Foo"),
            other => panic!("expected InvalidName, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_preserves_basic_fields() {
        let original = Manifest::new_default("test-app").unwrap();
        let toml = original.to_toml_string().unwrap();
        let parsed = Manifest::parse(&toml).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serialization_omits_empty_optional_fields() {
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
    fn parse_accepts_minimal_manifest() {
        let toml_text = r#"
[package]
name = "mi-app"
version = "0.1.0"
edition = "2026"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.package.name, "mi-app");
        assert!(m.bins.is_empty());
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn parse_accepts_complete_manifest() {
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
        assert_eq!(m.bins.len(), 1);
        // Legacy `[bin]` singular: name is filled from package.name.
        assert_eq!(m.bins[0].name, "mi-app");
        assert_eq!(m.bins[0].main, "src/main.fitz");
        assert!(m.bins[0].target.is_none());
        assert!(m.bins[0].mount.is_none());
        assert_eq!(m.dependencies.len(), 2);
        match m.dependencies.get("fitz-uuid").unwrap() {
            Dependency::Version(v) => assert_eq!(v, "1.0.0"),
            other => panic!("expected Version, got {other:?}"),
        }
    }

    // ---- Phase 11.5.b — multi-bin, targets, mount validation ----

    #[test]
    fn parse_legacy_bin_without_name_defaults_to_package_name() {
        let toml_text = r#"
[package]
name = "server"
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.bins.len(), 1);
        assert_eq!(m.bins[0].name, "server");
        assert_eq!(m.bins[0].main, "src/main.fitz");
        assert!(m.bins[0].target.is_none());
    }

    #[test]
    fn parse_legacy_bin_with_explicit_name_keeps_it() {
        let toml_text = r#"
[package]
name = "server"
version = "0.1.0"
edition = "2026"

[bin]
name = "custom"
main = "src/main.fitz"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.bins.len(), 1);
        assert_eq!(m.bins[0].name, "custom");
    }

    #[test]
    fn parse_multi_bin_array_of_tables() {
        let toml_text = r##"
[package]
name = "full-stack"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "server"
main = "src/main.fitz"

[[bin]]
name = "web"
main = "src/counter.fitzv"
target = "wasm-client"
mount = "#app"
"##;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.bins.len(), 2);
        assert_eq!(m.bins[0].name, "server");
        assert_eq!(m.bins[0].main, "src/main.fitz");
        assert!(m.bins[0].target.is_none());
        assert_eq!(m.bins[0].effective_target(), Target::Native);
        assert_eq!(m.bins[1].name, "web");
        assert_eq!(m.bins[1].main, "src/counter.fitzv");
        assert_eq!(m.bins[1].target, Some(Target::WasmClient));
        assert_eq!(m.bins[1].mount.as_deref(), Some("#app"));
    }

    #[test]
    fn parse_multi_bin_without_name_errors_with_index() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
main = "src/a.fitz"

[[bin]]
main = "src/b.fitz"
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        match err {
            ManifestError::BinMissingName { index } => assert_eq!(index, 0),
            other => panic!("expected BinMissingName, got {other:?}"),
        }
    }

    #[test]
    fn parse_multi_bin_duplicate_names_errors() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "server"
main = "src/a.fitz"

[[bin]]
name = "server"
main = "src/b.fitz"
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        match err {
            ManifestError::BinDuplicateName { name } => assert_eq!(name, "server"),
            other => panic!("expected BinDuplicateName, got {other:?}"),
        }
    }

    #[test]
    fn parse_fitzv_entry_with_native_target_errors() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "web"
main = "src/counter.fitzv"
target = "native"
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("web"), "msg: {msg}");
        assert!(msg.contains(".fitzv"), "msg: {msg}");
        assert!(msg.contains("wasm-client"), "msg: {msg}");
    }

    #[test]
    fn parse_fitzv_entry_with_default_target_errors_too() {
        // The default target is Native; omitting it must trigger the
        // same rejection as writing `target = "native"` explicitly.
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "web"
main = "src/counter.fitzv"
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        assert!(matches!(err, ManifestError::BinInvalidShape { .. }));
    }

    #[test]
    fn parse_wasm_client_without_mount_errors() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "web"
main = "src/counter.fitzv"
target = "wasm-client"
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("web"), "msg: {msg}");
        assert!(msg.contains("mount"), "msg: {msg}");
        assert!(msg.contains("#app"), "msg: {msg}");
    }

    #[test]
    fn parse_wasm_client_with_classic_fitz_and_mount_ok() {
        // A `.fitz` file with `target = "wasm-client"` is legal — it
        // is the composition case (11.5.d will wire multi-component
        // roots). Parse should accept.
        let toml_text = r##"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "web"
main = "src/app.fitz"
target = "wasm-client"
mount = "#root"
"##;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.bins.len(), 1);
        assert_eq!(m.bins[0].target, Some(Target::WasmClient));
        assert_eq!(m.bins[0].mount.as_deref(), Some("#root"));
    }

    #[test]
    fn parse_ssr_target_accepted_and_surfaces_warning() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "srv"
main = "src/main.fitz"
target = "ssr"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert_eq!(m.bins[0].target, Some(Target::Ssr));
        // Warnings surface Ssr as reserved for 11.6+.
        let warnings = m.warnings();
        assert_eq!(warnings.len(), 1);
        match &warnings[0] {
            ManifestWarning::SsrTargetReserved { bin_name } => assert_eq!(bin_name, "srv"),
        }
    }

    #[test]
    fn target_enum_serialises_kebab_case() {
        // Round-trip a multi-bin manifest through parse+emit and
        // verify the target strings are canonical.
        let toml_text = r##"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "srv"
main = "src/main.fitz"

[[bin]]
name = "web"
main = "src/counter.fitzv"
target = "wasm-client"
mount = "#app"
"##;
        let m = Manifest::parse(toml_text).unwrap();
        let emitted = m.to_toml_string().unwrap();
        assert!(
            emitted.contains("target = \"wasm-client\""),
            "emitted:\n{emitted}"
        );
        // Roundtrip through parse must give equal manifests.
        let m2 = Manifest::parse(&emitted).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn select_bin_none_ambiguous_when_multi_bin() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "a"
main = "src/a.fitz"

[[bin]]
name = "b"
main = "src/b.fitz"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        let err = m.select_bin(None).unwrap_err();
        match err {
            ManifestError::BinAmbiguous { available } => {
                assert_eq!(available, vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("expected BinAmbiguous, got {other:?}"),
        }
    }

    #[test]
    fn select_bin_by_name_picks_the_right_one() {
        let toml_text = r##"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "server"
main = "src/main.fitz"

[[bin]]
name = "web"
main = "src/counter.fitzv"
target = "wasm-client"
mount = "#app"
"##;
        let m = Manifest::parse(toml_text).unwrap();
        let picked = m.select_bin(Some("web")).unwrap().unwrap();
        assert_eq!(picked.name, "web");
        assert_eq!(picked.effective_target(), Target::WasmClient);
    }

    #[test]
    fn select_bin_missing_name_errors_listing_available() {
        let toml_text = r##"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "server"
main = "src/main.fitz"

[[bin]]
name = "web"
main = "src/w.fitz"
target = "wasm-client"
mount = "#app"
"##;
        let m = Manifest::parse(toml_text).unwrap();
        let err = m.select_bin(Some("nope")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope"), "msg: {msg}");
        assert!(msg.contains("server"), "msg: {msg}");
        assert!(msg.contains("web"), "msg: {msg}");
    }

    #[test]
    fn select_bin_single_no_selector_picks_the_only_one() {
        let m = Manifest::new_default("mi-app").unwrap();
        let picked = m.select_bin(None).unwrap().unwrap();
        assert_eq!(picked.name, "mi-app");
    }

    #[test]
    fn select_bin_empty_manifest_returns_ok_none() {
        let toml_text = r#"
[package]
name = "lib-only"
version = "0.1.0"
edition = "2026"

[lib]
entry = "src/lib.fitz"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        assert!(m.select_bin(None).unwrap().is_none());
    }

    #[test]
    fn serialise_single_legacy_shape_stays_visually_legacy() {
        // A single bin with default name and no target/mount emits
        // `[bin]` singular (not `[[bin]]`) — preserves the look of
        // pre-11.5.b scaffolded manifests.
        let m = Manifest::new_default("mi-app").unwrap();
        let emitted = m.to_toml_string().unwrap();
        assert!(
            emitted.contains("[bin]\n") && emitted.contains("main = \"src/main.fitz\""),
            "expected legacy [bin] shape, got:\n{emitted}"
        );
        assert!(
            !emitted.contains("[[bin]]"),
            "should NOT be array-of-tables:\n{emitted}"
        );
    }

    #[test]
    fn serialise_multi_bin_emits_array_of_tables() {
        let toml_text = r#"
[package]
name = "x"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "a"
main = "src/a.fitz"

[[bin]]
name = "b"
main = "src/b.fitz"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        let emitted = m.to_toml_string().unwrap();
        assert!(
            emitted.contains("[[bin]]"),
            "expected array-of-tables:\n{emitted}"
        );
    }

    #[test]
    fn serialise_single_bin_with_custom_name_emits_array_of_tables() {
        // Even one bin, if it has a name != package.name, emits as
        // `[[bin]]` so the name is preserved verbatim on re-parse.
        let toml_text = r#"
[package]
name = "srv"
version = "0.1.0"
edition = "2026"

[bin]
name = "custom"
main = "src/main.fitz"
"#;
        let m = Manifest::parse(toml_text).unwrap();
        let emitted = m.to_toml_string().unwrap();
        assert!(
            emitted.contains("[[bin]]") && emitted.contains("name = \"custom\""),
            "expected [[bin]] preserving custom name:\n{emitted}"
        );
    }

    // ---- Phase 12.8 — [flags] section ----

    #[test]
    fn flags_section_empty_or_absent_defaults_to_empty_map() {
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
    fn flags_section_parses_bool_pairs() {
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
    fn flags_non_bool_value_is_parse_error() {
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
            "expected ManifestError::Parse, was {err:?}"
        );
    }

    #[test]
    fn parse_fails_with_missing_required_field() {
        let toml_text = r#"
[package]
name = "mi-app"
# falta version y edition
"#;
        let err = Manifest::parse(toml_text).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn find_manifest_finds_in_current_dir() {
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
    fn find_manifest_walks_upward() {
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
    fn parse_dependency_short_version() {
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
            other => panic!("expected Version, got {other:?}"),
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
            other => panic!("expected Detailed, got {other:?}"),
        }
    }

    #[test]
    fn parse_dependency_git_reserved_is_accepted_at_parse_level() {
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
            other => panic!("expected Detailed, got {other:?}"),
        }
    }

    #[test]
    fn parse_lib_section() {
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
        assert!(m.bins.is_empty());
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
        std::fs::write(dir.join("src/lib.fitz"), "// empty lib\n").unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn resolve_dependencies_path_dep_returns_resolved_dep() {
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
            other => panic!("expected ResolvedDepSource::Path, got {other:?}"),
        }
    }

    #[test]
    fn resolve_dependencies_short_version_aborts_citing_9y5() {
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
    fn add_dep_to_manifest_without_dependencies_creates_the_section() {
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
    fn add_dep_path_emits_inline_table() {
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
    fn add_dep_git_with_tag_emits_inline_table() {
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
    fn add_dep_git_with_rev_emits_inline_table() {
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
    fn add_dep_overwrites_if_already_existed() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nfoo = { path = \"../viejo\" }\n";
        let spec = AddDepSpec::Path {
            path: "../nuevo".to_string(),
        };
        let updated = add_dep_to_manifest(original, "foo", &spec).unwrap();
        assert!(updated.contains("foo = { path = \"../nuevo\" }"));
        assert!(
            !updated.contains("../viejo"),
            "the old path should not have persisted:\n{updated}"
        );
    }

    #[test]
    fn add_dep_preserves_user_comments() {
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
    fn remove_dep_removes_entry_and_reports_true() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nuno = { path = \"../uno\" }\ndos = { path = \"../dos\" }\n";
        let (updated, removed) = remove_dep_from_manifest(original, "uno").unwrap();
        assert!(removed);
        assert!(!updated.contains("uno = "));
        assert!(updated.contains("dos = { path = \"../dos\" }"));
    }

    #[test]
    fn remove_dep_reports_false_if_did_not_exist() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nuno = { path = \"../uno\" }\n";
        let (updated, removed) = remove_dep_from_manifest(original, "no-existe").unwrap();
        assert!(!removed);
        // The manifest stays intact.
        assert!(updated.contains("uno = { path = \"../uno\" }"));
    }

    #[test]
    fn remove_dep_deletes_section_if_left_empty() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nuno = { path = \"../uno\" }\n";
        let (updated, removed) = remove_dep_from_manifest(original, "uno").unwrap();
        assert!(removed);
        assert!(
            !updated.contains("[dependencies]"),
            "[dependencies] should have been removed when left empty:\n{updated}"
        );
    }

    #[test]
    fn remove_dep_without_dependencies_section_is_no_op() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let (updated, removed) = remove_dep_from_manifest(original, "foo").unwrap();
        assert!(!removed);
        assert_eq!(updated, original);
    }

    #[test]
    fn add_and_remove_are_approximately_inverse() {
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
    fn resolve_dependencies_nonexistent_path_aborts() {
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
    fn resolve_dependencies_path_without_lib_aborts() {
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
    fn resolve_git_dep_without_tag_or_rev_aborts_requesting_one() {
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
            msg.contains("reproducibility"),
            "msg should cite reproducibility: {msg}"
        );
    }

    #[test]
    fn resolve_git_dep_with_tag_and_rev_together_aborts() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "helpers".to_string(),
            git_dep(None, Some("https://example.com/r"), Some("v1"), Some("abc")),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mutually exclusive"), "msg: {msg}");
    }

    #[test]
    fn resolve_git_dep_empty_tag_aborts() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies.insert(
            "helpers".to_string(),
            git_dep(None, Some("https://example.com/r"), Some("  "), None),
        );
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`tag` cannot be empty"), "msg: {msg}");
    }

    #[test]
    fn resolve_path_and_git_together_aborts_invalid_combination() {
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
        assert!(msg.contains("cannot combine"), "msg: {msg}");
    }

    #[test]
    fn resolve_tag_without_git_aborts_requesting_url() {
        let importer_dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::new_default("importer").unwrap();
        m.dependencies
            .insert("x".to_string(), git_dep(None, None, Some("v1"), None));
        let err = resolve_dependencies(&m, importer_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("also require `git"), "msg: {msg}");
    }

    #[test]
    fn parse_git_ref_returns_correct_tag_or_rev() {
        let t = parse_git_ref("x", Some("v1.0.0"), None).unwrap();
        assert_eq!(t, crate::git_dep::GitRef::Tag("v1.0.0".to_string()));

        let r = parse_git_ref("x", None, Some("abc123")).unwrap();
        assert_eq!(r, crate::git_dep::GitRef::Rev("abc123".to_string()));
    }

    #[test]
    fn resolve_dependencies_detailed_empty_aborts_invalid_shape() {
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
    fn find_manifest_returns_none_if_not_present() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        // We do not create fitz.toml. Walks up to the FS root without
        // finding it (assuming there is no fitz.toml at the system
        // root, which is reasonable).
        assert!(find_manifest(&nested).is_none());
    }
}
