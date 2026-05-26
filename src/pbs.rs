//! python-build-standalone (PBS) — descarga + cache local del tarball
//! CPython para `fitz build --bundle-python` (Fase 8.b).
//!
//! Descarga el tarball `install_only_stripped` de la release pinned de
//! PBS para el triple destino y lo guarda en
//! `<cache>/pbs/<tarball-name>` (cache global compartido entre builds).
//!
//! **Decisiones técnicas tomadas**:
//!
//! - **Release pinned** (constante `PBS_RELEASE`): builds reproducibles.
//!   Bump manual c/3-6 meses. Misma política que `Cargo.lock`.
//! - **CPython 3.14.x**: versión más reciente estable disponible en
//!   PBS y dentro del rango `abi3-py310` (3.10-3.14+). PyO3 con
//!   `abi3-py310` + `auto-initialize` linkea dinámico contra
//!   libpython específica del builder; bundlear PBS 3.14 exige
//!   builder con Python 3.14.x. Cuando cierre
//!   `R.bug-pyo3-abi3-portable-link`, el constraint desaparece
//!   (linkea contra `libpython3.so` stable ABI).
//! - **Sabor `install_only_stripped`**: drop-in Python directory
//!   portable (extrae a temp dir + corre `python.exe` directo), sin
//!   debug symbols. ~70% más chico que `install_only` (Linux x64: 33
//!   MB vs 117 MB comprimido).
//! - **Subprocess `curl`** para descarga: cero deps Rust nuevas, curl
//!   está garantizado en Windows 11/macOS/Linux moderno. Trade-off:
//!   no hay progress bar. Si aparece presión sumamos `ureq` o
//!   `reqwest` después.
//! - **Override del cache via `FITZ_CACHE_DIR`**: paralelo a
//!   `git_dep.rs`. Para tests (tempdirs aislados) y power users.
//! - **Cache key por tarball name** (no por release+triple separados):
//!   permite tener múltiples versiones de Python coexistiendo en el
//!   cache sin overwrites (útil para futuras versiones soportadas).

use std::path::{Path, PathBuf};
use std::process::Command;

// === Constantes pinned ===

/// Release de PBS pinned (formato YYYYMMDD). Bump manual.
pub const PBS_RELEASE: &str = "20260510";

/// Versión de CPython embebida (debe estar disponible en `PBS_RELEASE`).
pub const PYTHON_VERSION: &str = "3.14.5";

/// Sabor del tarball. `install_only_stripped` tiene los mismos archivos
/// que `install_only` pero sin debug symbols.
pub const TARBALL_FLAVOR: &str = "install_only_stripped";

/// Extensión del archivo de descarga.
pub const TARBALL_EXTENSION: &str = "tar.gz";

/// Nombre de la env var que override-a el root del cache (compartida
/// con `git_dep.rs`).
pub const CACHE_DIR_ENV: &str = "FITZ_CACHE_DIR";

// === Triples soportados ===

/// Lista de triples Rust para los que PBS publica tarballs y nosotros
/// soportamos para bundling. Si el usuario está en otro triple,
/// `host_triple()` falla con error claro.
pub fn supported_triples() -> &'static [&'static str] {
    &[
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
    ]
}

/// Triple del host actual, detectado via `cfg!`. Match exhaustivo
/// contra los 5 triples que PBS publica. Si no matchea ninguno (ej.
/// musl, freebsd, riscv), devuelve `PbsError::UnsupportedTriple`.
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

/// Nombre del archivo tarball para un triple dado. Formato canónico
/// usado por PBS: `cpython-<version>+<release>-<triple>-<flavor>.<ext>`.
pub fn tarball_name(triple: &str) -> String {
    format!(
        "cpython-{}+{}-{}-{}.{}",
        PYTHON_VERSION, PBS_RELEASE, triple, TARBALL_FLAVOR, TARBALL_EXTENSION
    )
}

/// URL de descarga del tarball en GitHub Releases de PBS.
pub fn download_url(triple: &str) -> String {
    format!(
        "https://github.com/astral-sh/python-build-standalone/releases/download/{}/{}",
        PBS_RELEASE,
        tarball_name(triple)
    )
}

// === Cache ===

/// Devuelve el root del cache: `$FITZ_CACHE_DIR` si está seteada, si
/// no `~/.fitz/cache`. Comparte la convención con `git_dep.rs`.
pub fn cache_root() -> Result<PathBuf, PbsError> {
    if let Ok(override_dir) = std::env::var(CACHE_DIR_ENV) {
        if !override_dir.is_empty() {
            return Ok(PathBuf::from(override_dir));
        }
    }
    let home = home_dir().ok_or(PbsError::NoHomeDir)?;
    Ok(home.join(".fitz").join("cache"))
}

/// Subdirectorio del cache donde viven los tarballs PBS descargados.
pub fn pbs_cache_root() -> Result<PathBuf, PbsError> {
    Ok(cache_root()?.join("pbs"))
}

/// Path absoluto del tarball cacheado para un triple. No toca disco.
pub fn cache_path_for(triple: &str) -> Result<PathBuf, PbsError> {
    Ok(pbs_cache_root()?.join(tarball_name(triple)))
}

/// Garantiza que el tarball PBS para `triple` esté en cache. Si ya
/// existe, devuelve su path. Si no, lo descarga via `curl` y devuelve
/// el path resultante. La descarga es atómica: descarga a `<path>.tmp`
/// + rename al final (evita estado parcial si interrupted).
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

/// Descarga el tarball a `target` via `curl`. Usa archivo `.tmp` +
/// rename para que el cache no quede con archivos parciales si la
/// descarga se interrumpe.
fn download_tarball(triple: &str, target: &Path) -> Result<(), PbsError> {
    let url = download_url(triple);
    let tmp = target.with_extension("tar.gz.tmp");

    // Borrar `.tmp` previo si quedó de una corrida interrumpida.
    if tmp.exists() {
        std::fs::remove_file(&tmp).map_err(PbsError::Io)?;
    }

    // -s: silent (sin progress bar — el output sería ruido en CI).
    // -L: follow redirects (GitHub redirecciona a fastly).
    // --fail: exit code != 0 si HTTP 4xx/5xx (sin esto, curl
    //         "exitosamente" descarga la página 404 de GitHub).
    // -o: archivo destino.
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

    // Rename atómico (cross-platform). Si target apareció mientras
    // descargábamos (race con otra corrida concurrente), rename lo
    // sobreescribe — no es problema porque el contenido es el mismo
    // (release pinned).
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

// === Errores ===

#[derive(Debug)]
pub enum PbsError {
    /// El triple del host no está entre los 5 que PBS publica y
    /// nosotros soportamos.
    UnsupportedHostTriple,
    /// `curl` no está en el `PATH` o no se pudo ejecutar.
    CurlNotFound(std::io::Error),
    /// La descarga HTTP falló (404, timeout, etc.).
    DownloadFailed {
        url: String,
        triple: String,
        stderr: String,
    },
    /// No se pudo determinar el home directory.
    NoHomeDir,
    /// Error de I/O al manipular el cache.
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

    /// Serializa los tests que mutan la env var global `FITZ_CACHE_DIR`.
    /// `cargo test` corre por default en paralelo y el env es proceso-
    /// global; sin lock, un test podía leer mientras otro hacía
    /// `remove_var` y devolver el path default `~/.fitz/cache` en vez
    /// del override del `tempdir`. El CI de Windows lo cazaba flake.
    /// `parking_lot::Mutex` (vs `std::sync::Mutex`) no envenena en
    /// panic — si un test `assert!` adentro del guard panic-ea, los
    /// que esperan siguen adelante sin propagar el poison.
    static ENV_VAR_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn pbs_release_es_string_yyyymmdd() {
        // Sanity check sobre el formato del release pinned.
        assert_eq!(PBS_RELEASE.len(), 8, "release debería ser YYYYMMDD");
        assert!(PBS_RELEASE.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn python_version_es_3_14_x() {
        // Sanity check: estamos pineados a CPython 3.14.x (último
        // stable dentro del rango `abi3-py310` que PyO3 soporta).
        assert!(PYTHON_VERSION.starts_with("3.14."));
    }

    #[test]
    fn tarball_name_formato_canonico() {
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
    fn download_url_apunta_a_github_releases() {
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
    fn supported_triples_cubre_5_plataformas() {
        let triples = supported_triples();
        assert_eq!(triples.len(), 5);
        assert!(triples.contains(&"x86_64-unknown-linux-gnu"));
        assert!(triples.contains(&"aarch64-unknown-linux-gnu"));
        assert!(triples.contains(&"x86_64-pc-windows-msvc"));
        assert!(triples.contains(&"aarch64-apple-darwin"));
        assert!(triples.contains(&"x86_64-apple-darwin"));
    }

    #[test]
    fn host_triple_detecta_el_host_actual() {
        // En cualquier plataforma soportada esto debe devolver Ok.
        // Si el test corre en una plataforma no soportada (ej. musl),
        // devuelve Err — aceptable, no es nuestro target.
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
    fn cache_path_for_combina_root_y_tarball_name() {
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
    fn cache_root_usa_env_override() {
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
    fn pbs_error_display_unsupported_triple_lista_soportados() {
        let s = format!("{}", PbsError::UnsupportedHostTriple);
        for triple in supported_triples() {
            assert!(
                s.contains(triple),
                "el mensaje debería mencionar el triple {triple}"
            );
        }
    }

    #[test]
    fn pbs_error_display_download_failed_incluye_url_y_stderr() {
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

    // NOTA: tests de descarga real (ensure_tarball / download_tarball
    // contra GitHub) viven en `tests/cli_e2e.rs` porque son lentos
    // (~5-30s), requieren red, y se benefician de tempdirs aislados.
}
