//! Transformación WHAT → HOW: [`Specification`] → [`crate::planner::BuildPlan`].
//!
//! El Planner determinista no modifica la Specification; solo la lee.

use crate::harness::specification::{Specification, SpecificationId, SpecificationValidationError};
use crate::planner::{BuildPlan, build_steps_for_kind, classify_plan_kind};

/// Resultado trazable de planificar desde una Specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecificationBuildPlan {
    pub specification_id: SpecificationId,
    pub plan: BuildPlan,
}

/// Error al planificar desde una Specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecificationPlannerError {
    InvalidSpecification(SpecificationValidationError),
}

impl std::fmt::Display for SpecificationPlannerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSpecification(error) => write!(f, "specification inválida: {error}"),
        }
    }
}

/// Planifica a partir de una [`Specification`] validada (WHAT → HOW).
///
/// No modifica `spec`. Los acceptance criteria permanecen en la Specification
/// para evaluación futura; no se convierten en steps de implementación.
pub fn plan_specification(
    spec: &Specification,
) -> Result<SpecificationBuildPlan, SpecificationPlannerError> {
    spec.validate()
        .map_err(SpecificationPlannerError::InvalidSpecification)?;

    let kind = classify_plan_kind(combined_planning_text(spec).as_str());
    let mut steps = build_steps_for_kind(kind);
    enrich_steps_from_requirements(&mut steps, spec);

    Ok(SpecificationBuildPlan {
        specification_id: spec.id.clone(),
        plan: BuildPlan { kind, steps },
    })
}

/// Texto combinado para clasificar estrategia: goal + requirements (no criteria).
fn combined_planning_text(spec: &Specification) -> String {
    let mut parts = vec![spec.goal.clone()];
    for requirement in &spec.requirements {
        parts.push(requirement.description.clone());
    }
    parts.join(" ")
}

/// Incorpora requirements no cubiertos por los steps base como pasos trazables.
fn enrich_steps_from_requirements(steps: &mut Vec<String>, spec: &Specification) {
    for requirement in &spec.requirements {
        if requirement_covered_by_steps(steps, &requirement.description) {
            continue;
        }
        steps.push(format!(
            "[requirement:{}] {}",
            requirement.id.as_str(),
            requirement.description
        ));
    }
}

fn requirement_covered_by_steps(steps: &[String], description: &str) -> bool {
    let normalized = description.to_lowercase();
    steps.iter().any(|step| {
        let step_lower = step.to_lowercase();
        step_lower.contains(&normalized) || normalized.contains(&step_lower)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::specification::{AcceptanceCriterion, Requirement};
    use crate::planner::{PlanKind, plan_from_goal};

    fn api_specification() -> Specification {
        Specification::new("spec-api-001", "Crear una API REST")
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
                .satisfying([crate::harness::RequirementId::new("req-health-endpoint")]),
            ])
    }

    fn calculator_specification() -> Specification {
        Specification::new("spec-calc-001", "Crear una calculadora").with_requirements(vec![
            Requirement::new("req-ops", "Debe soportar suma y resta"),
        ])
    }

    fn auth_specification() -> Specification {
        Specification::new("spec-auth-001", "Crear un sistema de autenticación")
    }

    fn generic_specification() -> Specification {
        Specification::new("spec-generic-001", "Crear una aplicación de inventario")
    }

    #[test]
    fn valid_specification_produces_build_plan() {
        let spec = api_specification();
        let planned = plan_specification(&spec).expect("plan válido");
        assert_eq!(planned.plan.kind, PlanKind::Api);
        assert!(planned.plan.steps.len() >= 4);
        assert_eq!(planned.specification_id.as_str(), "spec-api-001");
    }

    #[test]
    fn api_specification_produces_api_plan_kind() {
        let planned = plan_specification(&api_specification()).expect("plan");
        assert_eq!(planned.plan.kind, PlanKind::Api);
    }

    #[test]
    fn calculator_specification_produces_calculator_plan_kind() {
        let planned = plan_specification(&calculator_specification()).expect("plan");
        assert_eq!(planned.plan.kind, PlanKind::Calculator);
    }

    #[test]
    fn authentication_specification_produces_authentication_plan_kind() {
        let planned = plan_specification(&auth_specification()).expect("plan");
        assert_eq!(planned.plan.kind, PlanKind::Authentication);
    }

    #[test]
    fn unknown_specification_produces_generic_plan_kind() {
        let planned = plan_specification(&generic_specification()).expect("plan");
        assert_eq!(planned.plan.kind, PlanKind::Generic);
    }

    #[test]
    fn requirements_can_influence_build_plan_steps() {
        let spec = api_specification();
        let planned = plan_specification(&spec).expect("plan");
        assert!(
            planned
                .plan
                .steps
                .iter()
                .any(|step| step.contains("[requirement:req-health-endpoint]")),
            "requirement no cubierto debe aparecer trazable en steps: {:?}",
            planned.plan.steps
        );
    }

    #[test]
    fn acceptance_criteria_are_not_converted_to_implementation_steps() {
        let spec = api_specification();
        let planned = plan_specification(&spec).expect("plan");
        assert!(
            !planned
                .plan
                .steps
                .iter()
                .any(|step| step.contains("ac-health-200") || step.contains("HTTP 200")),
            "acceptance criteria no deben volverse steps: {:?}",
            planned.plan.steps
        );
        assert_eq!(spec.acceptance_criteria.len(), 1);
    }

    #[test]
    fn planner_does_not_mutate_specification() {
        let spec = api_specification();
        let before = spec.clone();
        let _ = plan_specification(&spec).expect("plan");
        assert_eq!(spec, before);
    }

    #[test]
    fn invalid_specification_returns_controlled_error() {
        let spec = Specification::new("spec-invalid", "   ");
        let err = plan_specification(&spec).unwrap_err();
        assert!(matches!(
            err,
            SpecificationPlannerError::InvalidSpecification(
                SpecificationValidationError::EmptyGoal
            )
        ));
    }

    #[test]
    fn specification_id_remains_traceable_in_planned_output() {
        let spec = api_specification();
        let planned = plan_specification(&spec).expect("plan");
        assert_eq!(planned.specification_id, spec.id);
        assert_eq!(planned.specification_id.as_str(), "spec-api-001");
    }

    #[test]
    fn legacy_plan_from_goal_matches_specification_base_steps() {
        let spec = api_specification();
        let from_spec = plan_specification(&spec).expect("plan").plan;
        let from_goal = plan_from_goal(&spec.goal);
        assert_eq!(from_spec.kind, from_goal.kind);
        assert_eq!(
            &from_spec.steps[..from_goal.steps.len()],
            from_goal.steps.as_slice()
        );
    }

    #[test]
    fn specification_and_build_plan_remain_distinct_models() {
        let spec = api_specification();
        let planned = plan_specification(&spec).expect("plan");

        assert_ne!(spec.goal, planned.plan.steps.join(" "));
        assert!(!spec.requirements.is_empty());
        assert!(
            planned
                .plan
                .steps
                .iter()
                .any(|s| s.contains("servidor HTTP"))
        );
        assert!(
            spec.requirements
                .iter()
                .any(|r| r.description.contains("/health"))
        );
        assert_eq!(planned.plan.kind, PlanKind::Api);
    }
}
