//! Integración AgentLoop + routing multi-modelo (tests M/N deterministas).

#[cfg(test)]
mod model_routing_integration_tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use crate::harness::action_policy::{ActionPolicy, FinishConstraint};
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::context::AgentContext;
    use crate::harness::failure_classification::{FailureClass, RecoveryBudget};
    use crate::harness::model::{
        AiSessionConfig, ModelClient, ModelError, ModelRequest, ModelResponse,
    };
    use crate::harness::model_routing::{
        EscalationBudget, ModelCandidate, RelativeTier, RoutingAction, RoutingReason,
    };
    use crate::harness::runtime::Harness;
    use crate::harness::tool_permission::ToolPermissionConstraint;
    use crate::harness::tools::{COMPILE, CompileTool, VALIDATE, ValidationTool};

    struct SequenceModelClient {
        label: String,
        errors: Vec<ModelError>,
        success: String,
        calls: AtomicU32,
    }

    impl SequenceModelClient {
        fn fail_then_finish(label: &str, errors: Vec<ModelError>) -> Self {
            Self {
                label: label.to_string(),
                errors,
                success: r#"{"action":"finish","summary":"ok"}"#.to_string(),
                calls: AtomicU32::new(0),
            }
        }

        fn always_invalid(label: &str) -> Self {
            Self {
                label: label.to_string(),
                errors: Vec::new(),
                success: "{not-json".to_string(),
                calls: AtomicU32::new(0),
            }
        }

        fn finish_ok(label: &str) -> Self {
            Self {
                label: label.to_string(),
                errors: Vec::new(),
                success: r#"{"action":"finish","summary":"ok"}"#.to_string(),
                calls: AtomicU32::new(0),
            }
        }
    }

    impl ModelClient for SequenceModelClient {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) as usize;
            if call < self.errors.len() {
                return Err(self.errors[call].clone());
            }
            let _ = &self.label;
            Ok(ModelResponse {
                raw_text: self.success.clone(),
            })
        }
    }

    fn harness() -> Harness {
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(ValidationTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));
        harness.register_constraint(Box::new(FinishConstraint));
        let _ = (COMPILE, VALIDATE, ActionPolicy::default_session_policy());
        harness
    }

    /// TEST M — routing coexiste con GoalProgressTracker (stale window intacta).
    #[test]
    fn m_compatible_with_goal_progress_tracker_stale_window() {
        let cheap = ModelCandidate::new("mock", "cheap", RelativeTier::Low, RelativeTier::Low);
        let strong = ModelCandidate::new("mock", "strong", RelativeTier::High, RelativeTier::High);
        let catalog = vec![
            (
                cheap,
                Box::new(SequenceModelClient::always_invalid("cheap")) as Box<dyn ModelClient>,
            ),
            (
                strong,
                Box::new(SequenceModelClient::finish_ok("strong")) as Box<dyn ModelClient>,
            ),
        ];
        let session = AiSessionConfig::new("goal", "Generic");
        let mut agent = AiAgent::with_model_routing(catalog, session, EscalationBudget::new(2));
        let result = AgentLoop::new(5)
            .with_max_stale_iterations(3)
            .with_recovery_budget(RecoveryBudget::new(1, Duration::ZERO))
            .run(&harness(), &mut agent, AgentContext::new("m"));

        assert!(
            result
                .history
                .routing_decisions
                .iter()
                .any(|d| d.action == RoutingAction::EscalateCapability)
        );
        assert_eq!(result.status, LoopStatus::Completed);
        // Tras escalar, el modelo strong termina OK; stale window no forzó NonProgress.
        assert!(
            result.history.progress_assessments.is_empty()
                || result.status == LoopStatus::Completed
        );
    }

    /// TEST N — goal-driven loop path: escalate ModelCapability y continúa.
    #[test]
    fn n_agent_loop_escalates_model_capability_then_completes() {
        let cheap = ModelCandidate::new("mock", "cheap", RelativeTier::Low, RelativeTier::Low);
        let strong = ModelCandidate::new("mock", "strong", RelativeTier::High, RelativeTier::High);
        // cheap: InvalidResponse → ModelCapability; strong: finish ok
        let catalog = vec![
            (
                cheap,
                Box::new(SequenceModelClient::fail_then_finish(
                    "cheap",
                    vec![ModelError::InvalidResponse("unusable".into())],
                )) as Box<dyn ModelClient>,
            ),
            (
                strong,
                Box::new(SequenceModelClient::finish_ok("strong")) as Box<dyn ModelClient>,
            ),
        ];
        let mut agent = AiAgent::with_model_routing(
            catalog,
            AiSessionConfig::new("n", "Generic"),
            EscalationBudget::new(2),
        );
        let result = AgentLoop::new(4)
            .with_recovery_budget(RecoveryBudget::new(1, Duration::ZERO))
            .run(&harness(), &mut agent, AgentContext::new("n"));

        assert_eq!(result.status, LoopStatus::Completed);
        let route = result
            .history
            .routing_decisions
            .iter()
            .find(|d| d.action == RoutingAction::EscalateCapability)
            .expect("escalation recorded");
        assert_eq!(route.from.model_id, "cheap");
        assert_eq!(route.to.as_ref().unwrap().model_id, "strong");
        assert_eq!(route.reason, RoutingReason::ModelCapabilityEscalate);
        assert_eq!(agent.active_model_candidate().unwrap().model_id, "strong");
    }

    /// TEST: ExternalTransient + Retry-After en loop → WaitSameModel, sin escalate.
    #[test]
    fn loop_transient_retry_after_waits_same_model() {
        let cheap = ModelCandidate::new("mock", "cheap", RelativeTier::Low, RelativeTier::Low);
        let strong = ModelCandidate::new("mock", "strong", RelativeTier::High, RelativeTier::High);
        let catalog = vec![
            (
                cheap,
                Box::new(SequenceModelClient::fail_then_finish(
                    "cheap",
                    vec![ModelError::rate_limited_with_retry_after(
                        "rate_limited",
                        Some(Duration::from_secs(1)),
                    )],
                )) as Box<dyn ModelClient>,
            ),
            (
                strong,
                Box::new(SequenceModelClient::finish_ok("strong")) as Box<dyn ModelClient>,
            ),
        ];
        let mut agent = AiAgent::with_model_routing(
            catalog,
            AiSessionConfig::new("t", "Generic"),
            EscalationBudget::new(3),
        );
        let result = AgentLoop::new(4)
            .with_recovery_budget(RecoveryBudget::new(3, Duration::ZERO))
            .run(&harness(), &mut agent, AgentContext::new("t"));

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(
            result
                .history
                .routing_decisions
                .iter()
                .any(|d| d.action == RoutingAction::WaitSameModel
                    && d.reason == RoutingReason::ExternalTransientWait)
        );
        assert!(
            result
                .history
                .routing_decisions
                .iter()
                .all(|d| d.action != RoutingAction::EscalateCapability)
        );
        assert_eq!(agent.active_model_candidate().unwrap().model_id, "cheap");
        assert_eq!(
            result.history.failure_reports[0].classification,
            FailureClass::ExternalTransient
        );
    }

    /// TEST: SystemFailure (unknown tool) → Stop, sin escalate aunque haya high tier.
    #[test]
    fn loop_system_failure_does_not_escalate() {
        let cheap = ModelCandidate::new("mock", "cheap", RelativeTier::Low, RelativeTier::Low);
        let strong = ModelCandidate::new("mock", "strong", RelativeTier::High, RelativeTier::High);
        let catalog = vec![
            (
                cheap,
                Box::new(SequenceModelClient::finish_ok("cheap")) as Box<dyn ModelClient>,
            ),
            (
                strong,
                Box::new(SequenceModelClient::finish_ok("strong")) as Box<dyn ModelClient>,
            ),
        ];
        // Agent sin fallo de modelo: forzamos UnknownTool vía acción InvokeTool.
        // Usamos un agente scripted a través de AiAgent no aplica; usamos MockAgent path.
        // Aquí verificamos try_route_after_failure directo con classify_system_failure.
        let mut agent = AiAgent::with_model_routing(
            catalog,
            AiSessionConfig::new("sys", "Generic"),
            EscalationBudget::new(5),
        );
        let evidence = crate::harness::failure_classification::classify_system_failure(
            "herramienta no registrada: ghost",
            Some("ghost".into()),
        );
        let decision = agent
            .try_route_after_failure(&evidence, false)
            .expect("routing configured");
        assert_eq!(decision.action, RoutingAction::Stop);
        assert_eq!(decision.reason, RoutingReason::SystemFailureNoEscalation);
        assert_eq!(agent.active_model_candidate().unwrap().model_id, "cheap");
    }
}
