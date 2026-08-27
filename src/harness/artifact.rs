//! Snapshot controlado del artefacto Rust sobre el que trabajará el Agent.
//!
//! [`RustArtifact`] es el objeto de dominio RESULT / WORKING PRODUCT.
//! Permanece separado de [`crate::state::CodeState`].

use crate::harness::specification::SpecificationId;

/// Versión del contrato de Artifact (evolución futura v1 → v2).
pub type ArtifactContractVersion = u32;

/// Versión activa del contrato implementado en esta unidad.
pub const ARTIFACT_CONTRACT_VERSION: ArtifactContractVersion = 1;

/// Identidad estable del Artifact (no deriva del contenido del source).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lenguaje del Artifact (extensible; hoy solo Rust).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactLanguage {
    Rust,
}

impl ArtifactLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
        }
    }
}

/// Artefacto Rust versionado: identidad estable + source revisable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustArtifact {
    id: ArtifactId,
    name: String,
    source: String,
    language: ArtifactLanguage,
    contract_version: ArtifactContractVersion,
    revision: u64,
    /// Enlace opcional Specification → Artifact (trazabilidad en memoria).
    specification_id: Option<SpecificationId>,
}

impl RustArtifact {
    /// Crea un artefacto con `revision == 0` e identidad derivada del nombre (no del source).
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        let name = name.into();
        Self::with_id(ArtifactId::new(format!("artifact:{name}")), name, source)
    }

    /// Crea un artefacto con identidad explícita.
    pub fn with_id(id: ArtifactId, name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            source: source.into(),
            language: ArtifactLanguage::Rust,
            contract_version: ARTIFACT_CONTRACT_VERSION,
            revision: 0,
            specification_id: None,
        }
    }

    pub fn id(&self) -> &ArtifactId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn language(&self) -> ArtifactLanguage {
        self.language
    }

    pub fn contract_version(&self) -> ArtifactContractVersion {
        self.contract_version
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn specification_id(&self) -> Option<&SpecificationId> {
        self.specification_id.as_ref()
    }

    pub fn with_specification_id(mut self, specification_id: SpecificationId) -> Self {
        self.specification_id = Some(specification_id);
        self
    }

    /// Reemplaza el source e incrementa `revision` en 1 solo si el contenido cambia.
    /// No modifica [`ArtifactId`].
    pub fn replace_source(&mut self, new_source: impl Into<String>) {
        let next = new_source.into();
        if next != self.source {
            self.source = next;
            self.revision += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder;
    use crate::planner;
    use crate::state::CodeState;

    fn build_valid_api_state() -> CodeState {
        let mut state = CodeState {
            request: "Crear una API REST".to_string(),
            plan: None,
            code: None,
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        };
        planner::plan(&mut state);
        state.iteration = 2;
        builder::build(&mut state);
        state
    }

    #[test]
    fn artifact_starts_at_revision_zero() {
        let artifact = RustArtifact::new("main.rs", "fn main() {}");
        assert_eq!(artifact.revision(), 0);
        assert_eq!(artifact.name(), "main.rs");
        assert_eq!(artifact.language(), ArtifactLanguage::Rust);
        assert_eq!(artifact.contract_version(), ARTIFACT_CONTRACT_VERSION);
    }

    #[test]
    fn artifact_returns_source() {
        let artifact = RustArtifact::new("main.rs", "fn main() {}");
        assert_eq!(artifact.source(), "fn main() {}");
        assert_eq!(artifact.revision(), 0);
    }

    #[test]
    fn artifact_has_stable_identity() {
        // A
        let artifact =
            RustArtifact::with_id(ArtifactId::new("art-stable"), "main.rs", "fn main() {}");
        assert_eq!(artifact.id().as_str(), "art-stable");
    }

    #[test]
    fn changing_source_does_not_change_artifact_id() {
        // B
        let mut artifact =
            RustArtifact::with_id(ArtifactId::new("art-stable"), "main.rs", "fn main() {}");
        let before = artifact.id().clone();
        artifact.replace_source("fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.id(), &before);
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn artifact_replace_source_updates_source_and_revision() {
        let mut artifact = RustArtifact::new("main.rs", "fn main() {}");
        artifact.replace_source("fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.source(), "fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn artifact_multiple_replacements_increment_revision() {
        let mut artifact = RustArtifact::new("main.rs", "fn main() {}");
        assert_eq!(artifact.revision(), 0);

        artifact.replace_source("fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.revision(), 1);

        artifact.replace_source("fn main() { println!(\"v2\"); }");
        assert_eq!(artifact.revision(), 2);

        artifact.replace_source("fn main() { println!(\"v2\"); }");
        assert_eq!(artifact.revision(), 2);
    }

    #[test]
    fn artifact_links_optional_specification_id() {
        let artifact = RustArtifact::new("main.rs", "fn main() {}")
            .with_specification_id(SpecificationId::new("spec-1"));
        assert_eq!(
            artifact.specification_id().map(|id| id.as_str()),
            Some("spec-1")
        );
    }

    #[test]
    fn artifact_snapshot_does_not_mutate_original_state() {
        let state = build_valid_api_state();
        let original_code = state.code.clone().expect("código del Constructor");
        let original_iteration = state.iteration;

        let mut artifact = RustArtifact::new("main.rs", original_code.clone());

        assert_eq!(artifact.revision(), 0);
        assert_eq!(artifact.source(), original_code);

        artifact.replace_source("fn main() { println!(\"harness-owned\"); }");

        assert_eq!(state.code.as_deref(), Some(original_code.as_str()));
        assert_eq!(state.iteration, original_iteration);
        assert_ne!(artifact.source(), original_code);
        assert_eq!(artifact.revision(), 1);
    }
}
