use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::{CHECK_FORMAT, tool_result_from_output};
use std::process::Command;

/// Ejecuta `cargo fmt --check`.
pub struct FmtTool;

impl Tool for FmtTool {
    fn name(&self) -> &str {
        CHECK_FORMAT
    }

    fn execute(&self, _input: &str, ctx: &AgentContext) -> ToolResult {
        let workspace = ctx.workspace();
        match Command::new("cargo")
            .arg("fmt")
            .arg("--check")
            .current_dir(&workspace)
            .output()
        {
            Ok(output) => tool_result_from_output(CHECK_FORMAT, output),
            Err(error) => ToolResult {
                success: false,
                output: format!("No se pudo ejecutar cargo fmt: {error}"),
                evidence: vec![
                    Evidence::new("tool", CHECK_FORMAT),
                    Evidence::new("spawn_error", error.to_string()),
                ],
            },
        }
    }
}
