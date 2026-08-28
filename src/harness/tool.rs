use crate::harness::artifact::RustArtifact;
use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;

/// Resultado de ejecutar una herramienta, con evidencia estructurada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub evidence: Vec<Evidence>,
    /// Estado post-mutación validado por preview. El Harness hace el commit canónico una vez.
    pub artifact_preview: Option<RustArtifact>,
}

impl ToolResult {
    pub fn failure(output: impl Into<String>, evidence: Vec<Evidence>) -> Self {
        Self {
            success: false,
            output: output.into(),
            evidence,
            artifact_preview: None,
        }
    }

    pub fn success(output: impl Into<String>, evidence: Vec<Evidence>) -> Self {
        Self {
            success: true,
            output: output.into(),
            evidence,
            artifact_preview: None,
        }
    }

    pub fn with_artifact_preview(mut self, preview: RustArtifact) -> Self {
        self.artifact_preview = Some(preview);
        self
    }
}

/// Herramienta invocable por el Harness a partir de una [`crate::harness::AgentAction`].
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult;
}
