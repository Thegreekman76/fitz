//! Git deps — clonado + cache local (Fase 9.y.3.c).
//!
//! Habilita `[dependencies] foo = { git = "https://...", tag = "v1.0.0" }`
//! en `fitz.toml`. El primer acceso clona el repo a
//! `<cache>/git/<sanitized-url>@<ref>/` (cache global) y reusa el dir
//! en accesos siguientes.
//!
//! **Decisiones técnicas tomadas**:
//!
//! - **Subprocess `git`** en lugar de crate (`git2`/`gix`): cero deps
//!   adicionales, asume `git` en el `PATH` (lo cual ya es el caso para
//!   cualquier dev de Fitz). Si falla, error claro.
//! - **`tag` o `rev`**, NUNCA `branch`: branches mutan upstream y
//!   rompen reproducibilidad. Esta restricción se valida acá.
//! - **`tag` y `rev` mutuamente exclusivos**: ambos especifican un
//!   "punto fijo"; mezclar genera ambigüedad.
//! - **Cache directory naming**: URL sanitizada + `@` + ref. Sin
//!   hashing, determinístico y human-readable
//!   (`github.com_foo_bar@v1.0.0/`). Trade-off: URLs muy largas o con
//!   chars exóticos podrían colisionar; en MVP truncamos a 200 chars
//!   y aceptamos el caso 99%.
//! - **Cache reuse**: si el dir ya existe, asumimos clonado correcto
//!   y solo leemos el commit hash. Sin re-clone automático.
//!   Invalidación manual (borrar el dir o `fitz cache clean` post-MVP).
//! - **Override del cache via env var `FITZ_CACHE_DIR`**: para tests
//!   (que necesitan tempdirs aislados) y power users que quieran
//!   compartir cache entre máquinas o moverlo de disco.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Nombre de la env var que override-a el root del cache.
pub const CACHE_DIR_ENV: &str = "FITZ_CACHE_DIR";

/// Devuelve el root del cache: `$FITZ_CACHE_DIR` si está seteada, si
/// no `~/.fitz/cache`. Si tampoco hay home, falla — sin cache root
/// no podemos manejar git deps.
pub fn cache_root() -> Result<PathBuf, GitDepError> {
    if let Ok(override_dir) = std::env::var(CACHE_DIR_ENV) {
        if !override_dir.is_empty() {
            return Ok(PathBuf::from(override_dir));
        }
    }
    let home = home_dir().ok_or(GitDepError::NoHomeDir)?;
    Ok(home.join(".fitz").join("cache"))
}

/// Subdirectorio del cache donde viven los clones git.
pub fn git_cache_root() -> Result<PathBuf, GitDepError> {
    Ok(cache_root()?.join("git"))
}

/// Devuelve el path absoluto del home del usuario. En Windows usa
/// `USERPROFILE`; en Unix `HOME`. Sin dep externa (no usamos `dirs`).
fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Ref pedida en el manifest para una git dep. Mutuamente exclusivos:
/// `Tag(s)` o `Rev(s)`, nunca ambos.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GitRef {
    Tag(String),
    Rev(String),
}

impl GitRef {
    /// String usado en el cache dir y en el lockfile. Mismo para tag
    /// y rev — el lockfile distingue por contexto (commit hash en
    /// `source`) si hace falta diferenciar.
    pub fn as_str(&self) -> &str {
        match self {
            GitRef::Tag(s) | GitRef::Rev(s) => s.as_str(),
        }
    }
}

/// Errores del módulo. Independiente de `ManifestError` — el caller
/// hace `From` o wrap cuando integra.
#[derive(Debug)]
pub enum GitDepError {
    /// `git` no está en el `PATH` o no se pudo ejecutar.
    GitNotFound(std::io::Error),
    /// `git clone` o `git checkout` o `git rev-parse` falló.
    /// Lleva el comando + el stderr para el mensaje.
    GitCommandFailed { command: String, stderr: String },
    /// No se pudo determinar el home directory (sin `HOME` ni
    /// `USERPROFILE`) y `FITZ_CACHE_DIR` tampoco está seteada.
    NoHomeDir,
    /// Error de I/O al manipular el cache directory.
    Io(std::io::Error),
    /// La validación del shape de la dep falló: por ejemplo, ambos
    /// `tag` y `rev` están presentes, o ninguno.
    InvalidGitDep(String),
}

impl std::fmt::Display for GitDepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitDepError::GitNotFound(e) => write!(
                f,
                "no se pudo invocar `git` ({e}). Instalalo y asegurate que esté en el PATH."
            ),
            GitDepError::GitCommandFailed { command, stderr } => {
                let trimmed = stderr.trim();
                if trimmed.is_empty() {
                    write!(f, "el comando `{command}` falló sin output")
                } else {
                    write!(f, "el comando `{command}` falló:\n{trimmed}")
                }
            }
            GitDepError::NoHomeDir => write!(
                f,
                "no se pudo determinar el home directory para ubicar el cache. \
                 Seteá `FITZ_CACHE_DIR=<path>` para apuntar a un directorio escribible."
            ),
            GitDepError::Io(e) => write!(f, "error de I/O sobre el cache: {e}"),
            GitDepError::InvalidGitDep(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for GitDepError {}

/// Sanitiza una URL para usarla como componente de path. Reemplaza
/// caracteres problemáticos por `_` y trunca a 200 chars para no
/// pasarse del límite de filesystem en Windows.
///
/// No es un hash — es transformación textual. Colisiones teóricas
/// existen (dos URLs muy distintas con prefijo común podrían
/// truncarse al mismo string) pero son irrelevantes en el caso 99%.
pub fn sanitize_url(url: &str) -> String {
    // Strip prefijo del schema para que el cache no se llene de
    // `https___...`. Aceptamos http, https, git, ssh, y file.
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

/// Construye el path absoluto del cache directory para una (url, ref)
/// dada, sin tocar disco. Útil para tests + para reportar errores
/// que mencionan la ubicación esperada.
pub fn cache_path_for(url: &str, gitref: &GitRef) -> Result<PathBuf, GitDepError> {
    let dir_name = format!("{}@{}", sanitize_url(url), sanitize_url(gitref.as_str()));
    Ok(git_cache_root()?.join(dir_name))
}

/// Resultado de resolver un git dep contra el cache.
#[derive(Debug, Clone, PartialEq)]
pub struct GitClonedRepo {
    /// Path absoluto al directorio del repo clonado.
    pub abs_path: PathBuf,
    /// Commit hash exacto (`git rev-parse HEAD` después del checkout).
    /// Se persiste en el lockfile como `source = "git+<url>#<commit>"`.
    pub commit_hash: String,
}

/// Garantiza que el repo esté clonado y checkeado al `gitref` pedido.
/// Si el cache ya existe, asume que el clone previo es válido y solo
/// lee el commit hash. Si no existe, clona desde cero.
///
/// El clone usa `--depth 1 --branch <tag-or-rev>`. Para revs (commit
/// SHA), git acepta como `--branch` solo si es resoluble pre-fetch;
/// si falla, hacemos fallback a clone completo + checkout explícito.
pub fn clone_or_use_cache(url: &str, gitref: &GitRef) -> Result<GitClonedRepo, GitDepError> {
    let target = cache_path_for(url, gitref)?;

    if !target.exists() {
        // Asegurar que el directorio padre existe.
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

/// Clone fresh: dos estrategias.
///
/// 1. Si `gitref` es Tag: `git clone --depth 1 --branch <tag> <url> <target>`
///    funciona y es eficiente.
/// 2. Si `gitref` es Rev (commit SHA): `--branch` no acepta SHAs.
///    Hacemos `git clone <url> <target>` full + `git checkout <sha>`.
///    Wasteful pero correcto. Optimización con `--filter=blob:none`
///    queda como deuda.
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

/// `git rev-parse HEAD` adentro del repo en `path`. Devuelve el SHA
/// completo (40 chars hex) sin trailing newline.
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

/// Ejecuta `git <args>` en el cwd actual. Reporta stderr en errors.
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

/// Ejecuta `git <args>` desde `cwd`. Reporta stderr en errors.
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

/// Construye el string que va al lockfile en `source` para una git
/// dep ya resuelta. Formato Cargo-style: `git+<url>#<commit-hash>`.
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
    fn sanitize_url_strips_otros_schemes() {
        assert_eq!(sanitize_url("http://x.com/r"), "x.com_r");
        assert_eq!(sanitize_url("git://example.org/p"), "example.org_p");
        assert_eq!(sanitize_url("ssh://user@host/r.git"), "user_host_r.git");
        assert_eq!(sanitize_url("file:///tmp/r"), "_tmp_r");
    }

    #[test]
    fn sanitize_url_preserva_letras_numeros_punto_guion_underscore() {
        assert_eq!(
            sanitize_url("https://github.com/some-user_name/proj.v1"),
            "github.com_some-user_name_proj.v1"
        );
    }

    #[test]
    fn sanitize_url_trunca_a_200_chars() {
        let very_long = format!("https://github.com/{}", "a".repeat(300));
        let s = sanitize_url(&very_long);
        assert!(s.len() <= 200);
    }

    #[test]
    fn sanitize_url_sin_prefix_acepta_input_raw() {
        // URLs sin scheme (raras) se aceptan tal cual.
        assert_eq!(sanitize_url("just/a/path"), "just_a_path");
    }

    #[test]
    fn gitref_as_str_funciona_para_ambas_variantes() {
        assert_eq!(GitRef::Tag("v1.0".to_string()).as_str(), "v1.0");
        assert_eq!(GitRef::Rev("abc123".to_string()).as_str(), "abc123");
    }

    #[test]
    fn cache_path_for_combina_url_sanitizada_y_ref() {
        let tmp = tempfile::tempdir().unwrap();
        // Override del cache para no tocar el home real durante tests.
        let prev = std::env::var(CACHE_DIR_ENV).ok();
        std::env::set_var(CACHE_DIR_ENV, tmp.path());

        let p = cache_path_for(
            "https://github.com/foo/bar",
            &GitRef::Tag("v1.0.0".to_string()),
        )
        .unwrap();
        // El path debe estar bajo el override + /git/<sanitized>@<ref>.
        assert!(p.starts_with(tmp.path()));
        assert!(p.ends_with("github.com_foo_bar@v1.0.0"));

        // Restaurar la env var (otros tests podrían correr en paralelo
        // con env vars distintas — atención, ver nota abajo).
        match prev {
            Some(v) => std::env::set_var(CACHE_DIR_ENV, v),
            None => std::env::remove_var(CACHE_DIR_ENV),
        }
    }

    #[test]
    fn lockfile_source_string_formato_cargo_style() {
        let s = lockfile_source_string(
            "https://github.com/foo/bar",
            "abc123def456789012345678901234567890abcd",
        );
        assert_eq!(
            s,
            "git+https://github.com/foo/bar#abc123def456789012345678901234567890abcd"
        );
    }

    // NOTA: los tests de clone_or_use_cache (que invocan git real
    // sobre repos bare locales) viven en tests/cli_e2e.rs porque
    // necesitan setup más elaborado (crear bare repo + commits + tag)
    // y se benefician de tempdirs aislados que no compiten por la
    // env var FITZ_CACHE_DIR.
}
