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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Bin>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
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
/// Sin uso en 9.y.1 (no hay consumidores todavía). Lo dejamos listo para
/// 9.y.2 cuando `fitz run`/`build` empiecen a leer el manifest. Marcado
/// `#[allow(dead_code)]` puntual hasta entonces.
#[allow(dead_code)]
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
        assert_eq!(m.dependencies.get("fitz-uuid").map(|s| s.as_str()), Some("1.0.0"));
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
