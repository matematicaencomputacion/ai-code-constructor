use crate::harness::context::AgentContext;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::{RUN_TESTS, run_cargo_on_artifact};

/// Ejecuta `cargo test` sobre la materialización del [`crate::harness::RustArtifact`] de sesión.
///
/// El `input` es un filtro opcional de tests (pasado a `cargo test`).
/// No usa el workspace del repositorio anfitrión.
pub struct TestTool;

impl Tool for TestTool {
    fn name(&self) -> &str {
        RUN_TESTS
    }

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult {
        let filter = input.trim().to_string();
        run_cargo_on_artifact(RUN_TESTS, ctx, move |command, _root| {
            command.arg("test");
            if !filter.is_empty() {
                command.arg(&filter);
            }
            command.arg("--").arg("--nocapture");
        })
    }
}
