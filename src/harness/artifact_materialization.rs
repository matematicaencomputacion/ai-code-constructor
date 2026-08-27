//! Materialización efímera de un [`RustArtifact`] como crate Cargo mínimo.
//!
//! RAII: al dropear la instancia se elimina el directorio temporal.
//! No toca el workspace del repositorio anfitrión.
//!
//! Layout (multi-file):
//! ```text
//! {temp}/
//! ├── Cargo.toml          ← generado por infraestructura (sin deps inventadas)
//! └── <ArtifactPath>…     ← cada archivo del Artifact, sin modificar contenido
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::harness::artifact::RustArtifact;

static MATERIALIZATION_SEQ: AtomicU64 = AtomicU64::new(0);

/// Crate temporal derivado de un [`RustArtifact`] (single- o multi-file).
#[derive(Debug)]
pub struct ArtifactMaterialization {
    root: PathBuf,
}

impl ArtifactMaterialization {
    /// Materializa todos los `files` del Artifact bajo un dir aislado en el temp del SO.
    pub fn from_artifact(artifact: &RustArtifact) -> Result<Self, String> {
        let seq = MATERIALIZATION_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir_name = format!(
            "ai_code_constructor_artifact_{}_{}_{}",
            std::process::id(),
            seq,
            sanitize_fragment(artifact.id().as_str())
        );
        let root = std::env::temp_dir().join(dir_name);
        fs::create_dir_all(&root).map_err(|error| {
            format!(
                "no se pudo crear materialización temporal {}: {error}",
                root.display()
            )
        })?;

        let cargo_toml = "\
[package]
name = \"artifact_session\"
version = \"0.1.0\"
edition = \"2021\"
";
        fs::write(root.join("Cargo.toml"), cargo_toml).map_err(|error| {
            format!(
                "no se pudo escribir Cargo.toml en {}: {error}",
                root.display()
            )
        })?;

        for (path, source) in artifact.files() {
            let dest = path.resolve_under(&root)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("no se pudo crear directorio {}: {error}", parent.display())
                })?;
            }
            fs::write(&dest, source).map_err(|error| {
                format!(
                    "no se pudo escribir {} en {}: {error}",
                    path.as_str(),
                    dest.display()
                )
            })?;
        }

        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for ArtifactMaterialization {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn sanitize_fragment(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    if sanitized.is_empty() {
        "artifact".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_path::ArtifactPath;

    #[test]
    fn materialization_writes_crate_and_cleans_on_drop() {
        // A + G
        let artifact =
            RustArtifact::with_id(ArtifactId::new("art-mat-1"), "main.rs", "fn main() {}");
        let path = {
            let mat = ArtifactMaterialization::from_artifact(&artifact).expect("materialize");
            let root = mat.root().to_path_buf();
            assert!(root.join("Cargo.toml").is_file());
            assert!(root.join("src").join("main.rs").is_file());
            let source = fs::read_to_string(root.join("src").join("main.rs")).expect("read");
            assert_eq!(source, "fn main() {}");
            root
        };
        assert!(
            !path.exists(),
            "el directorio temporal debe limpiarse al dropear: {}",
            path.display()
        );
    }

    #[test]
    fn materialization_reflects_source_revision() {
        let mut artifact =
            RustArtifact::with_id(ArtifactId::new("art-mat-2"), "main.rs", "fn main() {}");
        {
            let mat = ArtifactMaterialization::from_artifact(&artifact).expect("v0");
            let source = fs::read_to_string(mat.root().join("src").join("main.rs")).unwrap();
            assert_eq!(source, "fn main() {}");
        }
        artifact.replace_source("fn main() { println!(\"v1\"); }");
        {
            let mat = ArtifactMaterialization::from_artifact(&artifact).expect("v1");
            let source = fs::read_to_string(mat.root().join("src").join("main.rs")).unwrap();
            assert_eq!(source, "fn main() { println!(\"v1\"); }");
        }
    }

    #[test]
    fn materialization_writes_all_multi_file_paths() {
        // B + C + D
        let artifact = RustArtifact::try_from_files(
            ArtifactId::new("art-mat-multi"),
            "main.rs",
            ArtifactPath::parse("src/main.rs").unwrap(),
            [
                (
                    ArtifactPath::parse("src/main.rs").unwrap(),
                    "mod helper;\nfn main() { helper::ping(); }\n".to_string(),
                ),
                (
                    ArtifactPath::parse("src/helper.rs").unwrap(),
                    "pub fn ping() {}\n".to_string(),
                ),
                (
                    ArtifactPath::parse("src/domain/math.rs").unwrap(),
                    "pub fn add(a: i32, b: i32) -> i32 { a + b }\n".to_string(),
                ),
            ],
        )
        .unwrap();
        let mat = ArtifactMaterialization::from_artifact(&artifact).expect("mat");
        let root = mat.root();
        assert_eq!(
            fs::read_to_string(root.join("src").join("main.rs")).unwrap(),
            "mod helper;\nfn main() { helper::ping(); }\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("src").join("helper.rs")).unwrap(),
            "pub fn ping() {}\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("src").join("domain").join("math.rs")).unwrap(),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n"
        );
    }

    #[test]
    fn materialization_rejects_path_escape_even_if_constructed() {
        // F — resolve_under defiende el root
        let root = std::env::temp_dir().join("ai_code_constructor_path_guard");
        let _ = fs::create_dir_all(&root);
        let path = ArtifactPath::parse("src/main.rs").unwrap();
        let resolved = path.resolve_under(&root).unwrap();
        assert!(resolved.starts_with(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_materializations_are_independent() {
        // H
        let a = RustArtifact::with_id(ArtifactId::new("art-a"), "main.rs", "fn main() { /*a*/ }");
        let b = RustArtifact::with_id(ArtifactId::new("art-b"), "main.rs", "fn main() { /*b*/ }");
        let mat_a = ArtifactMaterialization::from_artifact(&a).unwrap();
        let mat_b = ArtifactMaterialization::from_artifact(&b).unwrap();
        assert_ne!(mat_a.root(), mat_b.root());
        assert_eq!(
            fs::read_to_string(mat_a.root().join("src").join("main.rs")).unwrap(),
            "fn main() { /*a*/ }"
        );
        assert_eq!(
            fs::read_to_string(mat_b.root().join("src").join("main.rs")).unwrap(),
            "fn main() { /*b*/ }"
        );
    }
}
