//! Preview / commit atómico para mutaciones de [`RustArtifact`].
//!
//! Contrato:
//! - **Preview**: valida y calcula el estado resultante sin mutar el artifact canónico.
//! - **Commit**: aplica el preview validado una sola vez (+1 `revision` si hay diff real).
//!
//! Las Tools producen preview + Evidence; el Harness realiza el commit canónico.

use crate::harness::artifact::RustArtifact;

/// `true` si archivos y primary coinciden entre dos snapshots.
pub fn artifact_files_unchanged(a: &RustArtifact, b: &RustArtifact) -> bool {
    a.files_snapshot() == b.files_snapshot() && a.primary_path() == b.primary_path()
}

/// Aplica un preview ya validado al artifact canónico.
///
/// Retorna `Ok(true)` si hubo cambio real (+1 revision), `Ok(false)` si era no-op.
pub fn commit_artifact_preview(
    artifact: &mut RustArtifact,
    preview: RustArtifact,
) -> Result<bool, String> {
    if artifact_files_unchanged(artifact, &preview) {
        return Ok(false);
    }
    artifact.commit_files_state(preview);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::artifact::{ArtifactId, RustArtifact};

    #[test]
    fn commit_preview_increments_revision_once() {
        let mut artifact =
            RustArtifact::with_id(ArtifactId::new("art-mut"), "main.rs", "fn main() {}");
        let mut preview = artifact.clone();
        preview.replace_source("fn main() { println!(\"v1\"); }");
        assert!(commit_artifact_preview(&mut artifact, preview).unwrap());
        assert_eq!(artifact.revision(), 1);
        assert_eq!(artifact.source(), "fn main() { println!(\"v1\"); }");
    }

    #[test]
    fn commit_identical_preview_is_noop() {
        let mut artifact =
            RustArtifact::with_id(ArtifactId::new("art-mut-2"), "main.rs", "fn main() {}");
        let preview = artifact.clone();
        assert!(!commit_artifact_preview(&mut artifact, preview).unwrap());
        assert_eq!(artifact.revision(), 0);
    }
}
