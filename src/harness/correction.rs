//! Operaciones de edición estructuradas para [`crate::harness::AgentAction::ApplyCorrection`].
//!
//! Independiente de Builder, Validator y Compiler.

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

/// Corrección estructurada: target + operación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub target: CorrectionTarget,
    pub operation: CorrectionOperation,
}

impl Correction {
    pub fn replace_session_text(search: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self {
            target: CorrectionTarget::SessionCode,
            operation: CorrectionOperation::ReplaceText {
                search: search.into(),
                replacement: replacement.into(),
            },
        }
    }

    pub fn insert_session_text(position: usize, text: impl Into<String>) -> Self {
        Self {
            target: CorrectionTarget::SessionCode,
            operation: CorrectionOperation::InsertText {
                position,
                text: text.into(),
            },
        }
    }

    pub fn remove_session_text(start: usize, end: usize) -> Self {
        Self {
            target: CorrectionTarget::SessionCode,
            operation: CorrectionOperation::RemoveText { start, end },
        }
    }

    pub fn apply_to(&self, code: &str) -> Result<String, String> {
        match self.target {
            CorrectionTarget::SessionCode => apply_operation(code, &self.operation),
        }
    }
}

/// Aplica una secuencia de correcciones sobre el código de sesión.
pub fn apply_corrections(code: &str, corrections: &[Correction]) -> Result<String, String> {
    let mut current = code.to_string();
    for correction in corrections {
        current = correction.apply_to(&current)?;
    }
    Ok(current)
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
}
