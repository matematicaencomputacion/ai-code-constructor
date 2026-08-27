//! Semántica explícita de un [`crate::harness::AcceptanceCriterion`].
//!
//! El ID identifica; [`CriterionKind`] describe el significado evaluable.
//! Una sola definición canónica para Specification y EvaluationEngine.

/// Tipo semántico determinista de un Acceptance Criterion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CriterionKind {
    Compile,
    Validate,
    RunTests,
    Clippy,
    CheckFormat,
    /// Criterio reconocido pero sin semántica evaluable por el engine actual.
    Unknown,
}

impl CriterionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compile => "compile",
            Self::Validate => "validate",
            Self::RunTests => "run_tests",
            Self::Clippy => "clippy",
            Self::CheckFormat => "check_format",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for CriterionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criterion_kind_variants_are_distinct() {
        assert_ne!(CriterionKind::Compile, CriterionKind::Validate);
        assert_eq!(CriterionKind::Compile.as_str(), "compile");
    }
}
