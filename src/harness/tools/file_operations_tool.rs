use crate::harness::artifact_file_operation::{
    ArtifactFileOperation, preview_file_operations_to_artifact,
};
use crate::harness::artifact_mutation::artifact_files_unchanged;
use crate::harness::artifact_path::ArtifactPath;
use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::APPLY_FILE_OPERATIONS;

const OP_SEP: &str = "\n<<<ACC_FILE_OP>>>\n";
const FIELD_SEP: &str = "\n<<<ACC_FIELD>>>\n";

/// Codifica operaciones estructurales para [`AgentAction::ApplyFileOperations`].
pub fn encode_file_operations_input(operations: &[ArtifactFileOperation]) -> String {
    operations
        .iter()
        .map(encode_file_operation)
        .collect::<Vec<_>>()
        .join(OP_SEP)
}

fn encode_file_operation(operation: &ArtifactFileOperation) -> String {
    match operation {
        ArtifactFileOperation::CreateFile { path, source } => {
            format!("create{FIELD_SEP}{}{FIELD_SEP}{}", path.as_str(), source)
        }
        ArtifactFileOperation::DeleteFile { path } => {
            format!("delete{FIELD_SEP}{}", path.as_str())
        }
        ArtifactFileOperation::RenameFile { from, to } => {
            format!(
                "rename{FIELD_SEP}{}{FIELD_SEP}{}",
                from.as_str(),
                to.as_str()
            )
        }
    }
}

pub fn decode_file_operations_input(input: &str) -> Result<Vec<ArtifactFileOperation>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("entrada de file_operations vacía".to_string());
    }
    trimmed.split(OP_SEP).map(decode_file_operation).collect()
}

fn decode_path(raw: &str) -> Result<ArtifactPath, String> {
    ArtifactPath::parse(raw).map_err(|message| format!("path inválido: {message}"))
}

fn decode_file_operation(raw: &str) -> Result<ArtifactFileOperation, String> {
    let fields: Vec<&str> = raw.split(FIELD_SEP).collect();
    match fields.first().copied() {
        Some("create") if fields.len() == 3 => Ok(ArtifactFileOperation::CreateFile {
            path: decode_path(fields[1])?,
            source: fields[2].to_string(),
        }),
        Some("delete") if fields.len() == 2 => Ok(ArtifactFileOperation::DeleteFile {
            path: decode_path(fields[1])?,
        }),
        Some("rename") if fields.len() == 3 => Ok(ArtifactFileOperation::RenameFile {
            from: decode_path(fields[1])?,
            to: decode_path(fields[2])?,
        }),
        Some(kind) => Err(format!("operación desconocida o mal formada: {kind}")),
        None => Err("operación vacía".to_string()),
    }
}

/// Valida operaciones estructurales y produce preview + Evidence.
///
/// El commit canónico lo realiza el Harness desde [`ToolResult::artifact_preview`].
pub struct FileOperationsTool;

impl Tool for FileOperationsTool {
    fn name(&self) -> &str {
        APPLY_FILE_OPERATIONS
    }

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult {
        let operations = match decode_file_operations_input(input) {
            Ok(parsed) => parsed,
            Err(error) => {
                return ToolResult::failure(
                    error.clone(),
                    vec![
                        Evidence::new("tool", APPLY_FILE_OPERATIONS),
                        Evidence::new("file_operation_status", "error"),
                        Evidence::new("parse_error", error),
                    ],
                );
            }
        };

        let Some(base_artifact) = ctx.working_artifact.as_ref() else {
            return ToolResult::failure(
                "no hay Artifact de sesión autorizado",
                vec![
                    Evidence::new("tool", APPLY_FILE_OPERATIONS),
                    Evidence::new("file_operation_status", "error"),
                    Evidence::new("security", "missing_session_artifact"),
                ],
            );
        };

        let preview = match preview_file_operations_to_artifact(base_artifact, &operations) {
            Ok(value) => value,
            Err(error) => {
                return ToolResult::failure(
                    error.clone(),
                    file_operation_error_evidence(&operations, &error),
                );
            }
        };

        let changed = !artifact_files_unchanged(base_artifact, &preview);
        let mut evidence = vec![
            Evidence::new("tool", APPLY_FILE_OPERATIONS),
            Evidence::new(
                "file_operation_status",
                if changed { "ok" } else { "unchanged" },
            ),
            Evidence::new("operation_count", operations.len().to_string()),
            Evidence::new("file_count", preview.file_count().to_string()),
        ];
        ctx.append_artifact_evidence(&mut evidence);

        for (index, operation) in operations.iter().enumerate() {
            evidence.push(Evidence::new(
                format!("file_operation_{index}_kind"),
                operation.kind_label(),
            ));
            match operation {
                ArtifactFileOperation::CreateFile { path, .. }
                | ArtifactFileOperation::DeleteFile { path } => {
                    evidence.push(Evidence::new(
                        format!("file_operation_{index}_path"),
                        path.as_str(),
                    ));
                }
                ArtifactFileOperation::RenameFile { from, to } => {
                    evidence.push(Evidence::new(
                        format!("file_operation_{index}_from"),
                        from.as_str(),
                    ));
                    evidence.push(Evidence::new(
                        format!("file_operation_{index}_to"),
                        to.as_str(),
                    ));
                }
            }
        }

        let mut result = ToolResult {
            success: changed,
            output: if changed {
                format!(
                    "operaciones estructurales aplicadas: {} operación(es)",
                    operations.len()
                )
            } else {
                "operaciones estructurales no modificaron el Artifact".to_string()
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

fn file_operation_error_evidence(
    operations: &[ArtifactFileOperation],
    error: &str,
) -> Vec<Evidence> {
    let mut evidence = vec![
        Evidence::new("tool", APPLY_FILE_OPERATIONS),
        Evidence::new("file_operation_status", "error"),
        Evidence::new("apply_error", error),
        Evidence::new("operation_count", operations.len().to_string()),
    ];
    if let Some(first) = operations.first() {
        evidence.push(Evidence::new("file_operation_0_kind", first.kind_label()));
    }
    evidence
}
