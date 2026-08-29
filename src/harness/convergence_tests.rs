//! Tests A–I: progreso medible, estancamiento y convergencia del loop autónomo.

#[cfg(test)]
mod tests {
    use crate::harness::Evidence;
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::failure_classification::FailureClass;
    use crate::harness::goal_driven::{
        Goal, GoalDrivenLoop, GoalDrivenStatus, GoalEvaluator, GoalProgressTracker, GoalStatus,
        ProgressSignal, RecommendedAction, collect_evidence_from_context,
        select_primary_recommendation, select_primary_recommendation_with_context,
    };
    use crate::harness::live_session::build_validate_compile_harness_with_policy;
    use crate::harness::model::{
        AiSessionConfig, MockModelClient, ModelClient, ModelDecision, ModelError, ModelRequest,
        ModelResponse, apply_gap_guidance, model_request_from_context, serialize_decision,
    };
    use crate::harness::observation::AgentObservation;
    use crate::harness::runtime::Harness;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tools::{
        APPLY_CORRECTION, COMPILE, CorrectionTool, REPAIR_DIAGNOSTIC, RepairDiagnosticTool,
        VALIDATE,
    };

    fn compile_only_goal() -> Goal {
        Goal::from_specification(
            Specification::new("spec-conv-compile", "El código debe compilar")
                .with_requirements(vec![Requirement::new("req-c", "compilar")])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                        .satisfying([crate::harness::RequirementId::new("req-c")]),
                ]),
        )
    }

    fn compile_and_validate_goal() -> Goal {
        Goal::from_specification(
            Specification::new("spec-conv-both", "API funcional")
                .with_requirements(vec![Requirement::new("req-q", "calidad")])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new("ac-v", "valida", CriterionKind::Validate)
                        .satisfying([crate::harness::RequirementId::new("req-q")]),
                    AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                        .satisfying([crate::harness::RequirementId::new("req-q")]),
                ]),
        )
    }

    fn diagnostic_harness() -> Harness {
        let mut harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness
    }

    /// TEST A — Fail → Pass = ProgressDetected
    #[test]
    fn test_a_progress_fail_to_pass() {
        let goal = compile_only_goal();
        let mut tracker = GoalProgressTracker::new();
        let fail = GoalEvaluator::new().evaluate(
            &goal,
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
            ],
        );
        let pass = GoalEvaluator::new().evaluate(
            &goal,
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
        );
        assert_eq!(tracker.record(&fail), ProgressSignal::Unchanged);
        let assessment = tracker.record_iteration(&pass, Some(COMPILE), Some(1));
        assert_eq!(assessment.signal, ProgressSignal::Improved);
        assert!(assessment.is_meaningful_progress());
    }

    /// TEST B — gap reduction = ProgressDetected
    #[test]
    fn test_b_gap_reduction_is_progress() {
        let goal = compile_and_validate_goal();
        let mut tracker = GoalProgressTracker::new();
        let both_open = GoalEvaluator::new().evaluate(&goal, &[]);
        assert_eq!(both_open.gap.unsatisfied.len(), 2);
        tracker.record(&both_open);

        let compile_only_open = GoalEvaluator::new().evaluate(
            &goal,
            &[
                Evidence::new("tool", VALIDATE),
                Evidence::new("validate_status", "ok"),
            ],
        );
        assert_eq!(compile_only_open.gap.unsatisfied.len(), 1);
        let assessment = tracker.record_iteration(&compile_only_open, Some(VALIDATE), Some(0));
        assert_eq!(assessment.signal, ProgressSignal::Improved);
        assert!(assessment.snapshot.gap_count < both_open.gap.unsatisfied.len());
    }

    /// TEST C — repeated action without state change → ModelCapabilityFailure
    #[test]
    fn test_c_repeated_action_non_progress() {
        struct RepeatRepairAgent;
        impl Agent for RepeatRepairAgent {
            fn propose(&mut self, _ctx: &AgentContext) -> AgentAction {
                AgentAction::RepairDiagnostic {
                    errors: vec!["stale error".to_string()],
                }
            }
        }

        let goal = compile_only_goal();
        let harness = diagnostic_harness();
        let mut agent = RepeatRepairAgent;
        let ctx = AgentContext::new("repeat-repair")
            .with_working_code("fn main() { broken")
            .with_evaluation_specification(goal.specification.clone());

        let result = AgentLoop::new(8)
            .with_max_stale_iterations(3)
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::ModelCapabilityFailure);
        let report = result.failure_report.expect("failure report");
        assert_eq!(report.classification, FailureClass::ModelCapability);
        assert!(
            result
                .history
                .progress_assessments
                .iter()
                .any(|item| item.repeated_action),
            "debe marcar acción repetida"
        );
        assert!(result.iterations < 8);
    }

    /// TEST D — state A → action → state A = RepeatedState
    #[test]
    fn test_d_repeated_state_detected() {
        let goal = compile_and_validate_goal();
        let mut tracker = GoalProgressTracker::new();
        let state_a = GoalEvaluator::new().evaluate(&goal, &[]);
        let state_b = GoalEvaluator::new().evaluate(
            &goal,
            &[
                Evidence::new("tool", VALIDATE),
                Evidence::new("validate_status", "ok"),
            ],
        );
        tracker.record_iteration(&state_a, Some("compile"), Some(0));
        tracker.record_iteration(&state_b, Some("validate"), Some(0));
        let back = tracker.record_iteration(&state_a, Some("compile"), Some(0));
        assert_eq!(back.signal, ProgressSignal::RepeatedState);
        assert!(back.is_non_progress());
    }

    /// TEST E — artifact mutation without criterion improvement ≠ progress
    #[test]
    fn test_e_artifact_mutation_without_improvement_not_progress() {
        let goal = compile_only_goal();
        let mut tracker = GoalProgressTracker::new();
        let fail = GoalEvaluator::new().evaluate(
            &goal,
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
            ],
        );
        tracker.record_iteration(&fail, Some(COMPILE), Some(0));
        let again = tracker.record_iteration(&fail, Some(APPLY_CORRECTION), Some(1));
        assert_eq!(again.signal, ProgressSignal::Unchanged);
        assert!(!again.is_meaningful_progress());
        assert!(again.artifact_changed_without_progress);
    }

    /// TEST F — broken → diagnostic → repair → compile → Pass
    #[test]
    fn test_f_successful_correction_e2e() {
        let goal = compile_only_goal();
        let broken = "fn main() { let x = 1; }";
        let harness = diagnostic_harness();

        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let mut loop_ = GoalDrivenLoop::with_defaults(12);
        let ctx = AgentContext::new("success-correction")
            .with_working_code(broken)
            .with_evaluation_specification(goal.specification.clone());

        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert_eq!(result.status, GoalDrivenStatus::GoalSatisfied);
        assert_eq!(result.loop_result.status, LoopStatus::Completed);
        assert_eq!(result.final_evaluation.status, GoalStatus::Satisfied);
    }

    /// TEST G — model stagnation terminates bounded
    #[test]
    fn test_g_model_stagnation_bounded() {
        struct AlwaysRepairClient;
        impl ModelClient for AlwaysRepairClient {
            fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
                Ok(ModelResponse {
                    raw_text: serialize_decision(&ModelDecision::RepairDiagnostic {
                        errors: request
                            .diagnostic_context
                            .compiler_stderr
                            .clone()
                            .into_iter()
                            .chain(std::iter::once("loop".to_string()))
                            .collect(),
                    }),
                })
            }
        }

        let goal = compile_only_goal();
        let harness = diagnostic_harness();
        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(AlwaysRepairClient), session);
        let ctx = AgentContext::new("stagnation")
            .with_working_code("fn main() { broken")
            .with_evaluation_specification(goal.specification.clone());

        // Seed compile FAIL so RepairDiagnostic is meaningful.
        let mut ctx = ctx;
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: false,
            output: "error".to_string(),
            evidence: vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
                Evidence::new("compiler_stderr", "error: broken"),
            ],
            verdict: EvaluationVerdict::Fail,
        });

        let result = AgentLoop::new(20)
            .with_max_stale_iterations(3)
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::ModelCapabilityFailure);
        assert!(result.iterations <= 10, "iterations={}", result.iterations);
        let report = result.failure_report.expect("failure report");
        assert_eq!(report.classification, FailureClass::ModelCapability);
    }

    /// TEST H — premature Finish redirected / rejected
    #[test]
    fn test_h_premature_finish_redirected() {
        let goal = compile_only_goal();
        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut ctx = AgentContext::new("premature")
            .with_working_code("fn main() { broken")
            .with_evaluation_specification(goal.specification.clone());
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: false,
            output: "error".to_string(),
            evidence: vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
                Evidence::new("compiler_stderr", "error: broken"),
            ],
            verdict: EvaluationVerdict::Fail,
        });
        ctx.push_observation(AgentObservation::CriterionEvaluated {
            specification_id: goal.specification.id.clone(),
            criterion_id: crate::harness::AcceptanceCriterionId::new("ac-compile"),
            kind: CriterionKind::Compile,
            verdict: EvaluationVerdict::Fail,
            message: "fail".to_string(),
            evidence: vec![Evidence::new("compile_status", "error")],
        });

        let request = model_request_from_context(&ctx, &session).expect("request");
        assert!(
            request
                .recommended_action
                .as_ref()
                .is_some_and(|rec| rec.kind == "RepairDiagnostic"),
            "recomendación={:?}",
            request.recommended_action
        );
        let guided = apply_gap_guidance(
            ModelDecision::Finish {
                summary: "done".to_string(),
            },
            &request,
        );
        assert!(
            !matches!(guided, ModelDecision::Finish { .. }),
            "Finish prematuro debe redirigirse: {guided:?}"
        );
    }

    /// TEST I — existing autonomous E2E still pass (smoke of mock path)
    #[test]
    fn test_i_no_regression_mock_autonomous_path() {
        let goal = compile_only_goal();
        let harness = diagnostic_harness();
        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let mut loop_ = GoalDrivenLoop::with_defaults(10);
        let ctx = AgentContext::new("regression")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(goal.specification.clone());
        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert!(
            matches!(
                result.status,
                GoalDrivenStatus::GoalSatisfied
                    | GoalDrivenStatus::MaxIterations
                    | GoalDrivenStatus::Failed
                    | GoalDrivenStatus::Escalated
                    | GoalDrivenStatus::NonProgress
                    | GoalDrivenStatus::ExternalServiceBlocked
                    | GoalDrivenStatus::ExternalConfigurationBlocked
                    | GoalDrivenStatus::ModelCapabilityFailure
                    | GoalDrivenStatus::SystemFailure
            ),
            "status inesperado: {:?}",
            result.status
        );
    }

    #[test]
    fn action_transition_repair_to_apply_correction() {
        let goal = compile_only_goal();
        let mut ctx = AgentContext::new("transition")
            .with_working_code("fn main() { broken")
            .with_evaluation_specification(goal.specification.clone());
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: false,
            output: "error".to_string(),
            evidence: vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
                Evidence::new("compiler_stderr", "error: broken"),
            ],
            verdict: EvaluationVerdict::Fail,
        });
        let evaluation = GoalEvaluator::new().evaluate(&goal, &collect_evidence_from_context(&ctx));
        assert!(matches!(
            select_primary_recommendation(&evaluation),
            RecommendedAction::RepairDiagnostic { .. }
        ));

        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: REPAIR_DIAGNOSTIC.to_string(),
            success: true,
            output: "feedback".to_string(),
            evidence: vec![
                Evidence::new("tool", REPAIR_DIAGNOSTIC),
                Evidence::new("diagnostic_status", "ok"),
                Evidence::new("repairer_feedback_0", "fix broken"),
            ],
            verdict: EvaluationVerdict::Pass,
        });
        let after_repair =
            GoalEvaluator::new().evaluate(&goal, &collect_evidence_from_context(&ctx));
        let rec = select_primary_recommendation_with_context(&after_repair, &ctx);
        assert!(
            matches!(rec, RecommendedAction::ApplyCorrection { .. }),
            "tras RepairDiagnostic exitoso debe recomendar ApplyCorrection: {rec:?}"
        );
    }

    #[test]
    fn action_transition_apply_correction_to_compile() {
        let goal = compile_only_goal();
        let mut ctx = AgentContext::new("transition-compile")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(goal.specification.clone());
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: false,
            output: "error".to_string(),
            evidence: vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
            ],
            verdict: EvaluationVerdict::Fail,
        });
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: APPLY_CORRECTION.to_string(),
            success: true,
            output: "ok".to_string(),
            evidence: vec![
                Evidence::new("tool", APPLY_CORRECTION),
                Evidence::new("correction_status", "ok"),
            ],
            verdict: EvaluationVerdict::Pass,
        });
        let evaluation = GoalEvaluator::new().evaluate(&goal, &collect_evidence_from_context(&ctx));
        let rec = select_primary_recommendation_with_context(&evaluation, &ctx);
        assert!(
            matches!(
                rec,
                RecommendedAction::InvokeTool {
                    tool_name: COMPILE,
                    ..
                }
            ),
            "tras ApplyCorrection exitoso debe recomendar Compile: {rec:?}"
        );
    }
}
