//! Manifest del proyecto (`fitz.toml`) — Fase 9.y.1.
//!
//! Primer pieza del package manager. Define el formato del archivo que
//! describe un proyecto Fitz (nombre, versión, entry point, deps) y la
//! API mínima para leerlo, escribirlo, y resolverlo desde un directorio
//! arbitrario.
//!
//! En 9.y.1 el manifest todavía no afecta a `fitz run`/`build`/`check`
//! — esos consumidores llegan en 9.y.2. Acá solo entregamos el formato
//! + `fitz new`/`fitz init` que lo crean.
//!
//! Convenciones cerradas en 9.y.1 (ver `docs/roadmap.md` → 9.y):
//! - Formato: TOML (`fitz.toml`).
//! - Estructura: `src/main.fitz` como entry point por default.
//! - Field versionado: `edition = "2026"` (Cargo-style year).
//! - Bin único en MVP (`[bin] main = "..."`). Multi-bin con `[[bin]]`
//!   queda como deuda 9.y.8+.
//! - Validación de nombre: `^[a-z][a-z0-9_-]{0,63}$` (política
//!   crates.io: lowercase + alfanumérico + `-`/`_`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Edición del lenguaje vigente para `fitz new`. Cargo-style year;
/// permite romper compatibilidad sin tocar la versión semántica del
/// compilador.
pub const CURRENT_EDITION: &str = "2026";

/// Nombre del archivo de manifest.
pub const MANIFEST_FILE: &str = "fitz.toml";

/// Manifest completo de un proyecto Fitz.
///
/// **Sobre `[bin]` vs `[lib]`** (Fase 9.y.3.a): un proyecto puede ser
/// ejecutable (`[bin]`), librería (`[lib]`), o ambos. `fitz run`/
/// `build` exigen `[bin]`. Los path deps de OTROS proyectos exigen
/// `[lib]` (un proyecto solo-bin no se puede importar desde otro).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Bin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lib: Option<Lib>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,
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

/// Sección `[lib]` del manifest. Marca al proyecto como librería
/// importable desde otros proyectos vía path/git deps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lib {
    /// Path relativo al manifest del entry de la librería. Por
    /// convención `src/lib.fitz`, paralelo a `[bin].main`.
    pub entry: String,
}

/// Declaración de una dependencia en `[dependencies]`. Acepta dos
/// formas TOML gracias a `serde(untagged)`:
///
/// - **Versión plana** `foo = "1.2.3"` → `Dependency::Version("1.2.3")`.
///   Reservada para 9.y.5 (registry). Al resolver hoy → error claro.
/// - **Detallada** `foo = { path = "../foo" }` → `Dependency::Detailed`.
///   Soporta `path` (9.y.3.a) y los campos `git`/`tag`/`rev` que
///   reservamos para 9.y.3.c (rechazo controlado al resolver).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Version(String),
    Detailed(DetailedDependency),
}

/// Forma detallada de una dependencia. Todos los campos son opcionales
/// para permitir el roadmap incremental:
///
/// - 9.y.3.a usa `path`.
/// - 9.y.3.c agrega `git` + `tag`/`rev`.
/// - 9.y.4 puede sumar `version` para combinar con git/registry.
///
/// La validación cruzada (no permitir `path` + `git`, etc.) ocurre al
/// resolver, no al parsear — el manifest acepta cualquier combinación
/// y el resolver emite errores específicos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetailedDependency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Reservado para 9.y.3.c (git deps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Reservado para 9.y.3.c — tag a checkear en el git dep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Reservado para 9.y.3.c — commit sha a checkear en el git dep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

/// Errores del manifest. Sin integrar con `FitzError` (que es del
/// pipeline del lenguaje) — esto es tooling de proyecto.
#[derive(Debug)]
pub enum ManifestError {
    /// El nombre no matchea `^[a-z][a-z0-9_-]{0,63}$`.
    InvalidName(String),
    /// Fallo al parsear el TOML.
    Parse(toml::de::Error),
    /// Fallo al serializar a TOML.
    Serialize(toml::ser::Error),
    /// Dep que requiere algo todavía no implementado (versión sin
    /// registry, git, etc.). Lleva el nombre de la dep y el sub-paso
    /// futuro que la habilita, para que el mensaje sea accionable.
    DepNotImplemented { name: String, reason: String },
    /// Dep `{path = "..."}` cuyo path no existe en disco.
    DepPathNotFound { name: String, path: PathBuf },
    /// Dep `{path = "..."}` cuyo manifest existe pero no parsea.
    DepManifestInvalid {
        name: String,
        path: PathBuf,
        source: Box<ManifestError>,
    },
    /// Dep `{path = "..."}` cuyo manifest no tiene sección `[lib]`.
    /// Las path deps son librerías por definición — si solo tiene
    /// `[bin]`, no se puede importar.
    DepMissingLib { name: String, path: PathBuf },
    /// Dep con forma inválida: ni `path`, ni `git`, ni nada
    /// resoluble.
    DepInvalidShape { name: String },
    /// Dep `git` con shape inválido: combinación prohibida o falta
    /// de tag/rev. El `reason` ya viene formateado para el usuario.
    DepInvalidGitShape { name: String, reason: String },
    /// Falló el clone/checkout/rev-parse de una git dep. Wrappea el
    /// error del módulo `git_dep` con el nombre de la dep para que
    /// el mensaje sea accionable.
    DepGitError {
        name: String,
        source: crate::git_dep::GitDepError,
    },
    /// Error parseando el manifest con `toml_edit` (camino de edición
    /// preservando formato — `fitz add`/`remove`). Separado de
    /// `Parse` porque ese viene del serde-toml flow.
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
    /// Construye un manifest default para un proyecto nuevo:
    /// - version `0.1.0`
    /// - edition vigente
    /// - bin con entry `src/main.fitz`
    /// - sin deps
    ///
    /// Valida el nombre.
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
        })
    }

    /// Parsea un manifest desde texto TOML.
    pub fn parse(input: &str) -> Result<Self, ManifestError> {
        toml::from_str(input).map_err(ManifestError::Parse)
    }

    /// Serializa el manifest a TOML.
    pub fn to_toml_string(&self) -> Result<String, ManifestError> {
        toml::to_string(self).map_err(ManifestError::Serialize)
    }
}

/// Valida un nombre de paquete contra la política crates.io-style:
/// `^[a-z][a-z0-9_-]{0,63}$`. Implementación a mano para evitar dep
/// de `regex` (es chica; no justifica el peso ahora).
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

/// Busca el `fitz.toml` más cercano subiendo desde `start`. Cargo-style:
/// el manifest puede estar en el directorio actual o en un ancestro.
/// Devuelve la ruta absoluta al archivo si lo encuentra.
///
/// Consumido por `resolve_entry` en `main.rs` desde Fase 9.y.2 (cuando
/// `fitz run`/`build`/`check` aceptan ser invocados sin archivo).
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

// ---- Fase 9.y.4 — edición del manifest preservando formato ----

/// Especificación de una dep para `fitz add`. Aliasing del shape que
/// el CLI parsea desde flags (`--path`, `--git`, `--tag`, `--rev`).
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

/// Agrega una dep al `fitz.toml` (texto). Preserva comentarios y
/// formatting del usuario gracias a `toml_edit`. Si ya existía una
/// entry con el mismo nombre, la sobreescribe (cargo-style). Si no
/// existía `[dependencies]`, la crea.
///
/// Devuelve el texto TOML actualizado. No persiste a disco — eso es
/// responsabilidad del caller (típicamente `main.rs::add_dep`).
pub fn add_dep_to_manifest(
    existing_text: &str,
    name: &str,
    spec: &AddDepSpec,
) -> Result<String, ManifestError> {
    let mut doc: toml_edit::DocumentMut =
        existing_text.parse().map_err(ManifestError::EditParse)?;

    // Asegurar [dependencies] como table existing.
    if !doc.contains_key("dependencies") {
        doc["dependencies"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let deps = doc["dependencies"]
        .as_table_mut()
        .ok_or_else(|| ManifestError::InvalidName("[dependencies] no es una tabla".to_string()))?;

    // Construir la inline table para la nueva dep.
    let inline = build_inline_dep_table(spec);
    deps.insert(
        name,
        toml_edit::Item::Value(toml_edit::Value::InlineTable(inline)),
    );

    Ok(doc.to_string())
}

/// Quita una dep del `fitz.toml` (texto). Si la dep no existe en
/// `[dependencies]`, devuelve `false` (sin error — el caller decide
/// si reportar como warning o error según el comando). Si `[dependencies]`
/// queda vacío tras quitar la entry, la sección se borra entera para
/// no dejar ruido.
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

/// Construye la inline table TOML para una dep agregada por
/// `fitz add`. La forma es `{ path = "..." }` o `{ git = "...",
/// tag = "..." }` / `{ git = "...", rev = "..." }`.
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

// ---- Fase 9.y.3.b — registry de deps consumido por evaluator + codegen ----

/// Map liviano `dep-name → lib_entry-absoluto` que pasamos al loader
/// del evaluator (`fitz run`) y al loader del codegen (`fitz build`).
///
/// Fase 9.y.3.b: el loader chequea esto ANTES de fallback a paths
/// relativos del importer. Cuando `from utils_lib import X` aparece y
/// `utils_lib` está acá, el loader carga directo desde `lib_entry`.
///
/// Hyphens en nombres de paquete: aceptados en el manifest, pero NO
/// pueden aparecer en imports Fitz porque el parser no acepta `-` en
/// identificadores. Una dep `utils-lib` queda en el registry pero
/// `from utils-lib import X` no parsea. Convención hasta 9.y.4:
/// nombrar deps con `_` o sin separador si vas a importarlas.
pub type DepRegistry = std::collections::HashMap<String, PathBuf>;

/// Helper: arma el registry desde una lista de `ResolvedDep`. Cada dep
/// entra una vez por su `name` (el nombre con el que aparece en
/// `[dependencies]` del importer).
pub fn build_dep_registry(deps: &[ResolvedDep]) -> DepRegistry {
    deps.iter()
        .map(|d| (d.name.clone(), d.lib_entry.clone()))
        .collect()
}

// ---- Fase 9.y.3.a — resolución de dependencias ----

/// Una dep resuelta a path absoluto + metadata necesaria para que el
/// lockfile y (eventualmente, 9.y.3.b) el loader puedan localizar el
/// código.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedDep {
    /// Nombre con el que la dep aparece en `[dependencies]` del
    /// importer. Puede ser distinto al `package.name` del manifest de
    /// la dep si el usuario usa aliasing (futuro 9.y.4); por ahora
    /// son iguales.
    pub name: String,
    /// `[package].version` del manifest de la dep. Para path deps esto
    /// es solo informativo (no se valida contra rangos — el roadmap
    /// para constraints semver entra con 9.y.5).
    pub version: String,
    /// Path absoluto al directorio raíz de la dep (donde vive su
    /// `fitz.toml`).
    pub abs_path: PathBuf,
    /// Path absoluto al entry de la librería (`[lib].entry` de la
    /// dep, resuelto contra `abs_path`).
    pub lib_entry: PathBuf,
    /// Tipo de source, para diferenciar en el lockfile y (futuro
    /// 9.y.3.b) en el cache. Por ahora solo `Path`.
    pub source: ResolvedDepSource,
}

/// Tipo de origen de una dep resuelta.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedDepSource {
    /// Path dep. El campo guarda el path tal cual aparece en el
    /// manifest del importer (no canonicalizado) para preservar la
    /// intención del usuario en mensajes y diffs del lockfile.
    Path { declared: String },
    /// Git dep (9.y.3.c). `url` es la URL tal como la declaró el
    /// usuario; `requested` distingue Tag/Rev; `commit_hash` es el
    /// SHA exacto que terminamos checkout-eando (consumido por el
    /// lockfile como `source = "git+<url>#<commit>"`).
    Git {
        url: String,
        requested: crate::git_dep::GitRef,
        commit_hash: String,
    },
    // `Registry { url, version }` llega en 9.y.5.
}

/// Resuelve TODAS las deps del manifest contra el filesystem.
/// Devuelve `ResolvedDep` ordenados por nombre (determinístico,
/// importante para diffs del lockfile).
///
/// El `manifest_dir` es el directorio que contiene el `fitz.toml`
/// del importer; sirve para resolver los `path` relativos.
///
/// Errores: corta en la primera dep que falla. Si querés acumular
/// todos los errores, hace falta refactor (sub-paso futuro).
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

            // path + git: combinación inválida (cuál prioridad?).
            if has_path && has_git {
                return Err(ManifestError::DepInvalidGitShape {
                    name: name.to_string(),
                    reason: "no se puede combinar `path` con `git` en la misma dep. \
                             Usá uno u otro."
                        .to_string(),
                });
            }

            if has_path {
                // path-only deps (9.y.3.a) — el resto de los fields
                // git-relacionados se ignoran silenciosamente. No
                // erroneamos para no romper el caso donde el usuario
                // está iterando entre `path` y `git`.
                let path_str = d.path.as_ref().expect("has_path checked");
                return resolve_path_dep(name, path_str, manifest_dir);
            }

            if has_git {
                let url = d.git.as_ref().expect("has_git checked");
                let gitref = parse_git_ref(name, d.tag.as_deref(), d.rev.as_deref())?;
                return resolve_git_dep(name, url, gitref);
            }

            // Ni path ni git pero alguno de tag/rev — usuario olvidó
            // el url. Mensaje específico.
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

/// Valida tag/rev: exactamente uno de los dos debe estar presente.
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

/// Resolución de git deps (9.y.3.c). Clona o reusa el cache, lee el
/// manifest de la dep, valida `[lib]`, y devuelve un `ResolvedDep`
/// con `source = ResolvedDepSource::Git { ... }`.
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
        // `authors`, `description`, `license`, `dependencies` deben
        // estar omitidos cuando están vacíos (skip_serializing_if).
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
        // Canonicalizar ambos lados — en Windows el tmp puede expandir
        // `~/AppData/Local/Temp` a un path distinto al de tempfile.
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

    // ---- Fase 9.y.3.a — Dependency / Lib / resolve_dependencies ----

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

    /// Helper: crea un proyecto path-dep candidato (manifest con
    /// `[lib]` + entry file vacío) en `target/`. Devuelve el path
    /// absoluto al directorio creado.
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

    // (El test viejo `resolve_dependencies_git_aborta_citando_9y3c`
    // se eliminó al cerrar 9.y.3.c — git deps son ahora soportadas.
    // La validación de shape la cubren los tests `resolve_git_dep_*`
    // arriba.)

    // ---- Fase 9.y.4 — edición del manifest (add/remove preservan formato) ----

    #[test]
    fn add_dep_a_manifest_sin_dependencies_crea_la_seccion() {
        let original = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[bin]\nmain = \"src/main.fitz\"\n";
        let spec = AddDepSpec::Path {
            path: "../utils".to_string(),
        };
        let updated = add_dep_to_manifest(original, "utils", &spec).unwrap();
        assert!(updated.contains("[dependencies]"));
        assert!(updated.contains("utils = { path = \"../utils\" }"));
        // El resto del manifest sigue intacto.
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
        // La dep previa sigue ahí.
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
        // El manifest queda intacto.
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
        // Tras add+remove de la MISMA dep, el manifest debe quedar
        // semánticamente equivalente al original. Acepto pequeñas
        // diferencias de formatting (toml_edit puede normalizar
        // whitespace) pero el shape es el mismo.
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
        // El parser debe ver el mismo manifest semánticamente.
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
        // Manifest solo con [bin], sin [lib] — no se puede importar.
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
        // El mensaje sugiere agregar [lib].
        let msg = err.to_string();
        assert!(msg.contains("[lib]"), "msg: {msg}");
        assert!(msg.contains("entry"), "msg: {msg}");
    }

    // ---- Fase 9.y.3.c — validaciones de shape de git deps ----

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
        // No creamos fitz.toml. Sube hasta el root del FS sin
        // encontrarlo (asumiendo que no hay un fitz.toml en root del
        // sistema, lo cual es razonable).
        assert!(find_manifest(&nested).is_none());
    }
}
