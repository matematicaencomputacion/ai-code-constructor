use crate::harness::context::AgentContext;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::{RUN_CLIPPY, run_cargo_on_artifact};

/// Ejecuta `cargo clippy -- -D warnings` sobre el [`crate::harness::RustArtifact`] materializado.
///
/// No usa el workspace del repositorio anfitrión.
pub struct ClippyTool;

impl Tool for ClippyTool {
    fn name(&self) -> &str {
        RUN_CLIPPY
    }

    fn execute(&self, _input: &str, ctx: &AgentContext) -> ToolResult {
        run_cargo_on_artifact(RUN_CLIPPY, ctx, |command, _root| {
            command.arg("clippy").arg("--").arg("-D").arg("warnings");
        })
    }
}
