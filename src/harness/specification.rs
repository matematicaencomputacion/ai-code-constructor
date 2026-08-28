//! Contrato explícito WHAT: qué debe lograr el sistema, independiente de HOW.
//!
//! [`Specification`] no conoce Agent, Tools, LLM, filesystem ni `CodeState`.
//! [`crate::planner::BuildPlan`] representa HOW; esta capa representa WHAT.

use crate::harness::criterion::CriterionKind;

/// Versión del contrato de Specification (evolución futura v1 → v2).
pub type SpecificationVersion = u32;

/// Versión activa del contrato implementado en esta unidad.
pub const SPECIFICATION_CONTRACT_VERSION: SpecificationVersion = 1;

/// Identidad estable de una Specification (no es el texto del goal).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpecificationId(String);

impl SpecificationId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identidad estable de un Requirement.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequirementId(String);

impl RequirementId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identidad estable de un AcceptanceCriterion (referenciable por Evaluation).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AcceptanceCriterionId(String);

impl AcceptanceCriterionId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Clave estable para trazabilidad Evaluation → Evidence → Observation.
    pub fn evaluation_key(&self) -> &str {
        self.as_str()
    }
}

/// QUÉ debe existir o cumplirse (intención verificable, no implementación).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub id: RequirementId,
    pub description: String,
}

impl Requirement {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: RequirementId::new(id),
            description: description.into(),
        }
    }
}

/// CÓMO sabemos que un Requirement está satisfecho (condición evaluable).
///
/// El ID identifica; [`CriterionKind`] define la semántica de evaluación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceCriterion {
    pub id: AcceptanceCriterionId,
    pub description: String,
    pub kind: CriterionKind,
    /// Requirements que este criterio verifica (trazabilidad).
    pub satisfies_requirements: Vec<RequirementId>,
}

impl AcceptanceCriterion {
    pub fn new(id: impl Into<String>, description: impl Into<String>, kind: CriterionKind) -> Self {
        Self {
            id: AcceptanceCriterionId::new(id),
            description: description.into(),
            kind,
            satisfies_requirements: Vec::new(),
        }
    }

    pub fn satisfying(mut self, requirement_ids: impl IntoIterator<Item = RequirementId>) -> Self {
        self.satisfies_requirements = requirement_ids.into_iter().collect();
        self
    }
}

/// Objetivo verificable del sistema: WHAT, no HOW.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Specification {
    pub id: SpecificationId,
    pub version: SpecificationVersion,
    pub goal: String,
    pub requirements: Vec<Requirement>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
}

impl Specification {
    pub fn new(id: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            id: SpecificationId::new(id),
            goal: goal.into(),
            version: SPECIFICATION_CONTRACT_VERSION,
            requirements: Vec::new(),
            acceptance_criteria: Vec::new(),
        }
    }

    pub fn with_version(mut self, version: SpecificationVersion) -> Self {
        self.version = version;
        self
    }

    pub fn with_requirements(mut self, requirements: Vec<Requirement>) -> Self {
        self.requirements = requirements;
        self
    }

    pub fn with_acceptance_criteria(mut self, criteria: Vec<AcceptanceCriterion>) -> Self {
        self.acceptance_criteria = criteria;
        self
    }

    /// Validación estructural del contrato (sin I/O, Tools, LLM ni compilación).
    pub fn validate(&self) -> Result<(), SpecificationValidationError> {
        if self.id.as_str().trim().is_empty() {
            return Err(SpecificationValidationError::EmptySpecificationId);
        }

        if self.goal.trim().is_empty() {
            return Err(SpecificationValidationError::EmptyGoal);
        }

        validate_unique_ids(
            self.requirements.iter().map(|item| item.id.as_str()),
            |id| SpecificationValidationError::DuplicateRequirementId { id: id.to_string() },
        )?;

        for (index, requirement) in self.requirements.iter().enumerate() {
            if requirement.id.as_str().trim().is_empty() {
                return Err(SpecificationValidationError::EmptyRequirementId { index });
            }
            if requirement.description.trim().is_empty() {
                return Err(SpecificationValidationError::EmptyRequirementDescription {
                    id: requirement.id.as_str().to_string(),
                });
            }
        }

        validate_unique_ids(
            self.acceptance_criteria.iter().map(|item| item.id.as_str()),
            |id| SpecificationValidationError::DuplicateAcceptanceCriterionId {
                id: id.to_string(),
            },
        )?;

        let known_requirements: std::collections::BTreeSet<&str> = self
            .requirements
            .iter()
            .map(|item| item.id.as_str())
            .collect();

        for criterion in &self.acceptance_criteria {
            if criterion.id.as_str().trim().is_empty() {
                return Err(SpecificationValidationError::EmptyAcceptanceCriterionId {
                    id: criterion.id.as_str().to_string(),
                });
            }
            if criterion.description.trim().is_empty() {
                return Err(
                    SpecificationValidationError::EmptyAcceptanceCriterionDescription {
                        id: criterion.id.as_str().to_string(),
                    },
                );
            }
            // CriterionKind es un enum cerrado: todas las variantes son válidas estructuralmente.
            // El ID puede ser arbitrario; no se exigen prefijos semánticos.

            for requirement_id in &criterion.satisfies_requirements {
                if requirement_id.as_str().trim().is_empty() {
                    return Err(SpecificationValidationError::EmptyRequirementId {
                        index: usize::MAX,
                    });
                }
                if !known_requirements.contains(requirement_id.as_str()) {
                    return Err(SpecificationValidationError::UnknownRequirementReference {
                        criterion_id: criterion.id.as_str().to_string(),
                        requirement_id: requirement_id.as_str().to_string(),
                    });
                }
            }
        }

        Ok(())
    }
}

/// Error de validación estructural del contrato Specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecificationValidationError {
    EmptySpecificationId,
    EmptyGoal,
    EmptyRequirementId {
        index: usize,
    },
    EmptyRequirementDescription {
        id: String,
    },
    EmptyAcceptanceCriterionId {
        id: String,
    },
    EmptyAcceptanceCriterionDescription {
        id: String,
    },
    DuplicateRequirementId {
        id: String,
    },
    DuplicateAcceptanceCriterionId {
        id: String,
    },
    UnknownRequirementReference {
        criterion_id: String,
        requirement_id: String,
    },
}

impl std::fmt::Display for SpecificationValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySpecificationId => write!(f, "specification id vacío"),
            Self::EmptyGoal => write!(f, "goal vacío"),
            Self::EmptyRequirementId { index } => {
                write!(f, "requirement id vacío en índice {index}")
            }
            Self::EmptyRequirementDescription { id } => {
                write!(f, "requirement description vacía: {id}")
            }
            Self::EmptyAcceptanceCriterionId { id } => {
                write!(f, "acceptance criterion id vacío: {id}")
            }
            Self::EmptyAcceptanceCriterionDescription { id } => {
                write!(f, "acceptance criterion description vacía: {id}")
            }
            Self::DuplicateRequirementId { id } => {
                write!(f, "requirement id duplicado: {id}")
            }
            Self::DuplicateAcceptanceCriterionId { id } => {
                write!(f, "acceptance criterion id duplicado: {id}")
            }
            Self::UnknownRequirementReference {
                criterion_id,
                requirement_id,
            } => write!(
                f,
                "criterion {criterion_id} referencia requirement desconocido {requirement_id}"
            ),
        }
    }
}

fn validate_unique_ids<'a, I, F>(
    ids: I,
    duplicate_error: F,
) -> Result<(), SpecificationValidationError>
where
    I: IntoIterator<Item = &'a str>,
    F: Fn(&str) -> SpecificationValidationError,
{
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(duplicate_error(id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{BuildPlan, PlanKind};
    use crate::state::CodeState;

    fn sample_valid_specification() -> Specification {
        Specification::new("spec-api-rest-001", "Crear una API REST")
            .with_requirements(vec![
                Requirement::new("req-http-server", "El sistema expone un servidor HTTP"),
                Requirement::new("req-health-endpoint", "Existe un endpoint /health"),
            ])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new(
                    "ac-health-200",
                    "GET /health responde HTTP 200",
                    CriterionKind::Unknown,
                )
                .satisfying([RequirementId::new("req-health-endpoint")]),
                AcceptanceCriterion::new(
                    "ac-server-listening",
                    "El servidor acepta conexiones en el puerto configurado",
                    CriterionKind::Unknown,
                )
                .satisfying([RequirementId::new("req-http-server")]),
            ])
    }

    #[test]
    fn specification_valid_contract_passes_validation() {
        let spec = sample_valid_specification();
        spec.validate().expect("specification válida");
        assert_eq!(spec.version, SPECIFICATION_CONTRACT_VERSION);
    }

    #[test]
    fn specification_rejects_empty_goal() {
        let spec = Specification::new("spec-empty-goal", "   ");
        let err = spec.validate().unwrap_err();
        assert!(matches!(err, SpecificationValidationError::EmptyGoal));
    }

    #[test]
    fn specification_rejects_empty_id() {
        let spec = Specification::new("", "Crear una API REST");
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::EmptySpecificationId
        ));
    }

    #[test]
    fn specification_rejects_invalid_requirements() {
        let spec = Specification::new("spec-req", "goal")
            .with_requirements(vec![Requirement::new("", "descripción")]);
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::EmptyRequirementId { .. }
        ));

        let spec = Specification::new("spec-req", "goal")
            .with_requirements(vec![Requirement::new("req-1", "   ")]);
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::EmptyRequirementDescription { .. }
        ));
    }

    #[test]
    fn specification_rejects_invalid_acceptance_criteria() {
        let spec = Specification::new("spec-ac", "goal")
            .with_requirements(vec![Requirement::new("req-1", "requiere algo")])
            .with_acceptance_criteria(vec![AcceptanceCriterion::new(
                "",
                "criterio",
                CriterionKind::Unknown,
            )]);
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::EmptyAcceptanceCriterionId { .. }
        ));

        let spec = Specification::new("spec-ac", "goal")
            .with_requirements(vec![Requirement::new("req-1", "requiere algo")])
            .with_acceptance_criteria(vec![AcceptanceCriterion::new(
                "ac-1",
                "   ",
                CriterionKind::Compile,
            )]);
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::EmptyAcceptanceCriterionDescription { .. }
        ));
    }

    #[test]
    fn specification_rejects_duplicate_ids() {
        let spec = Specification::new("spec-dup", "goal").with_requirements(vec![
            Requirement::new("req-dup", "uno"),
            Requirement::new("req-dup", "dos"),
        ]);
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::DuplicateRequirementId { .. }
        ));

        let spec = Specification::new("spec-dup", "goal")
            .with_requirements(vec![Requirement::new("req-1", "uno")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-dup", "a", CriterionKind::Compile)
                    .satisfying([RequirementId::new("req-1")]),
                AcceptanceCriterion::new("ac-dup", "b", CriterionKind::Validate)
                    .satisfying([RequirementId::new("req-1")]),
            ]);
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::DuplicateAcceptanceCriterionId { .. }
        ));
    }

    #[test]
    fn specification_rejects_unknown_requirement_reference() {
        let spec = Specification::new("spec-ref", "goal")
            .with_requirements(vec![Requirement::new("req-1", "uno")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-1", "criterio", CriterionKind::Unknown)
                    .satisfying([RequirementId::new("req-missing")]),
            ]);
        let err = spec.validate().unwrap_err();
        assert!(matches!(
            err,
            SpecificationValidationError::UnknownRequirementReference { .. }
        ));
    }

    #[test]
    fn specification_maintains_stable_identity() {
        let spec = sample_valid_specification();
        let cloned = spec.clone();
        assert_eq!(spec.id, cloned.id);
        assert_eq!(spec.id.as_str(), "spec-api-rest-001");
        assert_ne!(spec.id.as_str(), spec.goal);
    }

    #[test]
    fn specification_differentiates_what_from_build_plan() {
        let spec = sample_valid_specification();
        spec.validate().expect("spec válida");

        let build_plan = BuildPlan {
            kind: PlanKind::Api,
            steps: vec![
                "Crear servidor HTTP".to_string(),
                "Definir endpoints".to_string(),
            ],
        };

        let _what: &Specification = &spec;
        let _how: &BuildPlan = &build_plan;

        assert!(spec.goal.contains("API REST"));
        assert!(build_plan.steps.iter().any(|s| s.contains("servidor")));
        assert!(
            !spec
                .requirements
                .iter()
                .any(|r| r.description == build_plan.steps[0])
        );
    }

    #[test]
    fn specification_supports_multiple_requirements_and_criteria() {
        let spec = sample_valid_specification();
        assert_eq!(spec.requirements.len(), 2);
        assert_eq!(spec.acceptance_criteria.len(), 2);
        spec.validate().expect("válida");
    }

    #[test]
    fn acceptance_criterion_exposes_evaluation_key_for_future_traceability() {
        let criterion = AcceptanceCriterion::new(
            "ac-health-200",
            "GET /health responde HTTP 200",
            CriterionKind::Unknown,
        );
        assert_eq!(criterion.id.evaluation_key(), "ac-health-200");
        assert_eq!(criterion.kind, CriterionKind::Unknown);
    }

    #[test]
    fn specification_accepts_arbitrary_criterion_ids_without_semantic_prefixes() {
        let spec = Specification::new("spec-arbitrary-ids", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-1", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile)
                    .satisfying([RequirementId::new("req-1")]),
                AcceptanceCriterion::new("criterion-x", "valida", CriterionKind::Validate)
                    .satisfying([RequirementId::new("req-1")]),
                AcceptanceCriterion::new("abc123", "desconocido", CriterionKind::Unknown)
                    .satisfying([RequirementId::new("req-1")]),
            ]);
        spec.validate().expect("IDs arbitrarios válidos");
    }

    #[test]
    fn specification_contract_version_is_explicit() {
        let spec = Specification::new("spec-version", "goal").with_version(2);
        assert_eq!(spec.version, 2);
    }

    #[test]
    fn specification_validate_passes_without_touching_constructor_or_harness() {
        let spec = sample_valid_specification();
        spec.validate().expect("validate PASS");

        let mut state = CodeState {
            request: "Crear una API REST".to_string(),
            plan: None,
            code: None,
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        };
        let request_before = state.request.clone();
        crate::planner::plan(&mut state);
        assert_eq!(state.request, request_before);
        assert!(state.plan.is_some());
    }
}
