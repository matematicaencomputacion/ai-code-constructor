//! CLI Oleada 1: materializa un [`RustArtifact`] en un directorio de usuario.
//!
//! `export_artifact` escribe fuentes + `Cargo.toml` mínimo; esta unidad lo
//! expone como comando `export` sin añadir crates de parsing. El catálogo es
//! en memoria (MVP): no hay repositorio persistente de artefactos todavía.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::builder;
use crate::harness::artifact::{ArtifactId, RustArtifact};
use crate::harness::artifact_path::ArtifactPath;
use crate::harness::export::export_artifact;
use crate::planner::PlanKind;

/// Uso del subcomando `export`.
pub fn export_cli_usage() -> &'static str {
    "export [--force] [--artifact-id ID] --out <dir>\n       export [--force] <artifact_id> --out <dir>\n       export [--force] <artifact_id> <dir>            (forma legado)\n\nIDs conocidos: demo, api, calculator, authentication"
}

/// Opciones ya validadas del subcomando `export`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportCliOptions {
    pub artifact_id: String,
    pub output_path: PathBuf,
    pub force: bool,
}

/// Fallos controlados del CLI de export (sin panics en el camino feliz).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportCliError {
    MissingValue(&'static str),
    InvalidValue {
        flag: &'static str,
        expected: &'static str,
    },
    UnknownArgument(String),
    MissingArtifactId,
    MissingPath,
    TooManyArguments,
    UnknownArtifact(String),
    TargetIsFile(PathBuf),
    TargetNotEmpty(PathBuf),
    ExportFailed(String),
}

impl fmt::Display for ExportCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue(flag) => write!(f, "falta valor para {flag}"),
            Self::InvalidValue { flag, expected } => {
                write!(f, "valor inválido para {flag}; esperado {expected}")
            }
            Self::UnknownArgument(flag) => write!(f, "argumento desconocido: {flag}"),
            Self::MissingArtifactId => write!(f, "falta <artifact_id>"),
            Self::MissingPath => write!(f, "falta <path> de destino"),
            Self::TooManyArguments => write!(f, "demasiados argumentos posicionales"),
            Self::UnknownArtifact(id) => write!(
                f,
                "artifact `{id}` no encontrado; IDs conocidos: {}",
                known_artifact_ids().join(", ")
            ),
            Self::TargetIsFile(path) => {
                write!(
                    f,
                    "el destino {} es un archivo; se esperaba un directorio",
                    path.display()
                )
            }
            Self::TargetNotEmpty(path) => write!(
                f,
                "el directorio {} no está vacío; pasa --force para sobrescribir",
                path.display()
            ),
            Self::ExportFailed(error) => write!(f, "no se pudo exportar: {error}"),
        }
    }
}

/// IDs del catálogo MVP (Oleada 1, sin persistencia).
pub fn known_artifact_ids() -> Vec<&'static str> {
    vec!["demo", "api", "calculator", "authentication"]
}

/// Resuelve un artefacto de demostración por ID.
pub fn catalog_artifact(artifact_id: &str) -> Result<RustArtifact, ExportCliError> {
    let key = artifact_id.strip_prefix("artifact:").unwrap_or(artifact_id);
    let artifact = match key {
        "demo" => RustArtifact::with_id(ArtifactId::new("demo"), "demo", "fn main() {}\n"),
        "api" => artifact_for_kind("api", "api", PlanKind::Api),
        "calculator" => artifact_for_kind("calculator", "calculator", PlanKind::Calculator),
        "authentication" => {
            artifact_for_kind("authentication", "authentication", PlanKind::Authentication)
        }
        _ => return Err(ExportCliError::UnknownArtifact(artifact_id.to_string())),
    };
    Ok(artifact)
}

fn artifact_for_kind(id: &str, name: &str, kind: PlanKind) -> RustArtifact {
    let definition = builder::initial_artifact_definition_for_kind(kind);
    let artifact_id = ArtifactId::new(id);
    if definition.file_count() == 1 {
        return RustArtifact::with_id(artifact_id, name, builder::initial_source_for_kind(kind));
    }

    let primary = ArtifactPath::parse(definition.primary_path)
        .expect("primary path del Builder debe ser válido");
    let files: Vec<(ArtifactPath, String)> = definition
        .files()
        .map(|(path, source)| {
            (
                ArtifactPath::parse(path).expect("path del Builder debe ser válido"),
                source.to_string(),
            )
        })
        .collect();
    RustArtifact::try_from_files(artifact_id, name, primary, files)
        .expect("definición inicial del Builder debe ser válida")
}

/// Parsea `export` al estilo del probe: flags manuales, sin clap.
pub fn parse_export_cli_args<I, S>(args: I) -> Result<ExportCliOptions, ExportCliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut index = 0;
    let mut force = false;
    let mut artifact_id = None;
    let mut out_path = None;
    let mut positionals = Vec::new();

    while index < args.len() {
        match args[index].as_str() {
            "--force" => force = true,
            "--artifact-id" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or(ExportCliError::MissingValue("--artifact-id"))?;
                if value.trim().is_empty() || value.starts_with('-') {
                    return Err(ExportCliError::InvalidValue {
                        flag: "--artifact-id",
                        expected: "non-empty artifact id",
                    });
                }
                artifact_id = Some(value.clone());
            }
            "--out" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or(ExportCliError::MissingValue("--out"))?;
                if value.trim().is_empty() || value.starts_with('-') {
                    return Err(ExportCliError::InvalidValue {
                        flag: "--out",
                        expected: "non-empty output path",
                    });
                }
                out_path = Some(PathBuf::from(value));
            }
            other if other.starts_with('-') => {
                return Err(ExportCliError::UnknownArgument(other.to_string()));
            }
            other => positionals.push(other.to_string()),
        }
        index += 1;
    }

    let (id, path) = match (artifact_id, out_path, positionals.as_slice()) {
        (Some(id), Some(path), []) => (id, path),
        (Some(_), Some(_), _) => return Err(ExportCliError::TooManyArguments),
        (Some(id), None, [path]) => (id, PathBuf::from(path)),
        (Some(_), None, []) => return Err(ExportCliError::MissingPath),
        (Some(_), None, _) => return Err(ExportCliError::TooManyArguments),
        (None, Some(path), [id]) => (id.clone(), path),
        (None, Some(_), []) => return Err(ExportCliError::MissingArtifactId),
        (None, Some(_), _) => return Err(ExportCliError::TooManyArguments),
        (None, None, [id, path]) => (id.clone(), PathBuf::from(path)),
        (None, None, [_, _, ..]) => return Err(ExportCliError::TooManyArguments),
        (None, None, [_]) => return Err(ExportCliError::MissingPath),
        (None, None, []) => return Err(ExportCliError::MissingArtifactId),
    };

    if id.trim().is_empty() {
        return Err(ExportCliError::InvalidValue {
            flag: "artifact_id",
            expected: "non-empty artifact id",
        });
    }
    let path_text = path.to_string_lossy();
    if path_text.trim().is_empty() {
        return Err(ExportCliError::InvalidValue {
            flag: "path",
            expected: "non-empty output path",
        });
    }

    Ok(ExportCliOptions {
        artifact_id: id,
        output_path: path,
        force,
    })
}

/// Ejecuta el subcomando `export` y devuelve el mensaje de éxito.
pub fn run_export_cli<I, S>(args: I) -> Result<String, ExportCliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let options = parse_export_cli_args(args)?;
    let artifact = catalog_artifact(&options.artifact_id)?;
    prepare_target_dir(&options.output_path, options.force)?;
    let report = export_artifact(&artifact, &options.output_path)
        .map_err(|error| ExportCliError::ExportFailed(error.to_string()))?;

    Ok(format!(
        "Artifact `{}` (revision {}, {} file(s), contract v{}) exportado a {} como paquete `{}` (Cargo.toml + src/)",
        artifact.id().as_str(),
        artifact.revision(),
        artifact.file_count(),
        artifact.contract_version(),
        options.output_path.display(),
        report.package_name
    ))
}

fn prepare_target_dir(path: &Path, force: bool) -> Result<(), ExportCliError> {
    if path.is_file() {
        return Err(ExportCliError::TargetIsFile(path.to_path_buf()));
    }
    if !path.exists() {
        return Ok(());
    }
    if !force && dir_has_entries(path)? {
        return Err(ExportCliError::TargetNotEmpty(path.to_path_buf()));
    }
    Ok(())
}

fn dir_has_entries(path: &Path) -> Result<bool, ExportCliError> {
    let mut entries = fs::read_dir(path).map_err(|error| {
        ExportCliError::ExportFailed(format!("no se pudo leer {}: {error}", path.display()))
    })?;
    Ok(entries.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_temp(label: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ai_code_constructor_export_cli_{label}_{}_{seq}",
            std::process::id()
        ))
    }

    #[test]
    fn parse_positional_artifact_and_path() {
        let options = parse_export_cli_args(["demo", "/tmp/out"]).expect("parse");
        assert_eq!(options.artifact_id, "demo");
        assert_eq!(options.output_path, PathBuf::from("/tmp/out"));
        assert!(!options.force);
    }

    #[test]
    fn parse_force_and_artifact_id_flag() {
        let options =
            parse_export_cli_args(["--force", "--artifact-id", "api", "/tmp/api"]).expect("parse");
        assert_eq!(options.artifact_id, "api");
        assert_eq!(options.output_path, PathBuf::from("/tmp/api"));
        assert!(options.force);
    }

    #[test]
    fn parse_force_after_positionals() {
        let options = parse_export_cli_args(["demo", "/tmp/out", "--force"]).expect("parse");
        assert!(options.force);
        assert_eq!(options.artifact_id, "demo");
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        let error = parse_export_cli_args(["demo", "/tmp/out", "--json"]).unwrap_err();
        assert_eq!(error, ExportCliError::UnknownArgument("--json".to_string()));
    }

    #[test]
    fn parse_rejects_missing_args() {
        assert_eq!(
            parse_export_cli_args(Vec::<String>::new()).unwrap_err(),
            ExportCliError::MissingArtifactId
        );
        assert_eq!(
            parse_export_cli_args(["demo"]).unwrap_err(),
            ExportCliError::MissingPath
        );
        assert_eq!(
            parse_export_cli_args(["--artifact-id", "demo"]).unwrap_err(),
            ExportCliError::MissingPath
        );
        assert_eq!(
            parse_export_cli_args(["a", "b", "c"]).unwrap_err(),
            ExportCliError::TooManyArguments
        );
    }

    #[test]
    fn catalog_unknown_artifact_lists_known_ids() {
        let error = catalog_artifact("missing").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("missing"));
        assert!(text.contains("demo"));
        assert!(text.contains("authentication"));
    }

    #[test]
    fn catalog_accepts_artifact_prefix() {
        let artifact = catalog_artifact("artifact:demo").expect("demo");
        assert_eq!(artifact.id().as_str(), "demo");
        assert_eq!(artifact.source(), "fn main() {}\n");
    }

    #[test]
    fn run_export_writes_demo_files() {
        let target = unique_temp("demo");
        let _ = fs::remove_dir_all(&target);

        let output = run_export_cli(["demo", target.to_str().expect("utf8")]).expect("export");
        assert!(output.contains("Artifact `demo`"));
        assert!(output.contains(target.to_str().expect("utf8")));

        let main_rs = target.join("src/main.rs");
        assert!(main_rs.is_file());
        assert_eq!(fs::read_to_string(&main_rs).unwrap(), "fn main() {}\n");
        assert!(target.join("Cargo.toml").is_file());
        assert!(output.contains("paquete `demo`"));
        assert!(output.contains("Cargo.toml"));

        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn run_export_writes_authentication_siblings() {
        let target = unique_temp("auth");
        let _ = fs::remove_dir_all(&target);

        run_export_cli(["authentication", target.to_str().expect("utf8")]).expect("export");
        assert!(target.join("src/main.rs").is_file());
        assert!(target.join("src/auth.rs").is_file());
        assert!(target.join("Cargo.toml").is_file());
        let auth = fs::read_to_string(target.join("src/auth.rs")).unwrap();
        assert!(auth.contains("validar_credenciales"));

        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn run_export_rejects_nonempty_without_force() {
        let target = unique_temp("nonempty");
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), "stale").unwrap();

        let error = run_export_cli(["demo", target.to_str().expect("utf8")]).unwrap_err();
        assert!(matches!(error, ExportCliError::TargetNotEmpty(_)));
        assert_eq!(
            fs::read_to_string(target.join("keep.txt")).unwrap(),
            "stale"
        );

        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn run_export_force_overwrites_existing_dir() {
        let target = unique_temp("force");
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), "stale").unwrap();

        run_export_cli(["--force", "demo", target.to_str().expect("utf8")]).expect("export");
        assert!(target.join("src/main.rs").is_file());
        assert!(target.join("keep.txt").is_file(), "force no borra extras");

        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn run_export_rejects_file_target() {
        let target = unique_temp("file");
        let _ = fs::remove_file(&target);
        fs::write(&target, "not a dir").unwrap();

        let error = run_export_cli(["demo", target.to_str().expect("utf8")]).unwrap_err();
        assert!(matches!(error, ExportCliError::TargetIsFile(_)));

        let _ = fs::remove_file(&target);
    }

    #[test]
    fn usage_mentions_force_and_known_ids() {
        let usage = export_cli_usage();
        assert!(usage.contains("--force"));
        assert!(usage.contains("authentication"));
    }

    #[test]
    fn usage_mentions_out_flag() {
        let usage = export_cli_usage();
        assert!(usage.contains("--out <dir>"));
        assert!(usage.contains("forma legado"));
    }

    #[test]
    fn parse_out_flag_with_positional_id() {
        let options = parse_export_cli_args(["demo", "--out", "/tmp/o"]).expect("parse");
        assert_eq!(options.artifact_id, "demo");
        assert_eq!(options.output_path, PathBuf::from("/tmp/o"));
        assert!(!options.force);
    }

    #[test]
    fn parse_out_flag_with_artifact_id_flag() {
        let options = parse_export_cli_args(["--artifact-id", "api", "--out", "/tmp/a", "--force"])
            .expect("parse");
        assert_eq!(options.artifact_id, "api");
        assert_eq!(options.output_path, PathBuf::from("/tmp/a"));
        assert!(options.force);
    }

    #[test]
    fn parse_out_missing_value() {
        assert_eq!(
            parse_export_cli_args(["demo", "--out"]).unwrap_err(),
            ExportCliError::MissingValue("--out")
        );
        assert_eq!(
            parse_export_cli_args(["demo", "--out", "--force"]).unwrap_err(),
            ExportCliError::InvalidValue {
                flag: "--out",
                expected: "non-empty output path",
            }
        );
    }

    #[test]
    fn parse_out_and_positional_path_conflict() {
        let error = parse_export_cli_args(["demo", "/tmp/x", "--out", "/tmp/y"]).unwrap_err();
        assert_eq!(error, ExportCliError::TooManyArguments);
    }
}
