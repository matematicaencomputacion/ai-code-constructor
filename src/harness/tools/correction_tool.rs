use crate::harness::artifact_mutation::artifact_files_unchanged;
use crate::harness::artifact_path::ArtifactPath;
use crate::harness::context::AgentContext;
use crate::harness::correction::{
    Correction, CorrectionOperation, CorrectionTarget, SESSION_CODE_TARGET,
    preview_corrections_to_artifact,
};
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::APPLY_CORRECTION;

const CORR_SEP: &str = "\n<<<ACC_CORR>>>\n";
const FIELD_SEP: &str = "\n<<<ACC_FIELD>>>\n";

/// Codifica correcciones para [`crate::harness::AgentAction::ApplyCorrection`].
pub fn encode_correction_input(corrections: &[Correction]) -> String {
    let mut parts = vec![SESSION_CODE_TARGET.to_string()];
    for correction in corrections {
        parts.push(encode_correction(correction));
    }
    parts.join(CORR_SEP)
}

fn encode_correction(correction: &Correction) -> String {
    let path = correction
        .path
        .as_ref()
        .map(ArtifactPath::as_str)
        .unwrap_or("");
    match &correction.operation {
        CorrectionOperation::ReplaceText {
            search,
            replacement,
        } => {
            format!("replace{FIELD_SEP}{path}{FIELD_SEP}{search}{FIELD_SEP}{replacement}")
        }
        CorrectionOperation::InsertText { position, text } => {
            format!("insert{FIELD_SEP}{path}{FIELD_SEP}{position}{FIELD_SEP}{text}")
        }
        CorrectionOperation::RemoveText { start, end } => {
            format!("remove{FIELD_SEP}{path}{FIELD_SEP}{start}{FIELD_SEP}{end}")
        }
    }
}

fn decode_correction_input(input: &str) -> Result<Vec<Correction>, String> {
    let segments: Vec<&str> = input.split(CORR_SEP).collect();
    if segments.is_empty() {
        return Err("entrada de corrección vacía".to_string());
    }

    let target = CorrectionTarget::parse(segments[0])?;
    let mut corrections = Vec::new();
    for segment in segments.iter().skip(1) {
        corrections.push(decode_correction(target, segment)?);
    }

    if corrections.is_empty() {
        return Err("se requiere al menos una operación de corrección".to_string());
    }

    Ok(corrections)
}

fn decode_path(raw: &str) -> Result<Option<ArtifactPath>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        ArtifactPath::parse(trimmed).map(Some)
    }
}

fn decode_correction(target: CorrectionTarget, raw: &str) -> Result<Correction, String> {
    let fields: Vec<&str> = raw.split(FIELD_SEP).collect();
    // Formato v2: kind | path | …args
    // Compat v1 (sin path): kind | …args  (3 campos replace/insert/remove)
    match fields.first().copied() {
        Some("replace") if fields.len() == 4 => Ok(Correction {
            target,
            path: decode_path(fields[1])?,
            operation: CorrectionOperation::ReplaceText {
                search: fields[2].to_string(),
                replacement: fields[3].to_string(),
            },
        }),
        Some("replace") if fields.len() == 3 => Ok(Correction {
            target,
            path: None,
            operation: CorrectionOperation::ReplaceText {
                search: fields[1].to_string(),
                replacement: fields[2].to_string(),
            },
        }),
        Some("insert") if fields.len() == 4 => {
            let position = fields[2]
                .parse::<usize>()
                .map_err(|_| format!("InsertText: position inválida `{}`", fields[2]))?;
            Ok(Correction {
                target,
                path: decode_path(fields[1])?,
                operation: CorrectionOperation::InsertText {
                    position,
                    text: fields[3].to_string(),
                },
            })
        }
        Some("insert") if fields.len() == 3 => {
            let position = fields[1]
                .parse::<usize>()
                .map_err(|_| format!("InsertText: position inválida `{}`", fields[1]))?;
            Ok(Correction {
                target,
                path: None,
                operation: CorrectionOperation::InsertText {
                    position,
                    text: fields[2].to_string(),
                },
            })
        }
        Some("remove") if fields.len() == 4 => {
            let start = fields[2]
                .parse::<usize>()
                .map_err(|_| format!("RemoveText: start inválido `{}`", fields[2]))?;
            let end = fields[3]
                .parse::<usize>()
                .map_err(|_| format!("RemoveText: end inválido `{}`", fields[3]))?;
            Ok(Correction {
                target,
                path: decode_path(fields[1])?,
                operation: CorrectionOperation::RemoveText { start, end },
            })
        }
        Some("remove") if fields.len() == 3 => {
            let start = fields[1]
                .parse::<usize>()
                .map_err(|_| format!("RemoveText: start inválido `{}`", fields[1]))?;
            let end = fields[2]
                .parse::<usize>()
                .map_err(|_| format!("RemoveText: end inválido `{}`", fields[2]))?;
            Ok(Correction {
                target,
                path: None,
                operation: CorrectionOperation::RemoveText { start, end },
            })
        }
        Some(kind) => Err(format!("operación desconocida o mal formada: {kind}")),
        None => Err("operación vacía".to_string()),
    }
}

/// Valida correcciones sobre el Artifact de sesión y produce preview + Evidence.
///
/// No ejecuta Validator, Compiler ni Repairer. El commit canónico lo realiza el Harness
/// desde [`ToolResult::artifact_preview`].
pub struct CorrectionTool;

impl Tool for CorrectionTool {
    fn name(&self) -> &str {
        APPLY_CORRECTION
    }

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult {
        let corrections = match decode_correction_input(input) {
            Ok(parsed) => parsed,
            Err(error) => {
                return ToolResult::failure(
                    error.clone(),
                    vec![
                        Evidence::new("tool", APPLY_CORRECTION),
                        Evidence::new("correction_status", "error"),
                        Evidence::new("parse_error", error),
                    ],
                );
            }
        };

        if corrections
            .iter()
            .any(|c| c.target != CorrectionTarget::SessionCode)
        {
            return ToolResult::failure(
                "target no autorizado",
                vec![
                    Evidence::new("tool", APPLY_CORRECTION),
                    Evidence::new("correction_status", "error"),
                    Evidence::new("security", "target_rejected"),
                ],
            );
        }

        let Some(base_artifact) = ctx.working_artifact.as_ref() else {
            return ToolResult::failure(
                "no hay Artifact de sesión autorizado para corregir",
                vec![
                    Evidence::new("tool", APPLY_CORRECTION),
                    Evidence::new("correction_status", "error"),
                    Evidence::new("security", "missing_session_code"),
                ],
            );
        };

        let preview = match preview_corrections_to_artifact(base_artifact, &corrections) {
            Ok(value) => value,
            Err(error) => {
                return ToolResult::failure(
                    error.clone(),
                    vec![
                        Evidence::new("tool", APPLY_CORRECTION),
                        Evidence::new("correction_status", "error"),
                        Evidence::new("correction_target", SESSION_CODE_TARGET),
                        Evidence::new("apply_error", error),
                    ],
                );
            }
        };

        let changed = !artifact_files_unchanged(base_artifact, &preview);
        let corrected_primary = preview.source().to_string();
        let paths: Vec<String> = corrections
            .iter()
            .map(|c| c.resolved_path(&preview).as_str().to_string())
            .collect();

        let mut evidence = vec![
            Evidence::new("tool", APPLY_CORRECTION),
            Evidence::new(
                "correction_status",
                if changed { "ok" } else { "unchanged" },
            ),
            Evidence::new("correction_target", SESSION_CODE_TARGET),
            Evidence::new("correction_count", corrections.len().to_string()),
            Evidence::new("code_changed", changed.to_string()),
            Evidence::new("corrected_code", &corrected_primary),
            Evidence::new("base_code_bytes", base_artifact.source().len().to_string()),
            Evidence::new("corrected_code_bytes", corrected_primary.len().to_string()),
            Evidence::new("correction_paths", paths.join(",")),
        ];
        ctx.append_artifact_evidence(&mut evidence);

        for (index, correction) in corrections.iter().enumerate() {
            evidence.push(Evidence::new(
                format!("correction_{index}_kind"),
                correction.operation.kind_label(),
            ));
            evidence.push(Evidence::new(
                format!("correction_{index}_path"),
                correction.resolved_path(base_artifact).as_str(),
            ));
            evidence.push(Evidence::new(
                format!("correction_{index}_description"),
                describe_operation(&correction.operation),
            ));
        }

        let mut result = ToolResult {
            success: changed,
            output: if changed {
                format!(
                    "corrección aplicada: {} operación(es) sobre [{}]",
                    corrections.len(),
                    paths.join(", ")
                )
            } else {
                "corrección no modificó el código".to_string()
            },
            evidence,
            artifact_preview: None,
        };
        if changed {
            result = result.with_artifact_preview(preview);
        }
        result
    }
}

fn describe_operation(operation: &CorrectionOperation) -> String {
    match operation {
        CorrectionOperation::ReplaceText {
            search,
            replacement,
        } => format!("ReplaceText `{search}` → `{replacement}`"),
        CorrectionOperation::InsertText { position, text } => {
            format!("InsertText pos={position} len={}", text.len())
        }
        CorrectionOperation::RemoveText { start, end } => {
            format!("RemoveText [{start}, {end})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_mutation::commit_artifact_preview;
    use crate::harness::specification::Specification;

    #[test]
    fn correction_tool_modifies_artifact() {
        // E
        let tool = CorrectionTool;
        let mut ctx = AgentContext::new("corr").with_working_artifact(RustArtifact::with_id(
            ArtifactId::new("art-corr"),
            "main.rs",
            "Servidor NET",
        ));
        let id_before = ctx.working_artifact.as_ref().unwrap().id().clone();
        let input = encode_correction_input(&[Correction::replace_session_text("NET", "HTTP")]);
        let result = tool.execute(&input, &ctx);

        assert!(result.success);
        assert!(result.evidence.iter().any(|e| e.label == "corrected_code"));
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-corr")
        );
        let corrected = result
            .evidence
            .iter()
            .find(|e| e.label == "corrected_code")
            .map(|e| e.detail.as_str())
            .expect("corrected_code");
        assert_eq!(corrected, "Servidor HTTP");

        let preview = result.artifact_preview.expect("preview");
        commit_artifact_preview(ctx.working_artifact.as_mut().unwrap(), preview).unwrap();
        assert_eq!(ctx.working_code(), Some("Servidor HTTP"));
        assert_eq!(ctx.working_artifact.as_ref().unwrap().id(), &id_before);
    }

    #[test]
    fn correction_tool_does_not_mutate_specification() {
        // F
        let spec = Specification::new("spec-corr", "Crear una API REST");
        let tool = CorrectionTool;
        let mut ctx = AgentContext::new("corr")
            .with_working_code("Servidor NET")
            .with_evaluation_specification(spec.clone());
        let corrections = [Correction::replace_session_text("NET", "HTTP")];
        let input = encode_correction_input(&corrections);
        let result = tool.execute(&input, &ctx);
        assert!(result.success);
        let preview = result.artifact_preview.expect("preview");
        commit_artifact_preview(ctx.working_artifact.as_mut().unwrap(), preview).unwrap();
        assert_eq!(
            ctx.evaluation_specification.as_ref().map(|s| s.id.as_str()),
            Some(spec.id.as_str())
        );
        assert_eq!(
            ctx.evaluation_specification.as_ref().unwrap().goal,
            spec.goal
        );
    }

    #[test]
    fn correction_tool_rejects_unauthorized_target() {
        let tool = CorrectionTool;
        let ctx = AgentContext::new("corr").with_working_code("fn main() {}");
        let input = format!("../secrets{CORR_SEP}replace{FIELD_SEP}x{FIELD_SEP}y");
        let result = tool.execute(&input, &ctx);
        assert!(!result.success);
        assert!(
            result.output.contains("target no autorizado")
                || result.evidence.iter().any(|e| e.label == "parse_error")
        );
    }

    #[test]
    fn correction_tool_does_not_run_validator_or_compiler() {
        let tool = CorrectionTool;
        let ctx = AgentContext::new("corr").with_working_code("abc");
        let input = encode_correction_input(&[Correction::replace_session_text("a", "z")]);
        let result = tool.execute(&input, &ctx);

        assert!(!result.evidence.iter().any(|e| e.label == "validate_status"));
        assert!(!result.evidence.iter().any(|e| e.label == "compile_status"));
        assert!(
            !result
                .evidence
                .iter()
                .any(|e| e.label.starts_with("repairer_feedback_"))
        );
    }

    #[test]
    fn replace_insert_remove_operations_via_tool() {
        let tool = CorrectionTool;
        let mut ctx = AgentContext::new("ops").with_working_code("abc");

        let replace = tool.execute(
            &encode_correction_input(&[Correction::replace_session_text("b", "X")]),
            &ctx,
        );
        assert!(replace.success);

        ctx.update_working_source("abc");
        let insert = tool.execute(
            &encode_correction_input(&[Correction::insert_session_text(1, "Z")]),
            &ctx,
        );
        assert!(insert.success);

        ctx.update_working_source("abcd");
        let remove = tool.execute(
            &encode_correction_input(&[Correction::remove_session_text(1, 3)]),
            &ctx,
        );
        assert!(remove.success);
    }
}
