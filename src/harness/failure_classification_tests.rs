//! Tests A–K: clasificación de fallos autónomos y recovery acotado.

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use crate::harness::Evidence;
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::failure_classification::{
        FailureClass, RecoveryBudget, RecoveryStrategy, classify_model_error,
        select_recovery_strategy,
    };
    use crate::harness::goal_driven::{Goal, GoalProgressTracker, ProgressSignal};
    use crate::harness::live_session::build_validate_compile_harness_with_policy;
    use crate::harness::model::{
        AiSessionConfig, ModelClient, ModelDecision, ModelError, ModelRequest, ModelResponse,
        redact_secrets, serialize_decision,
    };
    use crate::harness::observation::AgentObservation;
    use crate::harness::runtime::Harness;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tools::{COMPILE, REPAIR_DIAGNOSTIC};

    fn compile_only_goal() -> Goal {
        Goal::from_specification(
            Specification::new("spec-compile", "compilar")
                .with_requirements(vec![Requirement::new("req-1", "compila")])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new(
                        "ac-compile",
                        "compila sin errores",
                        CriterionKind::Compile,
                    )
                    .satisfying([crate::harness::RequirementId::new("req-1")]),
                ]),
        )
    }

    fn compile_harness() -> Harness {
        build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy())
    }

    struct ScriptedModelClient {
        errors: Mutex<Vec<Option<ModelError>>>,
        success: ModelDecision,
    }

    impl ScriptedModelClient {
        fn new(errors: Vec<Option<ModelError>>, success: ModelDecision) -> Self {
            Self {
                errors: Mutex::new(errors),
                success,
            }
        }
    }

    impl ModelClient for ScriptedModelClient {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            let mut guard = self.errors.lock().expect("lock");
            if let Some(next) = guard.first().cloned() {
                guard.remove(0);
                if let Some(error) = next {
                    return Err(error);
                }
            }
            Ok(ModelResponse {
                raw_text: serialize_decision(&self.success),
            })
        }
    }

    /// TEST A — transient failure classification
    #[test]
    fn test_a_transient_failure_classification() {
        let evidence = classify_model_error(&ModelError::RateLimited("rate_limited".into()));
        assert_eq!(evidence.class, FailureClass::ExternalTransient);
        assert!(evidence.retryable);
        assert_eq!(evidence.http_status, Some(429));
        let strategy = select_recovery_strategy(&evidence, &RecoveryBudget::default());
        assert_eq!(strategy, RecoveryStrategy::RetryWithBackoff);
    }

    /// TEST B — recovery success then continue
    #[test]
    fn test_b_recovery_success() {
        let client = ScriptedModelClient::new(
            vec![Some(ModelError::RateLimited("rate_limited".into()))],
            ModelDecision::Finish {
                summary: "recovered ok".to_string(),
            },
        );
        let session = AiSessionConfig::new("meta", "Generic");
        let mut agent = AiAgent::new(Box::new(client), session);
        // Sin specification: Finish no es bloqueado por FinishConstraint.
        let ctx = AgentContext::new("recover");
        let harness = Harness::new(8);
        let result = AgentLoop::new(5)
            .with_recovery_budget(RecoveryBudget::new(3, Duration::ZERO))
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(
            result
                .history
                .evidence
                .iter()
                .any(|item| item.label == "failure_recovery"),
            "debe registrar recovery"
        );
        assert!(result.failure_report.is_none());
    }

    /// TEST C — recovery exhausted → ExternalServiceBlocked
    #[test]
    fn test_c_recovery_exhausted() {
        let client = ScriptedModelClient::new(
            vec![
                Some(ModelError::RateLimited("rate_limited".into())),
                Some(ModelError::RateLimited("rate_limited".into())),
                Some(ModelError::RateLimited("rate_limited".into())),
                Some(ModelError::RateLimited("rate_limited".into())),
            ],
            ModelDecision::Finish {
                summary: "should not reach".to_string(),
            },
        );
        let session = AiSessionConfig::new("meta", "Generic");
        let mut agent = AiAgent::new(Box::new(client), session);
        let ctx = AgentContext::new("exhausted");
        let harness = Harness::new(10);
        let result = AgentLoop::new(10)
            .with_recovery_budget(RecoveryBudget::new(3, Duration::ZERO))
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::ExternalServiceBlocked);
        let report = result.failure_report.expect("report");
        assert_eq!(report.classification, FailureClass::ExternalTransient);
        assert_eq!(report.strategy, RecoveryStrategy::StopExternalBlocked);
        assert_eq!(report.recovery_attempts, 3);
        assert!(!report.recovery_restored_progress);
        assert!(result.iterations <= 5);
    }

    /// TEST D — permanent external failure (no meaningless retry)
    #[test]
    fn test_d_permanent_external_failure() {
        let client = ScriptedModelClient::new(
            vec![Some(ModelError::Authentication("forbidden".into()))],
            ModelDecision::Finish {
                summary: "unreachable".to_string(),
            },
        );
        let session = AiSessionConfig::new("meta", "Generic");
        let mut agent = AiAgent::new(Box::new(client), session);
        let ctx = AgentContext::new("auth");
        let harness = Harness::new(5);
        let result = AgentLoop::new(5)
            .with_recovery_budget(RecoveryBudget::new(3, Duration::ZERO))
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::ExternalConfigurationBlocked);
        let report = result.failure_report.expect("report");
        assert_eq!(report.classification, FailureClass::ExternalPermanent);
        assert!(!report.retryable);
        assert_eq!(report.recovery_attempts, 0);
        assert!(
            !result
                .history
                .evidence
                .iter()
                .any(|item| item.label == "failure_recovery")
        );
    }

    /// TEST E — model capability failure (API ok, no progress)
    #[test]
    fn test_e_model_capability_failure() {
        struct RepeatRepair;
        impl Agent for RepeatRepair {
            fn propose(&mut self, _ctx: &AgentContext) -> AgentAction {
                AgentAction::RepairDiagnostic {
                    errors: vec!["same".to_string()],
                }
            }
        }

        let goal = compile_only_goal();
        let harness = compile_harness();
        let mut agent = RepeatRepair;
        let ctx = AgentContext::new("capability")
            .with_working_code("fn main() { broken")
            .with_evaluation_specification(goal.specification.clone());

        let result = AgentLoop::new(8)
            .with_max_stale_iterations(3)
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::ModelCapabilityFailure);
        assert_eq!(
            result.failure_report.expect("report").classification,
            FailureClass::ModelCapability
        );
    }

    /// TEST F — system failure must not blame the model
    #[test]
    fn test_f_system_failure() {
        struct UnknownToolAgent;
        impl Agent for UnknownToolAgent {
            fn propose(&mut self, _ctx: &AgentContext) -> AgentAction {
                AgentAction::InvokeTool {
                    tool_name: "not_registered_tool".to_string(),
                    input: String::new(),
                }
            }
        }

        let mut agent = UnknownToolAgent;
        let ctx = AgentContext::new("system");
        let harness = Harness::new(3);
        let result = AgentLoop::new(3).run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::SystemFailure);
        let report = result.failure_report.expect("report");
        assert_eq!(report.classification, FailureClass::SystemFailure);
        assert_ne!(report.classification, FailureClass::ModelCapability);
    }

    /// TEST G — unknown stall → ConvergenceStalled / NonProgress
    #[test]
    fn test_g_unknown_stall() {
        struct SilentNoOp;
        impl Agent for SilentNoOp {
            fn propose(&mut self, _ctx: &AgentContext) -> AgentAction {
                AgentAction::NoOp
            }
        }

        let goal = compile_only_goal();
        let mut agent = SilentNoOp;
        let ctx = AgentContext::new("stall")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(goal.specification.clone());
        let harness = Harness::new(8);

        let result = AgentLoop::new(8)
            .with_max_stale_iterations(3)
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::NonProgress);
        let report = result.failure_report.expect("report");
        assert_eq!(report.classification, FailureClass::ConvergenceStalled);
    }

    /// TEST H — progress without goal satisfaction clears stale
    #[test]
    fn test_h_progress_without_satisfaction() {
        let mut tracker = GoalProgressTracker::new();
        let goal = Goal::from_specification(
            Specification::new("spec-multi", "multi")
                .with_requirements(vec![
                    Requirement::new("req-1", "compila"),
                    Requirement::new("req-2", "valida"),
                ])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                        .satisfying([crate::harness::RequirementId::new("req-1")]),
                    AcceptanceCriterion::new("ac-validate", "valida", CriterionKind::Validate)
                        .satisfying([crate::harness::RequirementId::new("req-2")]),
                ]),
        );
        let open = crate::harness::goal_driven::GoalEvaluator::new().evaluate(&goal, &[]);
        let improved = crate::harness::goal_driven::GoalEvaluator::new().evaluate(
            &goal,
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
        );
        assert_eq!(
            tracker
                .record_iteration(&open, Some(REPAIR_DIAGNOSTIC), Some(0))
                .signal,
            ProgressSignal::Unchanged
        );
        let assessment = tracker.record_iteration(&improved, Some(COMPILE), Some(1));
        assert_eq!(assessment.signal, ProgressSignal::Improved);
        assert_eq!(tracker.stale_iterations(), 0);
        assert_ne!(
            improved.status,
            crate::harness::goal_driven::GoalStatus::Satisfied
        );
    }

    /// TEST I — goal satisfied / successful completion
    #[test]
    fn test_i_goal_satisfied() {
        let client = ScriptedModelClient::new(
            vec![],
            ModelDecision::Finish {
                summary: "done".to_string(),
            },
        );
        let session = AiSessionConfig::new("meta", "Generic");
        let mut agent = AiAgent::new(Box::new(client), session);
        let ctx = AgentContext::new("ok");
        let harness = Harness::new(3);
        let result = AgentLoop::new(3).run(&harness, &mut agent, ctx);
        assert_eq!(result.status, LoopStatus::Completed);
        assert!(result.failure_report.is_none());
    }

    /// TEST J — no secret leakage in terminal report
    #[test]
    fn test_j_no_secret_leakage() {
        let client = ScriptedModelClient::new(
            vec![Some(ModelError::Transport(
                "authorization: Bearer super-secret-key-xyz".into(),
            ))],
            ModelDecision::Finish {
                summary: "unreachable".to_string(),
            },
        );
        let session = AiSessionConfig::new("meta", "Generic");
        let mut agent = AiAgent::new(Box::new(client), session);
        let ctx = AgentContext::new("secrets");
        let harness = Harness::new(3);
        let result = AgentLoop::new(3)
            .with_recovery_budget(RecoveryBudget::new(1, Duration::ZERO))
            .run(&harness, &mut agent, ctx);

        let blob = format!(
            "{}{:?}",
            result.termination_reason,
            result
                .failure_report
                .as_ref()
                .map(|r| r.terminal_explanation())
        );
        assert!(!blob.contains("super-secret-key-xyz"));
        assert!(
            blob.contains("[REDACTED]")
                || redact_secrets("authorization: Bearer x").contains("[REDACTED]")
        );
        // Transport is non-retryable at ModelError level → terminal without recover spam.
        assert!(
            result.status == LoopStatus::ExternalServiceBlocked
                || result.status == LoopStatus::ExternalConfigurationBlocked
                || result.status == LoopStatus::Failed
        );
    }

    /// TEST K — rate-limit path with FinishConstraint (live-like) → not ModelCapability
    #[test]
    fn test_k_rate_limit_rejected_finish_not_model_capability() {
        let client = ScriptedModelClient::new(
            vec![
                Some(ModelError::RateLimited("rate_limited".into())),
                Some(ModelError::RateLimited("rate_limited".into())),
                Some(ModelError::RateLimited("rate_limited".into())),
                Some(ModelError::RateLimited("rate_limited".into())),
            ],
            ModelDecision::Compile {
                code: "fn main() {}".to_string(),
            },
        );
        let goal = compile_only_goal();
        let session = AiSessionConfig::new("compilar", "Generic");
        let mut agent = AiAgent::new(Box::new(client), session);
        let mut ctx = AgentContext::new("live-like")
            .with_working_code("fn main() { broken")
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

        let harness = compile_harness();
        let result = AgentLoop::new(10)
            .with_max_stale_iterations(3)
            .with_recovery_budget(RecoveryBudget::new(3, Duration::ZERO))
            .run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::ExternalServiceBlocked);
        let report = result.failure_report.expect("report");
        assert_eq!(report.classification, FailureClass::ExternalTransient);
        assert_ne!(report.classification, FailureClass::ModelCapability);
        assert_ne!(result.status, LoopStatus::NonProgress);
        assert_ne!(result.status, LoopStatus::ModelCapabilityFailure);
    }
}
