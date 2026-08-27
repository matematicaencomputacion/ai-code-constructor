use crate::harness::context::AgentContext;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::{CHECK_FORMAT, run_cargo_on_artifact};

/// Ejecuta `cargo fmt --check` sobre el [`crate::harness::RustArtifact`] materializado.
///
/// No usa el workspace del repositorio anfitrión.
pub struct FmtTool;

impl Tool for FmtTool {
    fn name(&self) -> &str {
        CHECK_FORMAT
    }

    fn execute(&self, _input: &str, ctx: &AgentContext) -> ToolResult {
        run_cargo_on_artifact(CHECK_FORMAT, ctx, |command, _root| {
            command.arg("fmt").arg("--check");
        })
    }
}
