// deploy.rs — Phase 12.6 (`fitz deploy` orchestrator)
//
// The `fitz deploy <target>` sub-command runs the deployment according
// to the selected target. MVP targets: `docker` (build + push) and
// `compose` (local up). Future extensible targets: `fly`, `railway`,
// `k8s`.
//
// Model: thin wrapper. We do NOT replicate docker logic; we just invoke
// the corresponding CLIs (`docker build/push`, `docker compose up`)
// with args derived from the project's manifest. If the user needs
// advanced flags we do not expose (multi-arch, --no-cache, etc.), they
// can run docker/compose directly.
//
// AST-only detection (parallel to `fitz docker init`): we read the
// manifest entry point to verify it exists + to validate that the
// project is ready (Dockerfile/compose.yml). No codegen parity because
// deploy does NOT emit Rust code — it just invokes external tools.

use crate::manifest::Manifest;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Deploy target. MVP: docker + compose. Extensible targets documented
/// as visible debt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployTarget {
    /// `docker build -t <pkg.name>:<tag> . && docker push <pkg.name>:<tag>`
    Docker,
    /// `docker compose up -d --build`
    Compose,
}

impl fmt::Display for DeployTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeployTarget::Docker => write!(f, "docker"),
            DeployTarget::Compose => write!(f, "compose"),
        }
    }
}

impl DeployTarget {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "docker" => Some(DeployTarget::Docker),
            "compose" | "docker-compose" => Some(DeployTarget::Compose),
            _ => None,
        }
    }

    pub fn known_values() -> &'static [&'static str] {
        &["docker", "compose"]
    }
}

/// Deploy options. Some are target-specific:
/// - `tag` applies only to `docker` (override of the default
///   `<pkg>:latest`).
/// - `push` applies only to `docker` (skip push if false — useful for
///   local builds without a registry).
/// - `detach` applies only to `compose` (default true — `up -d`).
/// - `build` applies only to `compose` (default true — `--build`).
#[derive(Debug, Clone, Default)]
pub struct DeployOptions {
    pub tag: Option<String>,
    pub no_push: bool,
    pub no_detach: bool,
    pub no_build: bool,
}

/// Deploy result: executed target + invoked command(s) + exit codes.
/// Useful for logging and tests.
#[derive(Debug, Clone)]
pub struct DeployResult {
    pub target: DeployTarget,
    pub commands: Vec<DeployCommand>,
}

#[derive(Debug, Clone)]
pub struct DeployCommand {
    pub bin: String,
    pub args: Vec<String>,
    pub exit_code: i32,
}

/// Deploy error: failed pre-flight checks or failed invocation of an
/// external CLI.
#[derive(Debug)]
pub enum DeployError {
    /// The project does not have a `Dockerfile` that the deploy
    /// requires (target = `docker` or `compose`).
    MissingDockerfile { manifest_dir: PathBuf },
    /// The project does not have `docker-compose.yml` (target =
    /// `compose`).
    MissingComposeFile { manifest_dir: PathBuf },
    /// The `docker` binary is not in the system PATH.
    DockerNotInstalled,
    /// An external command failed with exit code != 0. The stderr was
    /// already dumped by the child process (we inherit stdio).
    CommandFailed {
        bin: String,
        args: Vec<String>,
        exit_code: i32,
    },
    /// IO error invoking a sub-process.
    Io(std::io::Error),
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeployError::MissingDockerfile { manifest_dir } => {
                write!(
                    f,
                    "no se encontró `Dockerfile` en `{}`. Corré `fitz docker init` para \
                     generarlo (Fase 12.4) antes de deployar.",
                    manifest_dir.display()
                )
            }
            DeployError::MissingComposeFile { manifest_dir } => {
                write!(
                    f,
                    "no se encontró `docker-compose.yml` en `{}`. Corré `fitz docker init` \
                     para generarlo (Fase 12.4) antes de deployar.",
                    manifest_dir.display()
                )
            }
            DeployError::DockerNotInstalled => {
                write!(
                    f,
                    "el binario `docker` no está en el PATH. Instalalo desde \
                     https://docs.docker.com/get-docker/ antes de deployar."
                )
            }
            DeployError::CommandFailed {
                bin,
                args,
                exit_code,
            } => {
                write!(
                    f,
                    "`{} {}` falló con exit code {}",
                    bin,
                    args.join(" "),
                    exit_code
                )
            }
            DeployError::Io(e) => write!(f, "IO error invocando sub-proceso: {}", e),
        }
    }
}

/// Runs the deploy for the selected target. Pre-flight validation,
/// invocation of the external commands, and capture of exit codes.
///
/// `manifest_dir` is the project root (where `fitz.toml` lives). The
/// commands run with `current_dir(manifest_dir)`.
pub fn run_deploy(
    target: DeployTarget,
    manifest: &Manifest,
    manifest_dir: &Path,
    options: &DeployOptions,
) -> Result<DeployResult, DeployError> {
    // Pre-flight: check that `docker` is installed.
    if !docker_available() {
        return Err(DeployError::DockerNotInstalled);
    }

    match target {
        DeployTarget::Docker => run_docker_deploy(manifest, manifest_dir, options),
        DeployTarget::Compose => run_compose_deploy(manifest_dir, options),
    }
}

/// Checks if the `docker` binary is in the PATH by running
/// `docker --version` and validating exit code 0. More reliable than
/// a cross-platform `which docker`.
fn docker_available() -> bool {
    Command::new("docker")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Target `docker`: `docker build -t <tag> .` + (optional) `docker
/// push <tag>`. Default tag = `<package.name>:latest`. Override with
/// `options.tag`. Skip push with `--no-push`.
fn run_docker_deploy(
    manifest: &Manifest,
    manifest_dir: &Path,
    options: &DeployOptions,
) -> Result<DeployResult, DeployError> {
    let dockerfile = manifest_dir.join("Dockerfile");
    if !dockerfile.is_file() {
        return Err(DeployError::MissingDockerfile {
            manifest_dir: manifest_dir.to_path_buf(),
        });
    }

    let tag = options
        .tag
        .clone()
        .unwrap_or_else(|| format!("{}:latest", manifest.package.name));

    let mut commands = Vec::new();

    // 1) docker build -t <tag> .
    let build_args = vec![
        "build".to_string(),
        "-t".to_string(),
        tag.clone(),
        ".".to_string(),
    ];
    let build_status = invoke_command("docker", &build_args, manifest_dir)?;
    commands.push(DeployCommand {
        bin: "docker".to_string(),
        args: build_args.clone(),
        exit_code: build_status,
    });
    if build_status != 0 {
        return Err(DeployError::CommandFailed {
            bin: "docker".to_string(),
            args: build_args,
            exit_code: build_status,
        });
    }

    // 2) docker push <tag> (optional)
    if !options.no_push {
        let push_args = vec!["push".to_string(), tag.clone()];
        let push_status = invoke_command("docker", &push_args, manifest_dir)?;
        commands.push(DeployCommand {
            bin: "docker".to_string(),
            args: push_args.clone(),
            exit_code: push_status,
        });
        if push_status != 0 {
            return Err(DeployError::CommandFailed {
                bin: "docker".to_string(),
                args: push_args,
                exit_code: push_status,
            });
        }
    }

    Ok(DeployResult {
        target: DeployTarget::Docker,
        commands,
    })
}

/// Target `compose`: `docker compose up -d --build`. Flag
/// `--no-detach` removes the `-d`; `--no-build` removes the `--build`.
fn run_compose_deploy(
    manifest_dir: &Path,
    options: &DeployOptions,
) -> Result<DeployResult, DeployError> {
    let compose_file = manifest_dir.join("docker-compose.yml");
    if !compose_file.is_file() {
        return Err(DeployError::MissingComposeFile {
            manifest_dir: manifest_dir.to_path_buf(),
        });
    }

    let mut args = vec!["compose".to_string(), "up".to_string()];
    if !options.no_detach {
        args.push("-d".to_string());
    }
    if !options.no_build {
        args.push("--build".to_string());
    }

    let status = invoke_command("docker", &args, manifest_dir)?;
    let cmd = DeployCommand {
        bin: "docker".to_string(),
        args: args.clone(),
        exit_code: status,
    };
    if status != 0 {
        return Err(DeployError::CommandFailed {
            bin: "docker".to_string(),
            args,
            exit_code: status,
        });
    }

    Ok(DeployResult {
        target: DeployTarget::Compose,
        commands: vec![cmd],
    })
}

/// Invokes a sub-process with inherited stdio (the user sees output
/// in real time). Returns the exit code; any IO error propagates as
/// `DeployError::Io`.
fn invoke_command(bin: &str, args: &[String], cwd: &Path) -> Result<i32, DeployError> {
    let status = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(DeployError::Io)?;
    Ok(status.code().unwrap_or(-1))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Bin, Manifest, Package};
    use tempfile::tempdir;

    fn dummy_manifest(name: &str) -> Manifest {
        Manifest {
            package: Package {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                edition: "2026".to_string(),
                description: None,
                license: None,
                authors: Vec::new(),
            },
            bin: Some(Bin {
                main: "src/main.fitz".to_string(),
            }),
            lib: None,
            dependencies: Default::default(),
            flags: Default::default(),
        }
    }

    #[test]
    fn deploy_target_from_str_parses_aliases_and_lowercase() {
        assert_eq!(DeployTarget::parse("docker"), Some(DeployTarget::Docker));
        assert_eq!(DeployTarget::parse("Docker"), Some(DeployTarget::Docker));
        assert_eq!(DeployTarget::parse("compose"), Some(DeployTarget::Compose));
        assert_eq!(
            DeployTarget::parse("docker-compose"),
            Some(DeployTarget::Compose)
        );
        assert_eq!(DeployTarget::parse("k8s"), None);
        assert_eq!(DeployTarget::parse("fly"), None);
    }

    #[test]
    fn deploy_target_display_matches_canonical_name() {
        assert_eq!(DeployTarget::Docker.to_string(), "docker");
        assert_eq!(DeployTarget::Compose.to_string(), "compose");
    }

    #[test]
    fn run_docker_deploy_without_dockerfile_is_error() {
        let dir = tempdir().unwrap();
        let manifest = dummy_manifest("demo");
        let options = DeployOptions::default();
        let result = run_docker_deploy(&manifest, dir.path(), &options);
        match result {
            Err(DeployError::MissingDockerfile { manifest_dir }) => {
                assert_eq!(manifest_dir, dir.path());
            }
            other => panic!("esperaba MissingDockerfile, recibí {:?}", other.err()),
        }
    }

    #[test]
    fn run_compose_deploy_without_compose_file_is_error() {
        let dir = tempdir().unwrap();
        let options = DeployOptions::default();
        let result = run_compose_deploy(dir.path(), &options);
        match result {
            Err(DeployError::MissingComposeFile { manifest_dir }) => {
                assert_eq!(manifest_dir, dir.path());
            }
            other => panic!("esperaba MissingComposeFile, recibí {:?}", other.err()),
        }
    }

    #[test]
    fn deploy_error_messages_are_actionable() {
        let dir = std::env::temp_dir();
        let e = DeployError::MissingDockerfile {
            manifest_dir: dir.clone(),
        };
        let msg = format!("{}", e);
        assert!(msg.contains("Dockerfile"));
        assert!(msg.contains("fitz docker init"));

        let e2 = DeployError::MissingComposeFile { manifest_dir: dir };
        let msg2 = format!("{}", e2);
        assert!(msg2.contains("docker-compose.yml"));
        assert!(msg2.contains("fitz docker init"));

        let e3 = DeployError::DockerNotInstalled;
        let msg3 = format!("{}", e3);
        assert!(msg3.contains("docker") && msg3.contains("PATH"));
    }

    #[test]
    fn deploy_options_default_no_push_false_no_detach_false_no_build_false() {
        let opts = DeployOptions::default();
        assert!(opts.tag.is_none());
        assert!(!opts.no_push);
        assert!(!opts.no_detach);
        assert!(!opts.no_build);
    }

    #[test]
    fn known_values_lists_docker_and_compose() {
        let values = DeployTarget::known_values();
        assert!(values.contains(&"docker"));
        assert!(values.contains(&"compose"));
        assert_eq!(values.len(), 2);
    }
}
