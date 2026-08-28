//! Tests de ActionPolicy / ActionConstraint (permiso ≠ validez de acción).

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::{
        ActionPolicy, ApplyCorrectionConstraint, ArtifactStateConstraint, FinishConstraint,
        PolicyVerdict, RepairDiagnosticConstraint,
    };
    use crate::harness::agent::{Agent, FirstActionAgent};
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::constraint::{Constraint, ConstraintDecision};
    use crate::harness::context::AgentContext;
    use crate::harness::correction::{Correction, CorrectionOperation, CorrectionTarget};
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::{EvaluationVerdict, Evidence};
    use crate::harness::evaluation_engine::EvaluationEngine;
    use crate::harness::evaluation_observation::observation_from_criterion_evaluation;
    use crate::harness::model::{AiSessionConfig, MockModelClient};
    use crate::harness::observation::AgentObservation;
    use crate::harness::runtime::Harness;
    use crate::harness::specification::{
        AcceptanceCriterion, Requirement, Specification, SpecificationId,
    };
    use crate::harness::tool::Tool;
    use crate::harness::tool_permission::ToolPermissionConstraint;
    use crate::harness::tools::{COMPILE, CompileTool};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn compile_spec() -> Specification {
        Specification::new("spec-finish", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    fn artifact_ctx(source: &str) -> AgentContext {
        AgentContext::new("policy").with_working_artifact(RustArtifact::with_id(
            ArtifactId::new("art-policy"),
            "main.rs",
            source,
        ))
    }

    #[test]
    fn permission_allows_authorized_tool() {
        // A
        let constraint = ToolPermissionConstraint::default_constructor_tools();
        let decision = constraint.check(
            &AgentAction::Compile {
                code: "fn main() {}".to_string(),
            },
            &AgentContext::new("a"),
        );
        assert_eq!(decision, ConstraintDecision::Allow);
    }

    #[test]
    fn permission_rejects_unauthorized_tool() {
        // B
        let constraint = ToolPermissionConstraint::default_constructor_tools();
        let decision = constraint.check(
            &AgentAction::InvokeTool {
                tool_name: "echo".to_string(),
                input: "x".to_string(),
            },
            &AgentContext::new("b"),
        );
        assert!(matches!(decision, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn compile_without_artifact_is_rejected() {
        // C
        let decision = ArtifactStateConstraint.check(
            &AgentAction::Compile {
                code: "fn main() {}".to_string(),
            },
            &AgentContext::new("c"),
        );
        assert!(
            matches!(decision, ConstraintDecision::Reject { reason } if reason.contains("ausente"))
        );
    }

    #[test]
    fn validate_without_artifact_is_rejected() {
        // D
        let decision = ArtifactStateConstraint.check(
            &AgentAction::Validate {
                request: "Crear una API REST".to_string(),
                code: Some("fn main() {}".to_string()),
                plan_kind: "Api".to_string(),
            },
            &AgentContext::new("d"),
        );
        assert!(matches!(decision, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn correction_without_artifact_is_rejected() {
        // E
        let decision = ArtifactStateConstraint.check(
            &AgentAction::ApplyCorrection {
                corrections: vec![Correction::replace_session_text("a", "b")],
            },
            &AgentContext::new("e"),
        );
        assert!(matches!(decision, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn repair_diagnostic_without_errors_is_rejected() {
        // F
        let decision = RepairDiagnosticConstraint.check(
            &AgentAction::RepairDiagnostic { errors: vec![] },
            &artifact_ctx("fn main() {}"),
        );
        assert!(matches!(decision, ConstraintDecision::Reject { .. }));
        let blank = RepairDiagnosticConstraint.check(
            &AgentAction::RepairDiagnostic {
                errors: vec!["  ".to_string()],
            },
            &artifact_ctx("fn main() {}"),
        );
        assert!(matches!(blank, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn invalid_apply_correction_is_rejected() {
        // G
        let ctx = artifact_ctx("abc");
        let decision = ApplyCorrectionConstraint.check(
            &AgentAction::ApplyCorrection {
                corrections: vec![Correction {
                    target: CorrectionTarget::SessionCode,
                    path: None,
                    operation: CorrectionOperation::ReplaceText {
                        search: "zzz".to_string(),
                        replacement: "y".to_string(),
                    },
                }],
            },
            &ctx,
        );
        assert!(matches!(decision, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn finish_with_insufficient_evidence_is_rejected() {
        // H
        let mut ctx = artifact_ctx("fn main() {}").with_evaluation_specification(compile_spec());
        let evaluation = EvaluationEngine::new().evaluate_criterion(
            &compile_spec().acceptance_criteria[0],
            &[Evidence::new("tool", COMPILE)],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::InsufficientEvidence);
        ctx.push_observation(observation_from_criterion_evaluation(
            SpecificationId::new("spec-finish"),
            &evaluation,
        ));
        let decision = FinishConstraint.check(
            &AgentAction::Finish {
                summary: "done".to_string(),
            },
            &ctx,
        );
        assert!(
            matches!(decision, ConstraintDecision::Reject { reason } if reason.contains("InsufficientEvidence"))
        );
    }

    #[test]
    fn finish_with_fail_is_rejected() {
        // I
        let mut ctx = artifact_ctx("fn main() {}").with_evaluation_specification(compile_spec());
        let evaluation = EvaluationEngine::new().evaluate_criterion(
            &compile_spec().acceptance_criteria[0],
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
            ],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
        ctx.push_observation(observation_from_criterion_evaluation(
            SpecificationId::new("spec-finish"),
            &evaluation,
        ));
        let decision = FinishConstraint.check(
            &AgentAction::Finish {
                summary: "done".to_string(),
            },
            &ctx,
        );
        assert!(
            matches!(decision, ConstraintDecision::Reject { reason } if reason.contains("FAIL"))
        );
    }

    #[test]
    fn finish_with_required_criteria_pass_is_allowed() {
        // J
        let mut ctx = artifact_ctx("fn main() {}").with_evaluation_specification(compile_spec());
        let evaluation = EvaluationEngine::new().evaluate_criterion(
            &compile_spec().acceptance_criteria[0],
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
        ctx.push_observation(observation_from_criterion_evaluation(
            SpecificationId::new("spec-finish"),
            &evaluation,
        ));
        let decision = FinishConstraint.check(
            &AgentAction::Finish {
                summary: "done".to_string(),
            },
            &ctx,
        );
        assert_eq!(decision, ConstraintDecision::Allow);
    }

    #[test]
    fn rejected_action_does_not_execute_tool_and_produces_observation() {
        // K + L
        let executed = Arc::new(AtomicUsize::new(0));
        struct CountingCompile {
            calls: Arc<AtomicUsize>,
        }
        impl Tool for CountingCompile {
            fn name(&self) -> &str {
                COMPILE
            }
            fn execute(
                &self,
                _input: &str,
                _ctx: &AgentContext,
            ) -> crate::harness::tool::ToolResult {
                self.calls.fetch_add(1, Ordering::SeqCst);
                crate::harness::tool::ToolResult::success("ok", vec![])
            }
        }

        let mut harness = Harness::new(5);
        harness.register_tool(Box::new(CountingCompile {
            calls: Arc::clone(&executed),
        }));
        harness.register_constraint(Box::new(ArtifactStateConstraint));

        let mut ctx = AgentContext::new("kl");
        let outcome = harness.execute_step(
            AgentAction::Compile {
                code: "fn main() {}".to_string(),
            },
            &mut ctx,
        );
        assert!(!outcome.permitted);
        assert!(!outcome.tool_executed);
        assert_eq!(executed.load(Ordering::SeqCst), 0);
        assert!(matches!(
            outcome.observation,
            AgentObservation::ActionRejected {
                constraint,
                ..
            } if constraint == "artifact_state"
        ));
        assert_eq!(
            outcome.rejected_constraint.as_deref(),
            Some("artifact_state")
        );
    }

    #[test]
    fn agent_changes_decision_after_action_rejected() {
        // M — causalidad por Observation, no por iteration
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(ActionPolicy::default_session_policy()));

        let spec = compile_spec();
        let ctx = artifact_ctx("fn main() {}").with_evaluation_specification(spec);
        let mut agent = FirstActionAgent::new("fn main() {}");

        let first = agent.propose(&ctx);
        assert!(matches!(first, AgentAction::Finish { .. }));
        let mut ctx = ctx;
        let rejected = harness.execute_step(first, &mut ctx);
        assert!(!rejected.permitted);
        assert!(matches!(
            rejected.observation,
            AgentObservation::ActionRejected { .. }
        ));

        let second = agent.propose(&ctx);
        assert!(
            matches!(second, AgentAction::Compile { .. }),
            "segunda decisión debe venir de ActionRejected, no de iteración"
        );
    }

    #[test]
    fn action_policy_composes_constraints_with_short_circuit() {
        // N + O + P
        let later_calls = Arc::new(AtomicUsize::new(0));
        struct CountingAllow {
            name: &'static str,
            calls: Arc<AtomicUsize>,
        }
        impl Constraint for CountingAllow {
            fn name(&self) -> &str {
                self.name
            }
            fn check(&self, _action: &AgentAction, _ctx: &AgentContext) -> ConstraintDecision {
                self.calls.fetch_add(1, Ordering::SeqCst);
                ConstraintDecision::Allow
            }
        }
        struct AlwaysReject;
        impl Constraint for AlwaysReject {
            fn name(&self) -> &str {
                "always_reject"
            }
            fn check(&self, _action: &AgentAction, _ctx: &AgentContext) -> ConstraintDecision {
                ConstraintDecision::Reject {
                    reason: "blocked".to_string(),
                }
            }
        }

        let policy = ActionPolicy::new()
            .with_constraint(Box::new(AlwaysReject))
            .with_constraint(Box::new(CountingAllow {
                name: "later",
                calls: Arc::clone(&later_calls),
            }));

        let verdict = policy.decide(&AgentAction::NoOp, &AgentContext::new("n"));
        assert!(matches!(
            verdict,
            PolicyVerdict::Reject {
                constraint,
                ..
            } if constraint == "always_reject"
        ));
        assert_eq!(
            later_calls.load(Ordering::SeqCst),
            0,
            "short-circuit: constraints posteriores no deben ejecutarse"
        );
        assert!(matches!(
            policy.check(&AgentAction::NoOp, &AgentContext::new("n")),
            ConstraintDecision::Reject { .. }
        ));
    }

    #[test]
    fn reject_does_not_mutate_artifact_or_specification() {
        // Q + R
        let mut harness = Harness::new(5);
        harness.register_constraint(Box::new(ArtifactStateConstraint));
        let spec = compile_spec();
        let artifact = RustArtifact::with_id(ArtifactId::new("art-q"), "main.rs", "fn main() {}");
        let mut ctx = AgentContext::new("q")
            .with_working_artifact(artifact.clone())
            .with_evaluation_specification(spec.clone());
        // Forzar reject: ApplyCorrection inválida vía ApplyCorrectionConstraint + policy
        harness.register_constraint(Box::new(ApplyCorrectionConstraint));
        let before_artifact = ctx.working_artifact.clone();
        let before_spec = ctx.evaluation_specification.clone();
        let _ = harness.execute_step(
            AgentAction::ApplyCorrection {
                corrections: vec![Correction::replace_session_text("missing", "x")],
            },
            &mut ctx,
        );
        assert_eq!(ctx.working_artifact, before_artifact);
        assert_eq!(ctx.evaluation_specification, before_spec);
        assert_eq!(spec.id, before_spec.unwrap().id);
    }

    #[test]
    fn evaluation_engine_remains_independent_of_action_policy() {
        // S
        let evaluation = EvaluationEngine::new().evaluate_criterion(
            &AcceptanceCriterion::new("ac-1", "c", CriterionKind::Compile),
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn ai_agent_does_not_execute_tools() {
        // T
        let mut agent = AiAgent::new(
            Box::new(MockModelClient::new()),
            AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string()),
        );
        let action = agent.propose(&artifact_ctx("fn main() {}"));
        assert!(matches!(action, AgentAction::Validate { .. }));
    }

    #[test]
    fn e2e_invalid_action_reject_observation_valid_action_finish() {
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(ActionPolicy::default_session_policy()));

        let spec = compile_spec();
        let ctx = artifact_ctx("fn main() {}").with_evaluation_specification(spec);
        let mut agent = FirstActionAgent::new("fn main() {}");
        let result = AgentLoop::new(6).run(&harness, &mut agent, ctx);

        assert!(matches!(
            result.history.proposed_actions.first(),
            Some(AgentAction::Finish { .. })
        ));
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::ActionRejected {
                constraint,
                ..
            } if constraint == "finish"
        )));
        assert!(
            result
                .history
                .proposed_actions
                .iter()
                .any(|a| matches!(a, AgentAction::Compile { .. }))
        );
        assert!(result.tools_executed().iter().any(|t| t == COMPILE));
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Pass,
                ..
            }
        )));
        assert_eq!(result.status, LoopStatus::Completed);
        assert!(matches!(
            result.history.proposed_actions.last(),
            Some(AgentAction::Finish { summary }) if summary.contains("pass") || summary.contains("compile")
        ));
    }

    #[test]
    fn agent_loop_still_owns_max_iterations_under_policy() {
        struct SpamFinish;
        impl Agent for SpamFinish {
            fn propose(&mut self, _ctx: &AgentContext) -> AgentAction {
                AgentAction::Finish {
                    summary: "never allowed without pass".to_string(),
                }
            }
        }
        let mut harness = Harness::new(10);
        harness.register_constraint(Box::new(FinishConstraint));
        let ctx = artifact_ctx("fn main() {}").with_evaluation_specification(compile_spec());
        let result = AgentLoop::new(3).run(&harness, &mut SpamFinish, ctx);
        assert_eq!(result.status, LoopStatus::MaxIterations);
        assert_eq!(result.iterations, 3);
    }

    fn quality_spec() -> Specification {
        Specification::new("spec-quality", "Crear una API REST")
            .with_requirements(vec![
                Requirement::new("req-v", "validar"),
                Requirement::new("req-c", "compilar"),
                Requirement::new("req-t", "tests"),
                Requirement::new("req-l", "clippy"),
            ])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-validate", "valida", CriterionKind::Validate)
                    .satisfying([crate::harness::RequirementId::new("req-v")]),
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
                AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests)
                    .satisfying([crate::harness::RequirementId::new("req-t")]),
                AcceptanceCriterion::new("ac-clippy", "clippy", CriterionKind::Clippy)
                    .satisfying([crate::harness::RequirementId::new("req-l")]),
            ])
    }

    fn push_criterion(
        ctx: &mut AgentContext,
        criterion: &AcceptanceCriterion,
        evidence: &[Evidence],
    ) {
        let evaluation = EvaluationEngine::new().evaluate_criterion(criterion, evidence);
        ctx.push_observation(observation_from_criterion_evaluation(
            SpecificationId::new("spec-quality"),
            &evaluation,
        ));
    }

    #[test]
    fn finish_rejected_when_run_tests_is_fail() {
        // N
        let spec = quality_spec();
        let mut ctx = artifact_ctx("fn main() {}").with_evaluation_specification(spec.clone());
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[0],
            &[
                Evidence::new("tool", crate::harness::tools::VALIDATE),
                Evidence::new("validate_status", "ok"),
            ],
        );
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[1],
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
        );
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[2],
            &[
                Evidence::new("tool", crate::harness::tools::RUN_TESTS),
                Evidence::new("exit_status", "1"),
            ],
        );
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[3],
            &[
                Evidence::new("tool", crate::harness::tools::RUN_CLIPPY),
                Evidence::new("exit_status", "0"),
            ],
        );
        let decision = FinishConstraint.check(
            &AgentAction::Finish {
                summary: "done".to_string(),
            },
            &ctx,
        );
        assert!(
            matches!(decision, ConstraintDecision::Reject { reason } if reason.contains("FAIL"))
        );
    }

    #[test]
    fn finish_rejected_when_clippy_has_insufficient_evidence() {
        // O
        let spec = quality_spec();
        let mut ctx = artifact_ctx("fn main() {}").with_evaluation_specification(spec.clone());
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[0],
            &[
                Evidence::new("tool", crate::harness::tools::VALIDATE),
                Evidence::new("validate_status", "ok"),
            ],
        );
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[1],
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
        );
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[2],
            &[
                Evidence::new("tool", crate::harness::tools::RUN_TESTS),
                Evidence::new("exit_status", "0"),
            ],
        );
        // Clippy: tool presente sin exit_status → InsufficientEvidence
        push_criterion(
            &mut ctx,
            &spec.acceptance_criteria[3],
            &[Evidence::new("tool", crate::harness::tools::RUN_CLIPPY)],
        );
        let decision = FinishConstraint.check(
            &AgentAction::Finish {
                summary: "done".to_string(),
            },
            &ctx,
        );
        assert!(matches!(
            decision,
            ConstraintDecision::Reject { reason }
                if reason.contains("InsufficientEvidence") || reason.contains("evidencia insuficiente")
        ));
    }

    #[test]
    fn finish_allowed_only_when_all_required_criteria_pass() {
        // P
        let spec = quality_spec();
        let mut ctx = artifact_ctx("fn main() {}").with_evaluation_specification(spec.clone());
        for (criterion, evidence) in [
            (
                &spec.acceptance_criteria[0],
                vec![
                    Evidence::new("tool", crate::harness::tools::VALIDATE),
                    Evidence::new("validate_status", "ok"),
                ],
            ),
            (
                &spec.acceptance_criteria[1],
                vec![
                    Evidence::new("tool", COMPILE),
                    Evidence::new("compile_status", "ok"),
                ],
            ),
            (
                &spec.acceptance_criteria[2],
                vec![
                    Evidence::new("tool", crate::harness::tools::RUN_TESTS),
                    Evidence::new("exit_status", "0"),
                ],
            ),
            (
                &spec.acceptance_criteria[3],
                vec![
                    Evidence::new("tool", crate::harness::tools::RUN_CLIPPY),
                    Evidence::new("exit_status", "0"),
                ],
            ),
        ] {
            push_criterion(&mut ctx, criterion, &evidence);
        }
        let decision = FinishConstraint.check(
            &AgentAction::Finish {
                summary: "all pass".to_string(),
            },
            &ctx,
        );
        assert_eq!(decision, ConstraintDecision::Allow);
    }
}
