//! Materialización efímera de un [`RustArtifact`] como crate Cargo mínimo.
//!
//! RAII: al dropear la instancia se elimina el directorio temporal.
//! No toca el workspace del repositorio anfitrión.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::harness::artifact::RustArtifact;

static MATERIALIZATION_SEQ: AtomicU64 = AtomicU64::new(0);

/// Crate temporal single-file derivado de un [`RustArtifact`].
///
/// Layout:
/// ```text
/// {temp}/
/// ├── Cargo.toml
/// └── src/
///     └── main.rs
/// ```
#[derive(Debug)]
pub struct ArtifactMaterialization {
    root: PathBuf,
}

impl ArtifactMaterialization {
    /// Materializa `artifact.source()` como `src/main.rs` en un dir aislado bajo el temp del SO.
    pub fn from_artifact(artifact: &RustArtifact) -> Result<Self, String> {
        let seq = MATERIALIZATION_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir_name = format!(
            "ai_code_constructor_artifact_{}_{}_{}",
            std::process::id(),
            seq,
            sanitize_fragment(artifact.id().as_str())
        );
        let root = std::env::temp_dir().join(dir_name);
        fs::create_dir_all(root.join("src")).map_err(|error| {
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
        fs::write(root.join("src").join("main.rs"), artifact.source()).map_err(|error| {
            format!(
                "no se pudo escribir src/main.rs en {}: {error}",
                root.display()
            )
        })?;

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
    use crate::harness::artifact::ArtifactId;

    #[test]
    fn materialization_writes_crate_and_cleans_on_drop() {
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
}
