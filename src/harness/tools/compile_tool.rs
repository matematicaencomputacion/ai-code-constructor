//! Compila el [`crate::harness::RustArtifact`] de sesión vía crate Cargo materializado.
//!
//! No usa `rustc` sobre el primary aislado: materializa el árbol completo del Artifact
//! con [`ArtifactMaterialization`] y ejecuta `cargo check` en el root temporal.
//!
//! Evidence preserva el contrato EvaluationEngine: `tool=compile` + `compile_status`.

use std::process::Command;

#[cfg(test)]
use std::sync::Mutex;

use crate::harness::artifact_materialization::ArtifactMaterialization;
use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::COMPILE;

#[cfg(test)]
static COMPILE_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Compila el working Artifact (single- o multi-file) como crate temporal.
///
/// Requiere `working_artifact`. El `input` se ignora: el Harness actualiza el Artifact
/// antes de despachar; la fuente canónica es siempre el Artifact materializado.
pub struct CompileTool;

impl Tool for CompileTool {
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

        let materialization = match ArtifactMaterialization::from_artifact(artifact) {
            Ok(value) => value,
            Err(error) => {
                return ToolResult::failure(
                    error.clone(),
                    vec![
                        Evidence::new("tool", COMPILE),
                        Evidence::new("compile_status", "error"),
                        Evidence::new("materialization_error", error),
                    ],
                );
            }
        };

        // En tests, serializar cargo check: ejecución paralela compite por locks del cache.
        #[cfg(test)]
        let _compile_guard = COMPILE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // `cargo check`: verificación de compilación del crate completo (todos los files).
        // Suficiente para CriterionKind::Compile / compile_status; no requiere binario.
        let output = Command::new("cargo")
            .arg("check")
            .current_dir(materialization.root())
            .output();

        let mut result = match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if output.status.success() {
                    ToolResult::success(
                        format!(
                            "compilación exitosa (cargo check)\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                        ),
                        vec![
                            Evidence::new("tool", COMPILE),
                            Evidence::new("compile_status", "ok"),
                            Evidence::new("code_bytes", artifact.source().len().to_string()),
                        ],
                    )
                } else {
                    let error = if stderr.trim().is_empty() {
                        stdout.to_string()
                    } else {
                        stderr.to_string()
                    };
                    ToolResult::failure(
                        error.clone(),
                        vec![
                            Evidence::new("tool", COMPILE),
                            Evidence::new("compile_status", "error"),
                            Evidence::new("compiler_stderr", truncate(&error, 4_000)),
                        ],
                    )
                }
            }
            Err(error) => ToolResult::failure(
                format!("No se pudo ejecutar cargo check ({COMPILE}): {error}"),
                vec![
                    Evidence::new("tool", COMPILE),
                    Evidence::new("compile_status", "error"),
                    Evidence::new("spawn_error", error.to_string()),
                ],
            ),
        };

        ctx.append_artifact_evidence(&mut result.evidence);
        // `materialization` se dropea aquí → cleanup RAII del árbol temporal.
        result
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
