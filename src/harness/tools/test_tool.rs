use crate::harness::context::AgentContext;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::{RUN_TESTS, tool_result_from_output};
use std::process::Command;

/// Ejecuta `cargo test` de forma controlada.
///
/// El `input` se interpreta como filtro opcional de tests (pasado a `cargo test`).
/// Usar un filtro acotado evita re-ejecutar toda la suite de forma recursiva.
pub struct TestTool;

impl Tool for TestTool {
    fn name(&self) -> &str {
        RUN_TESTS
    }

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult {
        let workspace = ctx.workspace();
        let mut command = Command::new("cargo");
        command.arg("test").current_dir(&workspace);

        let filter = input.trim();
        if !filter.is_empty() {
            command.arg(filter);
        }

        // Evita capturar salida interactiva y reduce ruido en evidencia.
        command.arg("--").arg("--nocapture");

        match command.output() {
            Ok(output) => tool_result_from_output(RUN_TESTS, output),
            Err(error) => ToolResult {
                success: false,
                output: format!("No se pudo ejecutar cargo test: {error}"),
                evidence: vec![
                    crate::harness::evaluation::Evidence::new("tool", RUN_TESTS),
                    crate::harness::evaluation::Evidence::new("spawn_error", error.to_string()),
                ],
            },
        }
    }
}
