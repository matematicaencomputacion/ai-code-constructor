mod clippy_tool;
mod compile_tool;
mod correction_tool;
mod fmt_tool;
mod repair_diagnostic_tool;
mod test_tool;
mod validation_tool;

pub use clippy_tool::ClippyTool;
pub use compile_tool::CompileTool;
pub use correction_tool::{CorrectionTool, encode_correction_input};
pub use fmt_tool::FmtTool;
pub use repair_diagnostic_tool::{RepairDiagnosticTool, encode_repair_diagnostic_input};
pub use test_tool::TestTool;
pub use validation_tool::{ValidationTool, encode_validate_input};

pub const COMPILE: &str = "compile";
pub const RUN_TESTS: &str = "run_tests";
pub const RUN_CLIPPY: &str = "run_clippy";
pub const CHECK_FORMAT: &str = "check_format";
pub const VALIDATE: &str = "validate";
pub const REPAIR_DIAGNOSTIC: &str = "repair_diagnostic";
pub const APPLY_CORRECTION: &str = "apply_correction";

use crate::harness::evaluation::Evidence;
use crate::harness::tool::ToolResult;
use std::process::Output;

pub(crate) fn tool_result_from_output(tool_name: &str, output: Output) -> ToolResult {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_status = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());

    let success = output.status.success();
    let summary = format!(
        "tool={tool_name} exit={exit_status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    ToolResult {
        success,
        output: summary,
        evidence: vec![
            Evidence::new("tool", tool_name),
            Evidence::new("exit_status", exit_status),
            Evidence::new("stdout", truncate(&stdout, 4_000)),
            Evidence::new("stderr", truncate(&stderr, 4_000)),
        ],
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}
