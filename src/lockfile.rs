//! Lockfile (`fitz.lock`) — Fase 9.y.3.a.
//!
//! Archivo paralelo al manifest que registra las deps resueltas con la
//! información exacta usada (path absoluto, commit hash para git,
//! versión para registry). Se commitea junto al `fitz.toml` — es la
//! garantía de reproducibilidad del build.
//!
//! El formato sigue **Cargo.lock** en lo simbólico: una lista de
//! `[[package]]` con `name`, `version`, y `source` opcional. Para path
//! deps no emitimos `source` (sigue la convención de Cargo: las path
//! deps son implícitas, no requieren campo source).
//!
//! Esquema versionado vía el campo `version = N` del top-level. La
//! v1 (9.y.3.a) solo soporta path deps; las extensiones para git
//! (9.y.3.c) y registry (9.y.5) pueden mantener compatibilidad con
//! v1 o bumpear según necesidad.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

use crate::manifest::{ResolvedDep, ResolvedDepSource};

/// Nombre del archivo de lockfile, adyacente al `fitz.toml`.
pub const LOCKFILE_FILE: &str = "fitz.lock";

/// Versión del esquema del lockfile. Bumpea si rompemos compat con
/// versiones previas (por ahora solo v1).
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
    /// Source string-encoded estilo Cargo. `None` para path deps (la
    /// convención Cargo: las path deps son implícitas y no llevan
    /// campo source en el lockfile). Para futuras git/registry deps
    /// será `Some("git+url#sha")` o `Some("registry+url")`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug)]
pub enum LockfileError {
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    /// El lockfile en disco usa una versión que no entendemos. El
    /// `found` lleva la versión leída para citar en el mensaje.
    UnsupportedVersion {
        found: u32,
    },
}

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockfileError::Parse(e) => write!(f, "error parseando lockfile: {e}"),
            LockfileError::Serialize(e) => {
                write!(f, "error serializando lockfile: {e}")
            }
            LockfileError::UnsupportedVersion { found } => write!(
                f,
                "versión `{found}` del lockfile no soportada por este \
                 binario (entiendo hasta v{CURRENT_LOCKFILE_VERSION}). \
                 Borralo y dejá que `fitz run`/`build`/`check` lo \
                 regenere, o actualizá `fitz`."
            ),
        }
    }
}

impl std::error::Error for LockfileError {}

impl Lockfile {
    /// Parsea un lockfile desde texto TOML. Valida que `version` sea
    /// una que entendemos.
    pub fn parse(input: &str) -> Result<Self, LockfileError> {
        let l: Lockfile = toml::from_str(input).map_err(LockfileError::Parse)?;
        if l.version > CURRENT_LOCKFILE_VERSION {
            return Err(LockfileError::UnsupportedVersion { found: l.version });
        }
        Ok(l)
    }

    /// Serializa a TOML.
    pub fn to_toml_string(&self) -> Result<String, LockfileError> {
        toml::to_string(self).map_err(LockfileError::Serialize)
    }

    /// Construye un lockfile fresco a partir de las deps resueltas.
    /// Ordena por `name` para que el archivo sea determinístico
    /// (importante para diffs de git).
    pub fn from_resolved(deps: &[ResolvedDep]) -> Self {
        let mut packages: Vec<LockedPackage> = deps
            .iter()
            .map(|d| LockedPackage {
                name: d.name.clone(),
                version: d.version.clone(),
                source: match &d.source {
                    // Path deps no llevan `source` en el lockfile
                    // (convención Cargo: son implícitas).
                    ResolvedDepSource::Path { .. } => None,
                    // Git deps emiten `git+<url>#<commit-hash>` —
                    // Fase 9.y.3.c. El hash exacto del commit permite
                    // reproducir el build aunque el tag upstream
                    // mute.
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

/// Determina si el lockfile en `path` ya tiene exactamente el mismo
/// contenido que `new`, comparando estructuralmente (no byte-a-byte —
/// permite normalizar serialización del TOML entre versiones).
///
/// Devuelve `true` si NO hace falta escribir (mismo contenido), `false`
/// si hay que escribir.
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

/// Escribe el lockfile a `path` si su contenido difiere del que ya
/// hay (o no hay nada). No-op si el contenido coincide. Devuelve
/// `Ok(true)` si escribió, `Ok(false)` si no hizo nada.
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

/// Helper para construir el path absoluto del lockfile dado el
/// directorio del manifest.
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
    fn from_resolved_vacio_emite_lockfile_v1_sin_packages() {
        let l = Lockfile::from_resolved(&[]);
        assert_eq!(l.version, CURRENT_LOCKFILE_VERSION);
        assert!(l.packages.is_empty());
    }

    #[test]
    fn from_resolved_path_dep_no_emite_source() {
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
    fn from_resolved_ordena_alfabeticamente_por_nombre() {
        let l = Lockfile::from_resolved(&[
            dep("zeta", "1.0.0", "../zeta"),
            dep("alfa", "0.1.0", "../alfa"),
            dep("medio", "0.5.0", "../medio"),
        ]);
        let names: Vec<&str> = l.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["alfa", "medio", "zeta"]);
    }

    #[test]
    fn serializa_omite_packages_y_source_vacios() {
        let l = Lockfile::from_resolved(&[]);
        let toml_text = l.to_toml_string().unwrap();
        // Solo el header version, sin [[package]].
        assert!(toml_text.contains("version = 1"));
        assert!(!toml_text.contains("[[package]]"));
        assert!(!toml_text.contains("source"));
    }

    #[test]
    fn round_trip_preserva_estructura() {
        let original =
            Lockfile::from_resolved(&[dep("a", "0.1.0", "../a"), dep("b", "0.2.0", "../b")]);
        let toml_text = original.to_toml_string().unwrap();
        let parsed = Lockfile::parse(&toml_text).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn parse_acepta_lockfile_vacio() {
        let text = "version = 1\n";
        let l = Lockfile::parse(text).unwrap();
        assert_eq!(l.version, 1);
        assert!(l.packages.is_empty());
    }

    #[test]
    fn parse_rechaza_version_futura() {
        let text = "version = 999\n";
        let err = Lockfile::parse(text).unwrap_err();
        match err {
            LockfileError::UnsupportedVersion { found } => assert_eq!(found, 999),
            other => panic!("se esperaba UnsupportedVersion, fue {other:?}"),
        }
    }

    #[test]
    fn parse_acepta_lockfile_con_packages() {
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
    fn lockfile_matches_detecta_igualdad_estructural() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let l = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, l.to_toml_string().unwrap()).unwrap();
        assert!(lockfile_matches(&path, &l));
    }

    #[test]
    fn lockfile_matches_detecta_diferencia() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let original = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, original.to_toml_string().unwrap()).unwrap();
        let other = Lockfile::from_resolved(&[dep("x", "2.0.0", "../x")]);
        assert!(!lockfile_matches(&path, &other));
    }

    #[test]
    fn lockfile_matches_false_si_no_existe() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("no-existe.lock");
        let l = Lockfile::from_resolved(&[]);
        assert!(!lockfile_matches(&path, &l));
    }

    #[test]
    fn write_lockfile_no_escribe_si_iguala() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let l = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, l.to_toml_string().unwrap()).unwrap();

        let mtime_before = std::fs::metadata(&path).unwrap().modified().unwrap();
        // Esperar un poquito para que un eventual write produzca distinto mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));

        let wrote = write_lockfile_if_changed(&path, &l).unwrap();
        assert!(!wrote, "no debió escribir porque el contenido coincide");

        let mtime_after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "no debió tocar el archivo");
    }

    #[test]
    fn write_lockfile_escribe_si_difiere() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let old = Lockfile::from_resolved(&[dep("x", "1.0.0", "../x")]);
        std::fs::write(&path, old.to_toml_string().unwrap()).unwrap();

        let new = Lockfile::from_resolved(&[dep("x", "2.0.0", "../x")]);
        let wrote = write_lockfile_if_changed(&path, &new).unwrap();
        assert!(wrote, "debió escribir porque el contenido cambió");

        let on_disk = Lockfile::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, new);
    }

    #[test]
    fn write_lockfile_escribe_si_no_existe() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fitz.lock");
        let l = Lockfile::from_resolved(&[]);
        let wrote = write_lockfile_if_changed(&path, &l).unwrap();
        assert!(wrote);
        assert!(path.is_file());
    }
}
