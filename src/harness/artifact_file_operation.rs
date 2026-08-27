//! Operaciones estructurales sobre [`RustArtifact::files`] (distinct from [`Correction`]).
//!
//! **MoveFile** no es un variant separado: `RenameFile { from, to }` con paths completos
//! cubre mover/renombrar dentro del árbol lógico del Artifact.

use crate::harness::artifact::RustArtifact;
use crate::harness::artifact_path::ArtifactPath;

/// Operación estructural sobre un archivo del Artifact (contenido vs estructura).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum ArtifactFileOperation {
    CreateFile {
        path: ArtifactPath,
        source: String,
    },
    DeleteFile {
        path: ArtifactPath,
    },
    RenameFile {
        from: ArtifactPath,
        to: ArtifactPath,
    },
}

impl ArtifactFileOperation {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::CreateFile { .. } => "create_file",
            Self::DeleteFile { .. } => "delete_file",
            Self::RenameFile { .. } => "rename_file",
        }
    }
}

/// Valida un batch contra el estado actual sin mutar.
pub fn validate_file_operations(
    artifact: &RustArtifact,
    operations: &[ArtifactFileOperation],
) -> Result<(), String> {
    if operations.is_empty() {
        return Err("ApplyFileOperations requiere al menos una operación".to_string());
    }
    let mut trial = artifact.clone();
    for operation in operations {
        apply_single_file_operation(&mut trial, operation)?;
    }
    Ok(())
}

/// Aplica un batch **atómico**: valida todas las operaciones sobre un snapshot;
/// si alguna falla, el Artifact original no cambia. Un batch exitoso incrementa
/// `revision` exactamente una vez.
pub fn apply_file_operations_to_artifact(
    artifact: &mut RustArtifact,
    operations: &[ArtifactFileOperation],
) -> Result<(), String> {
    if operations.is_empty() {
        return Err("ApplyFileOperations requiere al menos una operación".to_string());
    }

    let mut trial = artifact.clone();
    for operation in operations {
        apply_single_file_operation(&mut trial, operation)?;
    }

    if trial.files_snapshot() == artifact.files_snapshot()
        && trial.primary_path() == artifact.primary_path()
    {
        return Ok(());
    }

    artifact.commit_files_state(trial);
    Ok(())
}

fn apply_single_file_operation(
    artifact: &mut RustArtifact,
    operation: &ArtifactFileOperation,
) -> Result<(), String> {
    match operation {
        ArtifactFileOperation::CreateFile { path, source } => {
            if artifact.file(path).is_some() {
                return Err(format!("CreateFile: archivo ya existe: {}", path.as_str()));
            }
            artifact.insert_file_internal(path.clone(), source.clone());
            Ok(())
        }
        ArtifactFileOperation::DeleteFile { path } => {
            if path == artifact.primary_path() {
                return Err(format!(
                    "DeleteFile: no se puede eliminar el archivo primary `{}`",
                    path.as_str()
                ));
            }
            if artifact.file(path).is_none() {
                return Err(format!(
                    "DeleteFile: archivo inexistente: {}",
                    path.as_str()
                ));
            }
            artifact.remove_file_internal(path);
            Ok(())
        }
        ArtifactFileOperation::RenameFile { from, to } => {
            if from == to {
                return Ok(());
            }
            if artifact.file(from).is_none() {
                return Err(format!("RenameFile: origen inexistente: {}", from.as_str()));
            }
            if artifact.file(to).is_some() {
                return Err(format!("RenameFile: destino ya existe: {}", to.as_str()));
            }
            let source = artifact
                .take_file_internal(from)
                .expect("origen verificado");
            artifact.insert_file_internal(to.clone(), source);
            if artifact.primary_path() == from {
                artifact.set_primary_internal(to.clone());
            }
            Ok(())
        }
    }
}

impl RustArtifact {
    /// Crea un archivo nuevo. Falla si `path` ya existe (no overwrite silencioso).
    pub fn create_file(
        &mut self,
        path: ArtifactPath,
        source: impl Into<String>,
    ) -> Result<(), String> {
        apply_file_operations_to_artifact(
            self,
            &[ArtifactFileOperation::CreateFile {
                path,
                source: source.into(),
            }],
        )
    }

    /// Elimina un archivo no-primary existente.
    pub fn delete_file(&mut self, path: &ArtifactPath) -> Result<(), String> {
        apply_file_operations_to_artifact(
            self,
            &[ArtifactFileOperation::DeleteFile { path: path.clone() }],
        )
    }

    /// Renombra/mueve un archivo dentro del Artifact. Actualiza `primary` si aplica.
    pub fn rename_file(&mut self, from: ArtifactPath, to: ArtifactPath) -> Result<(), String> {
        apply_file_operations_to_artifact(self, &[ArtifactFileOperation::RenameFile { from, to }])
    }

    /// Batch atómico de operaciones estructurales (+1 revision si hay cambio).
    pub fn apply_file_operations(
        &mut self,
        operations: &[ArtifactFileOperation],
    ) -> Result<(), String> {
        apply_file_operations_to_artifact(self, operations)
    }
}
