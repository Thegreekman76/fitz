//! Launcher template para `fitz build --bundle-python` (Fase 8.b.3).
//!
//! Cuando `--bundle-python` está activo, el output de `fitz build` es un
//! binario standalone Rust ("launcher") que internamente lleva embebidos:
//!  - El tarball PBS (CPython 3.13.x install_only_stripped)
//!  - El "real binary" (transpile estándar del programa Fitz con
//!    feature `python`, linkea libpython como hoy)
//!
//! En primer run, el launcher extrae todo a `$TMPDIR/fitz-py-<hash>/`,
//! setea `PYTHONHOME` + `LD_LIBRARY_PATH`/`DYLD_FALLBACK_LIBRARY_PATH`/
//! `PATH` según el OS, y exec/spawn-and-wait del real binary. Runs
//! subsecuentes reusan el dir extraído (sentinel `.extracted` marca
//! completitud).
//!
//! **Patrón validado en producción**: Datasette Desktop (Simon
//! Willison, 2021) ships así con bsdtar + PBS. Es el modelo
//! recomendado por la investigación del 2026-05-23 después de
//! descartar:
//!  - "Extract on first run + set PYTHONHOME in main()": no funciona,
//!    el OS resuelve libpython ANTES de main() (Linux: `DT_NEEDED`
//!    vía ld.so; macOS: `LC_LOAD_DYLIB` vía dyld; Windows: import
//!    table).
//!  - Linking estático con PBS "full": "multi-month rabbit hole",
//!    PyOxidizer ralentizado desde 2024.
//!  - Manual delay-load/dlopen: sin soporte documentado en PyO3,
//!    brittle entre versiones.
//!
//! **Decisiones técnicas tomadas**:
//!
//! - **Subprocess `tar -xzf`**: cero deps Rust en el launcher.
//!   bsdtar/GNU tar disponible nativo en Windows 11
//!   (`C:\WINDOWS\system32\tar.exe`), macOS, y todo Linux moderno.
//! - **Placeholders `__FITZ_REPLACE_*__`**: strings literales Rust
//!   válidos. Si el template no se sustituye (bug del codegen),
//!   `include_bytes!` falla con "no such file" — buena señal de
//!   error en build time, no en runtime.
//! - **`exec` en Unix vs `Command::status` en Windows**: en Unix
//!   `execv` reemplaza el proceso (signals/stdin/stdout transparentes,
//!   el OS forwarea todo). En Windows no hay exec real; usamos
//!   `Command::status` con inherit handles + propagamos el exit code.
//! - **Sentinel `.extracted`**: última cosa que escribimos. Si crash
//!   durante extract, el sentinel no existe y la próxima corrida
//!   re-extrae (no corrupción persistente).
//! - **Rename atómico**: extraemos a `<base>.tmp` + `fs::rename` a
//!   `<base>`. Si otra corrida concurrente nos ganó (race), el
//!   second rename falla y descartamos nuestro tmp — el resultado
//!   del primero es igual porque el tarball es determinístico.
//! - **TARBALL_HASH como string**: calculado por el `fitz build`
//!   (SHA256 truncado a 16 chars) y embebido. Cambio del tarball =
//!   cambio del hash = extract dir nuevo (cache automático por
//!   versión).

/// Placeholder reemplazado por el codegen con el path absoluto al
/// tarball PBS (resuelto vía `pbs::ensure_tarball(triple)?`).
pub const PLACEHOLDER_TARBALL_PATH: &str = "__FITZ_REPLACE_PBS_TARBALL_PATH__";

/// Placeholder reemplazado con el path absoluto al real binary
/// (recién buildeado por el codegen del sub-paso 8.b.2).
pub const PLACEHOLDER_REAL_BINARY_PATH: &str = "__FITZ_REPLACE_REAL_BINARY_PATH__";

/// Placeholder reemplazado con un hash corto del tarball (16 chars
/// hex). Identifica el extract dir en `$TMPDIR`.
pub const PLACEHOLDER_TARBALL_HASH: &str = "__FITZ_REPLACE_TARBALL_HASH__";

/// Template Rust del `main.rs` del launcher. Los tres placeholders se
/// reemplazan vía `gen_launcher_main_rs()` antes de escribirlo al
/// Cargo project del launcher.
pub const LAUNCHER_MAIN_RS_TEMPLATE: &str = r#"// Auto-generado por `fitz build --bundle-python`.
// NO editar — re-generado en cada build.
//
// Launcher: extrae CPython embebido + real binary a $TMPDIR en primer
// run, setea env (PYTHONHOME + LD_LIBRARY_PATH/DYLD/PATH), y exec del
// real binary. Subsecuentes runs son instantáneos (cache TMP).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const PBS_TARBALL: &[u8] = include_bytes!("__FITZ_REPLACE_PBS_TARBALL_PATH__");
const REAL_BINARY: &[u8] = include_bytes!("__FITZ_REPLACE_REAL_BINARY_PATH__");
const TARBALL_HASH: &str = "__FITZ_REPLACE_TARBALL_HASH__";

#[cfg(windows)]
const REAL_BINARY_NAME: &str = "fitz-real.exe";
#[cfg(not(windows))]
const REAL_BINARY_NAME: &str = "fitz-real";

fn main() {
    let extracted = ensure_extracted()
        .unwrap_or_else(|e| die(&format!("failed to extract bundled Python: {e}")));

    let real_binary = extracted.join(REAL_BINARY_NAME);
    let python_home = extracted.join("python");

    env::set_var("PYTHONHOME", &python_home);

    #[cfg(target_os = "linux")]
    prepend_path_env("LD_LIBRARY_PATH", &python_home.join("lib"));
    #[cfg(target_os = "macos")]
    prepend_path_env("DYLD_FALLBACK_LIBRARY_PATH", &python_home.join("lib"));
    #[cfg(windows)]
    prepend_path_env("PATH", &python_home);

    let args: Vec<String> = env::args().skip(1).collect();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new(&real_binary).args(&args).exec();
        die(&format!("exec failed: {err}"));
    }

    #[cfg(windows)]
    {
        let status = Command::new(&real_binary)
            .args(&args)
            .status()
            .unwrap_or_else(|e| die(&format!("spawn failed: {e}")));
        exit(status.code().unwrap_or(127));
    }
}

fn die(msg: &str) -> ! {
    eprintln!("fitz: {msg}");
    exit(127);
}

fn ensure_extracted() -> std::io::Result<PathBuf> {
    let base = env::temp_dir().join(format!("fitz-py-{}", TARBALL_HASH));
    let sentinel = base.join(".extracted");

    if sentinel.exists() {
        return Ok(base);
    }

    let tmp = base.with_extension("tmp");
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    fs::create_dir_all(&tmp)?;

    let tarball_path = tmp.join("__fitz_pbs.tar.gz");
    fs::write(&tarball_path, PBS_TARBALL)?;

    let tar_status = Command::new("tar")
        .args([
            "-xzf",
            &tarball_path.to_string_lossy(),
            "-C",
            &tmp.to_string_lossy(),
        ])
        .status()?;

    if !tar_status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "tar extraction failed",
        ));
    }

    let _ = fs::remove_file(&tarball_path);

    let real_binary_dest = tmp.join(REAL_BINARY_NAME);
    fs::write(&real_binary_dest, REAL_BINARY)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&real_binary_dest)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&real_binary_dest, perms)?;
    }

    fs::write(tmp.join(".extracted"), b"")?;

    if base.exists() {
        let _ = fs::remove_dir_all(&tmp);
    } else if let Err(e) = fs::rename(&tmp, &base) {
        let _ = fs::remove_dir_all(&tmp);
        if !base.exists() {
            return Err(e);
        }
    }

    Ok(base)
}

fn prepend_path_env(name: &str, dir: &Path) {
    #[cfg(windows)]
    const SEP: &str = ";";
    #[cfg(not(windows))]
    const SEP: &str = ":";

    let new = match env::var(name) {
        Ok(existing) if !existing.is_empty() => {
            format!("{}{}{}", dir.display(), SEP, existing)
        }
        _ => dir.display().to_string(),
    };
    env::set_var(name, new);
}
"#;

/// Cargo.toml del launcher. Cero deps externas — solo std. El nombre
/// del package es `__fitz_launcher__` (placeholder) para evitar
/// colisión con el real binary (que usa el package name del fuente
/// `.fitz` sanitizado).
pub const LAUNCHER_CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "fitz_launcher"
version = "0.0.0"
edition = "2021"

[[bin]]
name = "__FITZ_REPLACE_BIN_NAME__"
path = "src/main.rs"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
"#;

/// Placeholder en `LAUNCHER_CARGO_TOML_TEMPLATE` para el nombre del
/// binario final (sanitizado del stem del `.fitz` original).
pub const PLACEHOLDER_BIN_NAME: &str = "__FITZ_REPLACE_BIN_NAME__";

/// Personaliza el template del `main.rs` del launcher con paths reales
/// y el hash del tarball. Devuelve el código Rust listo para escribir
/// al Cargo project del launcher.
///
/// Los paths se escapan para que sean string literales Rust válidos
/// (backslashes de Windows → `\\`, doble quotes → `\"`).
pub fn gen_launcher_main_rs(tarball_path: &str, real_binary_path: &str, tarball_hash: &str) -> String {
    LAUNCHER_MAIN_RS_TEMPLATE
        .replace(PLACEHOLDER_TARBALL_PATH, &escape_rust_string_literal(tarball_path))
        .replace(
            PLACEHOLDER_REAL_BINARY_PATH,
            &escape_rust_string_literal(real_binary_path),
        )
        .replace(PLACEHOLDER_TARBALL_HASH, &escape_rust_string_literal(tarball_hash))
}

/// Personaliza el template del Cargo.toml del launcher con el nombre
/// del binario final.
pub fn gen_launcher_cargo_toml(bin_name: &str) -> String {
    LAUNCHER_CARGO_TOML_TEMPLATE.replace(PLACEHOLDER_BIN_NAME, bin_name)
}

/// Escapa una string para que sea un string literal Rust válido.
/// Maneja backslash (Windows paths), double quote, y newlines.
fn escape_rust_string_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// Calcula un hash corto (16 chars hex) determinístico del tarball PBS.
/// Usa una implementación simple de FNV-1a 64-bit — no necesita ser
/// criptográfico, solo identificar de forma única el contenido del
/// tarball para el dir name en `$TMPDIR`.
///
/// FNV-1a es suficiente porque:
///  - El input es un tarball PBS conocido (no atacante adversario).
///  - Colisión accidental entre dos tarballs PBS distintos es
///    astronómicamente improbable.
///  - Cero deps: implementación de 8 LoC.
pub fn tarball_hash_short(tarball_bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x100_0000_01b3;
    let mut hash: u64 = FNV_OFFSET;
    for b in tarball_bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_son_strings_validos_rust() {
        // Los placeholders deben ser identificadores entre comillas
        // dobles, válidos como string literal Rust.
        assert!(PLACEHOLDER_TARBALL_PATH.starts_with("__FITZ_REPLACE_"));
        assert!(PLACEHOLDER_REAL_BINARY_PATH.starts_with("__FITZ_REPLACE_"));
        assert!(PLACEHOLDER_TARBALL_HASH.starts_with("__FITZ_REPLACE_"));
        assert!(PLACEHOLDER_BIN_NAME.starts_with("__FITZ_REPLACE_"));
    }

    #[test]
    fn template_main_rs_contiene_los_3_placeholders() {
        assert!(LAUNCHER_MAIN_RS_TEMPLATE.contains(PLACEHOLDER_TARBALL_PATH));
        assert!(LAUNCHER_MAIN_RS_TEMPLATE.contains(PLACEHOLDER_REAL_BINARY_PATH));
        assert!(LAUNCHER_MAIN_RS_TEMPLATE.contains(PLACEHOLDER_TARBALL_HASH));
    }

    #[test]
    fn template_cargo_toml_contiene_bin_name_placeholder() {
        assert!(LAUNCHER_CARGO_TOML_TEMPLATE.contains(PLACEHOLDER_BIN_NAME));
    }

    #[test]
    fn gen_launcher_main_rs_sustituye_los_3_placeholders() {
        let result = gen_launcher_main_rs("/tmp/tarball.tar.gz", "/tmp/fitz-real", "abc123");

        // Los placeholders ya no deben aparecer en el output.
        assert!(!result.contains(PLACEHOLDER_TARBALL_PATH));
        assert!(!result.contains(PLACEHOLDER_REAL_BINARY_PATH));
        assert!(!result.contains(PLACEHOLDER_TARBALL_HASH));

        // Los valores reemplazados deben aparecer.
        assert!(result.contains("/tmp/tarball.tar.gz"));
        assert!(result.contains("/tmp/fitz-real"));
        assert!(result.contains("abc123"));
    }

    #[test]
    fn gen_launcher_main_rs_escapa_backslashes_de_windows() {
        let result = gen_launcher_main_rs(
            r"C:\Users\test\tarball.tar.gz",
            r"C:\Users\test\fitz-real.exe",
            "abc123",
        );

        // En Rust source, los backslashes deben aparecer escapados.
        assert!(
            result.contains(r"C:\\Users\\test\\tarball.tar.gz"),
            "el path Windows debería tener backslashes escapados"
        );
        assert!(result.contains(r"C:\\Users\\test\\fitz-real.exe"));

        // Los backslashes raw NO deben aparecer en el include_bytes!
        // (string literal Rust con `\` raw es ilegal salvo en raw strings).
        let include_bytes_line = result
            .lines()
            .find(|l| l.contains("PBS_TARBALL: &[u8]"))
            .expect("debe haber línea include_bytes! del tarball");
        assert!(!include_bytes_line.contains(r"C:\Users"));
        assert!(include_bytes_line.contains(r"C:\\Users"));
    }

    #[test]
    fn gen_launcher_main_rs_escapa_double_quotes() {
        // Edge case improbable pero posible (paths con espacios y
        // comillas en Windows — no es típico pero válido).
        let result = gen_launcher_main_rs("/tmp/a\"b.tar.gz", "/tmp/real", "h");
        assert!(result.contains("/tmp/a\\\"b.tar.gz"));
    }

    #[test]
    fn gen_launcher_cargo_toml_sustituye_bin_name() {
        let result = gen_launcher_cargo_toml("hola");
        assert!(!result.contains(PLACEHOLDER_BIN_NAME));
        assert!(result.contains("name = \"hola\""));
    }

    #[test]
    fn tarball_hash_short_devuelve_16_chars_hex() {
        let h = tarball_hash_short(b"hello world");
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tarball_hash_short_es_deterministico() {
        let h1 = tarball_hash_short(b"foo bar baz");
        let h2 = tarball_hash_short(b"foo bar baz");
        assert_eq!(h1, h2);
    }

    #[test]
    fn tarball_hash_short_cambia_con_input_distinto() {
        let h1 = tarball_hash_short(b"hello");
        let h2 = tarball_hash_short(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn tarball_hash_short_empty_input_devuelve_offset() {
        // FNV-1a sobre vacío devuelve el offset basis.
        let h = tarball_hash_short(b"");
        assert_eq!(h, "cbf29ce484222325");
    }

    // NOTA: que el código generado COMPILE como Rust válido se valida
    // en `tests/cli_e2e.rs` con un `cargo build` real sobre el output
    // de `gen_launcher_main_rs` (sub-paso 8.b.4/8.b.5).
}
