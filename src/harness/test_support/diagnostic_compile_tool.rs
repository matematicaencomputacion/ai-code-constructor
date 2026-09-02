//! CompileTool determinista para E2E: emite `compiler_stderr` derivado del Artifact
//! sin invocar `cargo check` (evita flakiness por cache/locks en CI).

use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::COMPILE;

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

fn bare_identifier_lines(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                return None;
            }
            if trimmed
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                && trimmed
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
                && !RUST_KEYWORDS.contains(&trimmed)
            {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Simula compile FAIL/PASS inspeccionando el Artifact (sin reglas error→fix opacas).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticCompileTool;

impl Tool for DiagnosticCompileTool {
    fn name(&self) -> &str {
        COMPILE
    }

    fn execute(&self, _input: &str, ctx: &AgentContext) -> ToolResult {
        let Some(artifact) = ctx.working_artifact.as_ref() else {
            return ToolResult::failure(
                format!("working_artifact ausente para tool `{COMPILE}`"),
                vec![
                    Evidence::new("tool", COMPILE),
                    Evidence::new("compile_status", "error"),
                    Evidence::new("missing_artifact", "working_artifact required"),
                ],
            );
        };

        for (path, source) in artifact.files() {
            if let Some(token) = bare_identifier_lines(source).into_iter().next() {
                let stderr = format!(
                    "error[E0425]: cannot find value `{token}` in this scope\n --> {}:1:1",
                    path.as_str()
                );
                return ToolResult::failure(
                    stderr.clone(),
                    vec![
                        Evidence::new("tool", COMPILE),
                        Evidence::new("compile_status", "error"),
                        Evidence::new("compiler_stderr", stderr),
                    ],
                );
            }
        }

        ToolResult::success(
            "compilación exitosa (diagnostic compile tool)".to_string(),
            vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
                Evidence::new("code_bytes", artifact.source().len().to_string()),
            ],
        )
    }
}
