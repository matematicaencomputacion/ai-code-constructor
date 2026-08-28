//! Operaciones de edición estructuradas para [`crate::harness::AgentAction::ApplyCorrection`].
//!
//! Independiente de Builder, Validator y Compiler.
//!
//! Contrato v2: cada [`Correction`] puede apuntar a un [`ArtifactPath`] existente del
//! [`crate::harness::RustArtifact`]. `path = None` → archivo **primary** (compat single-file).

use crate::harness::artifact::RustArtifact;
use crate::harness::artifact_path::ArtifactPath;

/// Target autorizado para correcciones (sandbox mínima).
pub const SESSION_CODE_TARGET: &str = "session_code";

/// Artefacto sobre el que se permite aplicar una corrección.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionTarget {
    /// Código de sesión gestionado por el Harness (`AgentContext::working_artifact`).
    SessionCode,
}

impl CorrectionTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionCode => SESSION_CODE_TARGET,
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            SESSION_CODE_TARGET => Ok(Self::SessionCode),
            other => Err(format!("target no autorizado: {other}")),
        }
    }
}

/// Operación atómica de edición de texto.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // nombres explícitos del contrato Harness
pub enum CorrectionOperation {
    ReplaceText { search: String, replacement: String },
    InsertText { position: usize, text: String },
    RemoveText { start: usize, end: usize },
}

impl CorrectionOperation {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::ReplaceText { .. } => "replace_text",
            Self::InsertText { .. } => "insert_text",
            Self::RemoveText { .. } => "remove_text",
        }
    }
}

/// Corrección estructurada: target + archivo opcional + operación.
///
/// - `path = None` → opera sobre el **primary** del Artifact (compat legacy).
/// - `path = Some(p)` → opera sobre ese archivo existente (no crea archivos).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub target: CorrectionTarget,
    pub path: Option<ArtifactPath>,
    pub operation: CorrectionOperation,
}

impl Correction {
    /// ReplaceText sobre el primary (compat single-file).
    pub fn replace_session_text(search: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self {
            target: CorrectionTarget::SessionCode,
            path: None,
            operation: CorrectionOperation::ReplaceText {
                search: search.into(),
                replacement: replacement.into(),
            },
        }
    }

    /// ReplaceText sobre un archivo existente del Artifact.
    pub fn replace_file_text(
        path: ArtifactPath,
        search: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Self {
        Self {
            target: CorrectionTarget::SessionCode,
            path: Some(path),
            operation: CorrectionOperation::ReplaceText {
                search: search.into(),
                replacement: replacement.into(),
            },
        }
    }

    pub fn insert_session_text(position: usize, text: impl Into<String>) -> Self {
        Self {
            target: CorrectionTarget::SessionCode,
            path: None,
            operation: CorrectionOperation::InsertText {
                position,
                text: text.into(),
            },
        }
    }

    pub fn remove_session_text(start: usize, end: usize) -> Self {
        Self {
            target: CorrectionTarget::SessionCode,
            path: None,
            operation: CorrectionOperation::RemoveText { start, end },
        }
    }

    /// Path efectivo: explícito o primary del Artifact.
    pub fn resolved_path<'a>(&'a self, artifact: &'a RustArtifact) -> &'a ArtifactPath {
        self.path
            .as_ref()
            .unwrap_or_else(|| artifact.primary_path())
    }

    /// Aplica la operación sobre un buffer de texto (tests / ops atómicas).
    pub fn apply_to(&self, code: &str) -> Result<String, String> {
        match self.target {
            CorrectionTarget::SessionCode => apply_operation(code, &self.operation),
        }
    }

    /// Aplica esta corrección a un archivo existente del Artifact (batch de 1).
    ///
    /// Delega en [`apply_corrections_to_artifact`] (+1 revision si hay cambio).
    pub fn apply_to_artifact(&self, artifact: &mut RustArtifact) -> Result<(), String> {
        apply_corrections_to_artifact(artifact, std::slice::from_ref(self))
    }
}

/// Aplica una secuencia de correcciones sobre un buffer único (legacy / tests).
pub fn apply_corrections(code: &str, corrections: &[Correction]) -> Result<String, String> {
    let mut current = code.to_string();
    for correction in corrections {
        current = correction.apply_to(&current)?;
    }
    Ok(current)
}

/// Valida un batch de correcciones sin mutar el artifact canónico.
pub fn validate_corrections(
    artifact: &RustArtifact,
    corrections: &[Correction],
) -> Result<(), String> {
    preview_corrections_to_artifact(artifact, corrections).map(|_| ())
}

/// Calcula el estado resultante de un batch de correcciones sin mutar el original.
pub fn preview_corrections_to_artifact(
    artifact: &RustArtifact,
    corrections: &[Correction],
) -> Result<RustArtifact, String> {
    if corrections.is_empty() {
        return Err("ApplyCorrection requiere al menos una corrección".to_string());
    }
    let mut trial = artifact.clone();
    for correction in corrections {
        apply_single_correction_to_trial(&mut trial, correction)?;
    }
    Ok(trial)
}

/// Aplica correcciones al Artifact canónico en un **batch atómico**.
///
/// Todas las correcciones se validan sobre un snapshot; si alguna falla, no hay cambio.
/// Un batch exitoso con diff real incrementa `revision` exactamente una vez.
pub fn apply_corrections_to_artifact(
    artifact: &mut RustArtifact,
    corrections: &[Correction],
) -> Result<(), String> {
    let preview = preview_corrections_to_artifact(artifact, corrections)?;
    crate::harness::artifact_mutation::commit_artifact_preview(artifact, preview)?;
    Ok(())
}

fn apply_single_correction_to_trial(
    trial: &mut RustArtifact,
    correction: &Correction,
) -> Result<(), String> {
    if correction.target != CorrectionTarget::SessionCode {
        return Err("target de corrección no autorizado".to_string());
    }
    let path = correction.resolved_path(trial).clone();
    let Some(current) = trial.file(&path).map(str::to_string) else {
        return Err(format!("archivo inexistente: {}", path.as_str()));
    };
    let next = apply_operation(&current, &correction.operation)?;
    if next != current {
        trial.insert_file_internal(path, next);
    }
    Ok(())
}

fn apply_operation(code: &str, operation: &CorrectionOperation) -> Result<String, String> {
    match operation {
        CorrectionOperation::ReplaceText {
            search,
            replacement,
        } => {
            if search.is_empty() {
                return Err("ReplaceText: search vacío".to_string());
            }
            if !code.contains(search.as_str()) {
                return Err(format!("ReplaceText: no se encontró `{search}`"));
            }
            Ok(code.replace(search.as_str(), replacement.as_str()))
        }
        CorrectionOperation::InsertText { position, text } => {
            if *position > code.len() {
                return Err(format!(
                    "InsertText: position {position} fuera de rango (len={})",
                    code.len()
                ));
            }
            let mut result = String::with_capacity(code.len() + text.len());
            result.push_str(&code[..*position]);
            result.push_str(text);
            result.push_str(&code[*position..]);
            Ok(result)
        }
        CorrectionOperation::RemoveText { start, end } => {
            if start > end || *end > code.len() {
                return Err(format!(
                    "RemoveText: rango inválido [{start}, {end}) para len={}",
                    code.len()
                ));
            }
            let mut result = String::with_capacity(code.len());
            result.push_str(&code[..*start]);
            result.push_str(&code[*end..]);
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::artifact::ArtifactId;

    #[test]
    fn correction_replace_text_works() {
        let correction = Correction::replace_session_text("NET", "HTTP");
        let result = correction.apply_to("Servidor NET").expect("replace");
        assert_eq!(result, "Servidor HTTP");
    }

    #[test]
    fn correction_insert_text_works() {
        let correction = Correction::insert_session_text(3, "X");
        let result = correction.apply_to("abc").expect("insert");
        assert_eq!(result, "abcX");
    }

    #[test]
    fn correction_remove_text_works() {
        let correction = Correction::remove_session_text(1, 3);
        let result = correction.apply_to("abcd").expect("remove");
        assert_eq!(result, "ad");
    }

    #[test]
    fn correction_rejects_unauthorized_target_parse() {
        assert!(CorrectionTarget::parse("/etc/passwd").is_err());
    }

    #[test]
    fn batch_corrections_increment_revision_once() {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let mut artifact = RustArtifact::try_from_files(
            ArtifactId::new("art-batch"),
            "main.rs",
            main.clone(),
            [
                (
                    main.clone(),
                    "mod helper;\nfn main() { helper::value(); }\n".to_string(),
                ),
                (helper.clone(), "pub fn value() -> i32 { 1 }\n".to_string()),
            ],
        )
        .unwrap();
        apply_corrections_to_artifact(
            &mut artifact,
            &[
                Correction::replace_file_text(helper.clone(), "1", "2"),
                Correction::replace_file_text(main, "helper::value()", "helper::value() /*x*/"),
            ],
        )
        .unwrap();
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn replace_file_preserves_siblings_and_identity() {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let mut artifact = RustArtifact::try_from_files(
            ArtifactId::new("art-corr-multi"),
            "main.rs",
            main.clone(),
            [
                (
                    main,
                    "mod helper;\nfn main() { println!(\"{}\", helper::value()); }\n".to_string(),
                ),
                (
                    helper.clone(),
                    "pub fn value() -> i32 {\n    1\n}\n".to_string(),
                ),
            ],
        )
        .unwrap()
        .with_specification_id(crate::harness::specification::SpecificationId::new(
            "spec-1",
        ));
        let id_before = artifact.id().clone();
        let main_before = artifact.source().to_string();
        let rev_before = artifact.revision();

        Correction::replace_file_text(helper.clone(), "1", "2")
            .apply_to_artifact(&mut artifact)
            .unwrap();

        assert_eq!(artifact.id(), &id_before);
        assert_eq!(
            artifact.specification_id().map(|s| s.as_str()),
            Some("spec-1")
        );
        assert_eq!(artifact.source(), main_before);
        assert!(artifact.file(&helper).unwrap().contains('2'));
        assert!(!artifact.file(&helper).unwrap().contains("    1\n"));
        assert_eq!(artifact.revision(), rev_before + 1);
    }

    #[test]
    fn missing_file_and_missing_search_fail() {
        let mut artifact = RustArtifact::new("main.rs", "fn main() { 1 }");
        let missing = ArtifactPath::parse("src/helper.rs").unwrap();
        assert!(
            Correction::replace_file_text(missing, "1", "2")
                .apply_to_artifact(&mut artifact)
                .unwrap_err()
                .contains("inexistente")
        );
        assert!(
            Correction::replace_session_text("zzz", "2")
                .apply_to_artifact(&mut artifact)
                .unwrap_err()
                .contains("no se encontró")
        );
    }
}
