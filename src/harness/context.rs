use std::path::PathBuf;

use crate::harness::artifact::RustArtifact;
use crate::harness::evaluation::Evidence;
use crate::harness::observation::AgentObservation;
use crate::harness::specification::Specification;

/// Contexto observable que el agente y el Harness comparten durante la ejecución.
///
/// [`RustArtifact`] es la fuente canónica del código de trabajo.
/// `working_code()` es un accessor de compatibilidad derivado del Artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContext {
    pub goal: String,
    /// Historial textual de bajo nivel (compatibilidad / debugging).
    pub observations: Vec<String>,
    /// Última observación estructurada (entrada causal del Agent).
    pub last_observation: Option<AgentObservation>,
    /// Historial estructurado de observaciones.
    pub observation_history: Vec<AgentObservation>,
    pub step: u32,
    /// Directorio de trabajo para Tools basadas en `cargo` (opcional).
    pub workspace_dir: Option<PathBuf>,
    /// Artefacto de trabajo canónico (`CorrectionTarget::SessionCode`).
    pub working_artifact: Option<RustArtifact>,
    /// Specification opcional para Evaluation tras Tools (AgentLoop orquesta).
    pub evaluation_specification: Option<Specification>,
}

impl AgentContext {
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            observations: Vec::new(),
            last_observation: None,
            observation_history: Vec::new(),
            step: 0,
            workspace_dir: None,
            working_artifact: None,
            evaluation_specification: None,
        }
    }

    /// Vista derivada del source del Artifact (compatibilidad temporal).
    pub fn working_code(&self) -> Option<&str> {
        self.working_artifact.as_ref().map(RustArtifact::source)
    }

    /// Compatibilidad: crea un [`RustArtifact`] canónico a partir de un String.
    pub fn with_working_code(mut self, code: impl Into<String>) -> Self {
        self.set_working_artifact(RustArtifact::new("main.rs", code));
        self
    }

    pub fn with_working_artifact(mut self, artifact: RustArtifact) -> Self {
        self.set_working_artifact(artifact);
        self
    }

    pub fn with_workspace(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = Some(dir.into());
        self
    }

    pub fn with_evaluation_specification(mut self, specification: Specification) -> Self {
        self.evaluation_specification = Some(specification);
        self
    }

    /// Establece el Artifact canónico (única fuente de verdad del source).
    pub fn set_working_artifact(&mut self, artifact: RustArtifact) {
        self.working_artifact = Some(artifact);
    }

    /// Actualiza el source del Artifact existente; crea uno si aún no hay.
    /// Conserva [`crate::harness::ArtifactId`] cuando el Artifact ya existe.
    pub fn update_working_source(&mut self, source: impl Into<String>) {
        match &mut self.working_artifact {
            Some(artifact) => artifact.replace_source(source),
            None => self.set_working_artifact(RustArtifact::new("main.rs", source)),
        }
    }

    /// Añade Evidence de trazabilidad `artifact_id` cuando hay Artifact de sesión.
    pub fn append_artifact_evidence(&self, evidence: &mut Vec<Evidence>) {
        if let Some(artifact) = &self.working_artifact {
            evidence.push(
                Evidence::new("artifact_id", artifact.id().as_str())
                    .with_artifact_id(artifact.id().clone()),
            );
        }
    }

    pub fn record(&mut self, observation: impl Into<String>) {
        self.observations.push(observation.into());
    }

    pub fn push_observation(&mut self, observation: AgentObservation) {
        self.record(observation.summary());
        self.last_observation = Some(observation.clone());
        self.observation_history.push(observation);
    }

    pub fn workspace(&self) -> PathBuf {
        self.workspace_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::specification::SpecificationId;

    #[test]
    fn agent_context_stores_rust_artifact() {
        // C
        let artifact = RustArtifact::with_id(ArtifactId::new("art-1"), "main.rs", "fn main() {}");
        let ctx = AgentContext::new("goal").with_working_artifact(artifact.clone());
        assert_eq!(
            ctx.working_artifact.as_ref().map(|a| a.id().as_str()),
            Some("art-1")
        );
        assert_eq!(ctx.working_code(), Some("fn main() {}"));
    }

    #[test]
    fn single_canonical_source_via_artifact() {
        // D
        let mut ctx = AgentContext::new("goal").with_working_code("fn main() {}");
        assert_eq!(ctx.working_code(), Some("fn main() {}"));
        assert_eq!(
            ctx.working_artifact.as_ref().map(RustArtifact::source),
            ctx.working_code()
        );
        let id_before = ctx.working_artifact.as_ref().unwrap().id().clone();
        ctx.update_working_source("fn main() { let x = 1; }");
        assert_eq!(ctx.working_code(), Some("fn main() { let x = 1; }"));
        assert_eq!(
            ctx.working_artifact.as_ref().map(RustArtifact::source),
            ctx.working_code()
        );
        assert_eq!(ctx.working_artifact.as_ref().unwrap().id(), &id_before);
    }

    #[test]
    fn specification_to_artifact_is_traceable() {
        // K
        let artifact =
            RustArtifact::with_id(ArtifactId::new("art-trace"), "main.rs", "fn main() {}")
                .with_specification_id(SpecificationId::new("spec-trace"));
        let ctx = AgentContext::new("goal").with_working_artifact(artifact);
        assert_eq!(
            ctx.working_artifact
                .as_ref()
                .and_then(|a| a.specification_id())
                .map(|id| id.as_str()),
            Some("spec-trace")
        );
        assert_eq!(
            ctx.working_artifact.as_ref().map(|a| a.id().as_str()),
            Some("art-trace")
        );
    }
}
