use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::{RUN_CLIPPY, tool_result_from_output};
use std::process::Command;

/// Ejecuta `cargo clippy -- -D warnings`.
pub struct ClippyTool;

impl Tool for ClippyTool {
    fn name(&self) -> &str {
        RUN_CLIPPY
    }

    fn execute(&self, _input: &str, ctx: &AgentContext) -> ToolResult {
        let workspace = ctx.workspace();
        match Command::new("cargo")
            .arg("clippy")
            .arg("--")
            .arg("-D")
            .arg("warnings")
            .current_dir(&workspace)
            .output()
        {
            Ok(output) => tool_result_from_output(RUN_CLIPPY, output),
            Err(error) => ToolResult {
                success: false,
                output: format!("No se pudo ejecutar cargo clippy: {error}"),
                evidence: vec![
                    Evidence::new("tool", RUN_CLIPPY),
                    Evidence::new("spawn_error", error.to_string()),
                ],
            },
        }
    }
}
