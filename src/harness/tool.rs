use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;

/// Resultado de ejecutar una herramienta, con evidencia estructurada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub evidence: Vec<Evidence>,
}

/// Herramienta invocable por el Harness a partir de una [`crate::harness::AgentAction`].
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult;
}
