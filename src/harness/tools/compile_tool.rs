use crate::compiler;
use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::COMPILE;

/// Adaptador de [`compiler::compile`] como Tool del Harness.
///
/// Consume el source del [`crate::harness::RustArtifact`] de sesión cuando el input
/// está vacío; si hay input, compila ese source y conserva trazabilidad al Artifact.
pub struct CompileTool;

impl Tool for CompileTool {
    fn name(&self) -> &str {
        COMPILE
    }

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult {
        let source = if input.is_empty() {
            ctx.working_code().unwrap_or("")
        } else {
            input
        };

        let mut result = match compiler::compile(source) {
            Ok(()) => ToolResult {
                success: true,
                output: "compilación exitosa".to_string(),
                evidence: vec![
                    Evidence::new("tool", COMPILE),
                    Evidence::new("compile_status", "ok"),
                    Evidence::new("code_bytes", source.len().to_string()),
                ],
            },
            Err(error) => ToolResult {
                success: false,
                output: error.clone(),
                evidence: vec![
                    Evidence::new("tool", COMPILE),
                    Evidence::new("compile_status", "error"),
                    Evidence::new("compiler_stderr", error),
                ],
            },
        };
        ctx.append_artifact_evidence(&mut result.evidence);
        result
    }
}
