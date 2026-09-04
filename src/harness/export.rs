//! Materialización cargo-buildable de un RustArtifact: fuentes + Cargo.toml mínimo.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::harness::artifact::RustArtifact;

pub const EXPORTED_EDITION: &str = "2021";
pub const EXPORTED_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub out_dir: PathBuf,
    pub package_name: String,
    pub file_count: usize,
    pub manifest_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    Sources(String),
    Manifest { path: PathBuf, error: String },
    InvalidPackageName(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sources(error) => {
                write!(f, "no se pudieron escribir las fuentes: {error}")
            }
            Self::Manifest { path, error } => {
                write!(f, "no se pudo escribir {}: {error}", path.display())
            }
            Self::InvalidPackageName(raw) => {
                write!(f, "nombre de paquete Cargo inválido: `{raw}`")
            }
        }
    }
}

pub fn export_artifact(
    artifact: &RustArtifact,
    out_dir: &Path,
) -> Result<ExportReport, ExportError> {
    artifact
        .export_to_dir(out_dir)
        .map_err(|error| ExportError::Sources(error.to_string()))?;
    let package_name = cargo_package_name(artifact.id().as_str())?;
    let manifest = render_manifest(&package_name, artifact);
    let manifest_path = out_dir.join("Cargo.toml");
    fs::write(&manifest_path, manifest).map_err(|error| ExportError::Manifest {
        path: manifest_path.clone(),
        error: error.to_string(),
    })?;
    Ok(ExportReport {
        out_dir: out_dir.to_path_buf(),
        package_name,
        file_count: artifact.file_count(),
        manifest_path,
    })
}

/// Nombre de paquete válido: [A-Za-z0-9_-], no empieza por dígito, no vacío.
pub fn cargo_package_name(raw: &str) -> Result<String, ExportError> {
    let mut name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    if name.trim_matches('_').is_empty() {
        return Err(ExportError::InvalidPackageName(raw.to_string()));
    }
    Ok(name)
}

pub fn render_manifest(package_name: &str, artifact: &RustArtifact) -> String {
    let mut out = format!(
        "[package]\nname = \"{package_name}\"\nversion = \"{EXPORTED_VERSION}\"\nedition = \"{EXPORTED_EDITION}\"\n\n[dependencies]\n"
    );
    let primary = artifact.primary_path().as_str();
    if primary != "src/main.rs" && primary != "src/lib.rs" {
        out.push_str(&format!(
            "\n[[bin]]\nname = \"{package_name}\"\npath = \"{primary}\"\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::harness::artifact::ArtifactId;
    use crate::harness::artifact_path::ArtifactPath;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_temp(label: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ai_code_constructor_export_{label}_{}_{seq}",
            std::process::id()
        ))
    }

    #[test]
    fn export_artifact_writes_cargo_toml_and_sources() {
        let artifact = RustArtifact::with_id(ArtifactId::new("demo"), "demo", "fn main() {}\n");
        let target = unique_temp("demo");
        let _ = fs::remove_dir_all(&target);

        let report = export_artifact(&artifact, &target).expect("export");
        assert_eq!(report.package_name, "demo");
        assert_eq!(report.file_count, 1);
        assert_eq!(report.out_dir, target);
        assert_eq!(report.manifest_path, target.join("Cargo.toml"));

        let manifest = fs::read_to_string(target.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("name = \"demo\""));
        assert!(manifest.contains("version = \"0.1.0\""));
        assert!(manifest.contains("edition = \"2021\""));
        assert!(manifest.contains("[dependencies]"));

        let main_rs = target.join("src/main.rs");
        assert!(main_rs.is_file());
        assert_eq!(fs::read_to_string(&main_rs).unwrap(), "fn main() {}\n");

        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn package_name_is_sanitized() {
        assert_eq!(cargo_package_name("main.rs").unwrap(), "main_rs");
        assert_eq!(cargo_package_name("3d").unwrap(), "_3d");
        assert_eq!(cargo_package_name("a:b").unwrap(), "a_b");
        assert!(matches!(
            cargo_package_name("..."),
            Err(ExportError::InvalidPackageName(raw)) if raw == "..."
        ));
    }

    #[test]
    fn manifest_adds_bin_for_unconventional_primary() {
        let conventional = RustArtifact::with_id(ArtifactId::new("demo"), "demo", "fn main() {}\n");
        let conventional_manifest = render_manifest("demo", &conventional);
        assert!(
            !conventional_manifest.contains("[[bin]]"),
            "src/main.rs no requiere [[bin]] explícito"
        );

        let primary = ArtifactPath::parse("src/bin/tool.rs").unwrap();
        let tool = RustArtifact::try_from_files(
            ArtifactId::new("tool"),
            "tool",
            primary,
            [(
                ArtifactPath::parse("src/bin/tool.rs").unwrap(),
                "fn main() {}".to_string(),
            )],
        )
        .unwrap();
        let tool_manifest = render_manifest("tool", &tool);
        assert!(tool_manifest.contains("[[bin]]"));
        assert!(tool_manifest.contains("name = \"tool\""));
        assert!(tool_manifest.contains("path = \"src/bin/tool.rs\""));
    }

    #[test]
    fn export_artifact_multi_file_keeps_nested_dirs() {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let artifact = RustArtifact::try_from_files(
            ArtifactId::new("art-export-multi"),
            "main.rs",
            main,
            [
                (
                    ArtifactPath::parse("src/main.rs").unwrap(),
                    "fn main() {}".to_string(),
                ),
                (
                    ArtifactPath::parse("src/lib.rs").unwrap(),
                    "pub fn run() {}".to_string(),
                ),
                (
                    ArtifactPath::parse("src/domain/math.rs").unwrap(),
                    "pub fn add(a: i32, b: i32) -> i32 { a + b }".to_string(),
                ),
            ],
        )
        .unwrap();

        let target = unique_temp("multi");
        let _ = fs::remove_dir_all(&target);
        let report = export_artifact(&artifact, &target).expect("multi export");
        assert_eq!(report.package_name, "art-export-multi");
        assert_eq!(report.file_count, 3);
        assert!(target.join("Cargo.toml").is_file());
        assert!(target.join("src/main.rs").is_file());
        assert!(target.join("src/lib.rs").is_file());
        assert!(
            target.join("src/domain/math.rs").is_file(),
            "debe crear subdirectorios anidados"
        );
        assert_eq!(
            fs::read_to_string(target.join("src/domain/math.rs")).unwrap(),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }"
        );

        let _ = fs::remove_dir_all(&target);
    }
}
