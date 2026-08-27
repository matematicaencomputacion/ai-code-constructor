//! Bridge Evaluation → AgentObservation.
//!
//! EvaluationEngine permanece independiente del Agent.
//! Este adapter convierte resultados de verificación en contexto de decisión.

use crate::harness::action::AgentAction;
use crate::harness::agent::Agent;
use crate::harness::context::AgentContext;
use crate::harness::criterion::CriterionKind;
use crate::harness::evaluation::{EvaluationVerdict, Evidence};
use crate::harness::evaluation_engine::{
    CriterionEvaluation, EvaluationEngine, SpecificationEvaluation, SpecificationEvaluationStatus,
};
use crate::harness::observation::AgentObservation;
use crate::harness::specification::{Specification, SpecificationId};

/// Convierte la evaluación de un criterio en Observation tipada.
pub fn observation_from_criterion_evaluation(
    specification_id: SpecificationId,
    evaluation: &CriterionEvaluation,
) -> AgentObservation {
    AgentObservation::CriterionEvaluated {
        specification_id,
        criterion_id: evaluation.criterion_id.clone(),
        kind: evaluation.kind,
        verdict: evaluation.verdict,
        message: evaluation.message.clone(),
        evidence: evaluation.evidence_used.clone(),
    }
}

/// Convierte la evaluación agregada de una Specification en Observation tipada.
pub fn observation_from_specification_evaluation(
    evaluation: &SpecificationEvaluation,
) -> AgentObservation {
    AgentObservation::SpecificationEvaluated {
        specification_id: evaluation.specification_id.clone(),
        status: evaluation.status,
        message: evaluation.message.clone(),
        criteria: evaluation.criteria.clone(),
    }
}

/// Mapea el nombre de Tool a [`CriterionKind`] evaluable (sin inferir desde AcceptanceCriterionId).
pub fn criterion_kind_for_tool(tool_name: &str) -> Option<CriterionKind> {
    use crate::harness::tools::{CHECK_FORMAT, COMPILE, RUN_CLIPPY, RUN_TESTS, VALIDATE};
    match tool_name {
        COMPILE => Some(CriterionKind::Compile),
        VALIDATE => Some(CriterionKind::Validate),
        RUN_TESTS => Some(CriterionKind::RunTests),
        RUN_CLIPPY => Some(CriterionKind::Clippy),
        CHECK_FORMAT => Some(CriterionKind::CheckFormat),
        _ => None,
    }
}

/// Resultado del paso de coordinación Evaluation tras una Tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEvaluationStep {
    pub evaluation: CriterionEvaluation,
    pub observation: AgentObservation,
}

/// Evalúa el AcceptanceCriterion cuyo [`CriterionKind`] corresponde a la Tool,
/// usando únicamente Evidence preexistente.
///
/// No ejecuta Tools. No conoce Agent ni AgentLoop.
pub fn evaluate_tool_evidence(
    specification: &Specification,
    tool_name: &str,
    evidence: &[Evidence],
) -> Option<ToolEvaluationStep> {
    let kind = criterion_kind_for_tool(tool_name)?;
    let criterion = specification
        .acceptance_criteria
        .iter()
        .find(|item| item.kind == kind)?;
    let evaluation = EvaluationEngine::new().evaluate_criterion(criterion, evidence);
    let observation = observation_from_criterion_evaluation(specification.id.clone(), &evaluation);
    Some(ToolEvaluationStep {
        evaluation,
        observation,
    })
}

/// Agent de demostración: decide a partir de Observation de Evaluation (causalidad).
///
/// No ejecuta EvaluationEngine; solo reacciona a Observations ya empujadas al contexto.
pub struct EvaluationAwareAgent {
    pub request: String,
}

impl EvaluationAwareAgent {
    pub fn new(request: impl Into<String>) -> Self {
        Self {
            request: request.into(),
        }
    }
}

impl Agent for EvaluationAwareAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            Some(AgentObservation::CriterionEvaluated {
                kind: CriterionKind::Compile,
                verdict: EvaluationVerdict::Fail,
                evidence,
                message,
                ..
            }) => {
                let mut errors: Vec<String> = evidence
                    .iter()
                    .filter(|item| item.label == "compiler_stderr")
                    .map(|item| item.detail.clone())
                    .collect();
                if errors.is_empty() {
                    errors.push(message.clone());
                }
                AgentAction::RepairDiagnostic { errors }
            }
            Some(AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Pass,
                ..
            })
            | Some(AgentObservation::SpecificationEvaluated {
                status: SpecificationEvaluationStatus::Pass,
                ..
            }) => AgentAction::Finish {
                summary: "evaluation pass".to_string(),
            },
            Some(AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::InsufficientEvidence,
                ..
            })
            | Some(AgentObservation::SpecificationEvaluated {
                status: SpecificationEvaluationStatus::InsufficientEvidence,
                ..
            }) => AgentAction::Finish {
                summary: "insufficient evidence - not pass".to_string(),
            },
            Some(AgentObservation::SpecificationEvaluated {
                status: SpecificationEvaluationStatus::Fail,
                criteria,
                ..
            }) => {
                if let Some(failed) = criteria.iter().find(|item| {
                    item.verdict == EvaluationVerdict::Fail && item.kind == CriterionKind::Compile
                }) {
                    let mut errors: Vec<String> = failed
                        .evidence_used
                        .iter()
                        .filter(|item| item.label == "compiler_stderr")
                        .map(|item| item.detail.clone())
                        .collect();
                    if errors.is_empty() {
                        errors.push(failed.message.clone());
                    }
                    return AgentAction::RepairDiagnostic { errors };
                }
                AgentAction::Finish {
                    summary: "specification evaluation fail".to_string(),
                }
            }
            Some(other) => AgentAction::Finish {
                summary: format!("fin tras: {}", other.summary()),
            },
            None => {
                if let Some(code) = ctx.working_code() {
                    AgentAction::Compile {
                        code: code.to_string(),
                    }
                } else {
                    AgentAction::Finish {
                        summary: format!("sin observación ({})", self.request),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::evaluation::Evidence;
    use crate::harness::evaluation_engine::EvaluationEngine;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::specification_planner::plan_specification;
    use crate::harness::tool::Tool;
    use crate::harness::tools::{COMPILE, CompileTool, RUN_TESTS, VALIDATE};

    fn compile_pass_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
        ]
    }

    fn compile_fail_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "error"),
            Evidence::new("compiler_stderr", "unclosed delimiter"),
        ]
    }

    fn validate_pass_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", VALIDATE),
            Evidence::new("validate_status", "ok"),
        ]
    }

    fn tests_fail_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", RUN_TESTS),
            Evidence::new("exit_status", "101"),
        ]
    }

    #[test]
    fn pass_evaluation_becomes_pass_observation() {
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &compile_pass_evidence());
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-1"), &evaluation);
        match observation {
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Pass,
                ..
            } => {}
            other => panic!("expected PASS observation, got {other:?}"),
        }
    }

    #[test]
    fn fail_evaluation_becomes_fail_observation() {
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &compile_fail_evidence());
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-1"), &evaluation);
        assert!(matches!(
            observation,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Fail,
                ..
            }
        ));
    }

    #[test]
    fn insufficient_evidence_becomes_insufficient_observation() {
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &[]);
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-1"), &evaluation);
        assert!(matches!(
            observation,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::InsufficientEvidence,
                ..
            }
        ));
        assert!(!observation.is_success());
        assert!(!observation.is_evaluation_pass());
    }

    #[test]
    fn observation_preserves_specification_and_criterion_identity() {
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-xyz", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &compile_pass_evidence());
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-api-42"), &evaluation);
        match observation {
            AgentObservation::CriterionEvaluated {
                specification_id,
                criterion_id,
                kind,
                message,
                evidence,
                ..
            } => {
                assert_eq!(specification_id.as_str(), "spec-api-42");
                assert_eq!(criterion_id.as_str(), "ac-xyz");
                assert_eq!(kind, CriterionKind::Compile);
                assert!(!message.is_empty());
                assert!(evidence.iter().any(|e| e.label == "compile_status"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn fail_compile_observation_drives_repair_diagnostic() {
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &compile_fail_evidence());
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-1"), &evaluation);

        let mut ctx = AgentContext::new("eval-bridge");
        ctx.push_observation(observation);
        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::RepairDiagnostic { .. }));
    }

    #[test]
    fn pass_observation_does_not_trigger_repair_diagnostic() {
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &compile_pass_evidence());
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-1"), &evaluation);

        let mut ctx = AgentContext::new("eval-bridge");
        ctx.push_observation(observation);
        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let action = agent.propose(&ctx);
        assert!(matches!(&action, AgentAction::Finish { summary } if summary.contains("pass")));
        assert!(!matches!(&action, AgentAction::RepairDiagnostic { .. }));
    }

    #[test]
    fn insufficient_evidence_observation_is_not_pass() {
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &[]);
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-1"), &evaluation);

        let mut ctx = AgentContext::new("eval-bridge");
        ctx.push_observation(observation.clone());
        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let action = agent.propose(&ctx);
        assert!(matches!(
            action,
            AgentAction::Finish { summary } if summary.contains("insufficient")
        ));
        assert!(!observation.is_evaluation_pass());
    }

    #[test]
    fn multiple_criteria_preserve_identities_in_specification_observation() {
        let spec = Specification::new("spec-multi", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-1", "calidad")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-1")]),
                AcceptanceCriterion::new("ac-validate", "valida", CriterionKind::Validate)
                    .satisfying([crate::harness::RequirementId::new("req-1")]),
                AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests)
                    .satisfying([crate::harness::RequirementId::new("req-1")]),
            ]);

        let mut evidence = compile_pass_evidence();
        evidence.extend(validate_pass_evidence());
        evidence.extend(tests_fail_evidence());

        let aggregated = EvaluationEngine::new().evaluate_specification(&spec, &evidence);
        assert_eq!(aggregated.status, SpecificationEvaluationStatus::Fail);
        let observation = observation_from_specification_evaluation(&aggregated);
        match observation {
            AgentObservation::SpecificationEvaluated {
                specification_id,
                status,
                criteria,
                ..
            } => {
                assert_eq!(specification_id.as_str(), "spec-multi");
                assert_eq!(status, SpecificationEvaluationStatus::Fail);
                assert_eq!(criteria.len(), 3);
                assert_eq!(criteria[0].criterion_id.as_str(), "ac-compile");
                assert_eq!(criteria[0].kind, CriterionKind::Compile);
                assert_eq!(criteria[0].verdict, EvaluationVerdict::Pass);
                assert_eq!(criteria[1].criterion_id.as_str(), "ac-validate");
                assert_eq!(criteria[1].verdict, EvaluationVerdict::Pass);
                assert_eq!(criteria[2].criterion_id.as_str(), "ac-tests");
                assert_eq!(criteria[2].kind, CriterionKind::RunTests);
                assert_eq!(criteria[2].verdict, EvaluationVerdict::Fail);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn changing_criterion_id_does_not_change_kind_in_observation() {
        let engine = EvaluationEngine::new();
        let a = engine.evaluate_criterion(
            &AcceptanceCriterion::new("id-a", "x", CriterionKind::Compile),
            &compile_pass_evidence(),
        );
        let b = engine.evaluate_criterion(
            &AcceptanceCriterion::new("id-b", "y", CriterionKind::Compile),
            &compile_pass_evidence(),
        );
        let obs_a = observation_from_criterion_evaluation(SpecificationId::new("spec"), &a);
        let obs_b = observation_from_criterion_evaluation(SpecificationId::new("spec"), &b);
        assert_ne!(a.criterion_id, b.criterion_id);
        assert_eq!(a.kind, b.kind);
        assert_eq!(obs_a.evaluation_kind(), Some(CriterionKind::Compile));
        assert_eq!(obs_b.evaluation_kind(), Some(CriterionKind::Compile));
        assert_eq!(obs_a.evaluation_verdict(), obs_b.evaluation_verdict());
    }

    #[test]
    fn changing_kind_changes_evaluation_and_observation_semantics() {
        let engine = EvaluationEngine::new();
        let mut evidence = compile_pass_evidence();
        evidence.extend(validate_fail_evidence_local());

        let as_compile = engine.evaluate_criterion(
            &AcceptanceCriterion::new("shared", "c", CriterionKind::Compile),
            &evidence,
        );
        let as_validate = engine.evaluate_criterion(
            &AcceptanceCriterion::new("shared", "c", CriterionKind::Validate),
            &evidence,
        );
        let obs_compile =
            observation_from_criterion_evaluation(SpecificationId::new("spec"), &as_compile);
        let obs_validate =
            observation_from_criterion_evaluation(SpecificationId::new("spec"), &as_validate);

        assert_eq!(obs_compile.evaluation_kind(), Some(CriterionKind::Compile));
        assert_eq!(
            obs_validate.evaluation_kind(),
            Some(CriterionKind::Validate)
        );
        assert_eq!(
            obs_compile.evaluation_verdict(),
            Some(EvaluationVerdict::Pass)
        );
        assert_eq!(
            obs_validate.evaluation_verdict(),
            Some(EvaluationVerdict::Fail)
        );
    }

    fn validate_fail_evidence_local() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", VALIDATE),
            Evidence::new("validate_status", "error"),
        ]
    }

    #[test]
    fn e2e_specification_to_observation_affects_agent_decision() {
        let spec = Specification::new("spec-e2e", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-compile", "debe compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-001", "El código compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-compile")]),
            ]);

        let planned = plan_specification(&spec).expect("plan");
        assert_eq!(planned.specification_id, spec.id);

        let artifact_code = "fn main() { println!(\"broken\"";
        let tool_result = CompileTool.execute(artifact_code, &AgentContext::new("e2e-eval"));
        assert!(!tool_result.success);

        let evaluation = EvaluationEngine::new()
            .evaluate_criterion(&spec.acceptance_criteria[0], &tool_result.evidence);
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
        assert_eq!(evaluation.kind, CriterionKind::Compile);

        let observation = observation_from_criterion_evaluation(spec.id.clone(), &evaluation);
        let mut ctx = AgentContext::new("e2e-eval").with_working_code(artifact_code);
        ctx.push_observation(observation);

        let mut agent = EvaluationAwareAgent::new(&spec.goal);
        let action = agent.propose(&ctx);
        assert!(
            matches!(&action, AgentAction::RepairDiagnostic { errors } if !errors.is_empty()),
            "FAIL compile observation debe causar RepairDiagnostic, got {action:?}"
        );
    }
}
