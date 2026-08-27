//! Tests del flujo Specification → Autonomous Construction.

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::LoopStatus;
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::autonomous_construction::{
        AutonomousConstructionConfig, AutonomousConstructionSession, ConstructionStatus,
    };
    use crate::harness::bridge::introduce_validation_defect;
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::evaluation_engine::SpecificationEvaluationStatus;
    use crate::harness::model::{AiSessionConfig, MockModelClient, ModelClient, ModelDecision};
    use crate::harness::observation::AgentObservation;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::specification_planner::plan_specification;
    use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC, VALIDATE};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn api_valid_code() -> String {
        r#"fn main() {
    crear_servidor();
    definir_endpoints();
    implementar_handlers();
}

fn crear_servidor() {
    println!("Servidor HTTP configurado");
}

fn definir_endpoints() {
    println!("Endpoints definidos");
}

fn implementar_handlers() {
    println!("Handlers implementados");
}
"#
        .to_string()
    }

    fn api_construction_spec() -> Specification {
        Specification::new("spec-auto-api", "Crear una API REST")
            .with_requirements(vec![
                Requirement::new("req-validate", "El código valida el plan Api"),
                Requirement::new("req-compile", "El código compila"),
            ])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new(
                    "ac-validate",
                    "ValidationTool pasa",
                    CriterionKind::Validate,
                )
                .satisfying([crate::harness::RequirementId::new("req-validate")]),
                AcceptanceCriterion::new("ac-compile", "CompileTool pasa", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-compile")]),
            ])
    }

    fn compile_only_spec() -> Specification {
        Specification::new("spec-auto-compile", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    #[test]
    fn invalid_specification_stops_before_agent() {
        // A
        let called = Arc::new(AtomicBool::new(false));
        struct SpyClient {
            called: Arc<AtomicBool>,
        }
        impl ModelClient for SpyClient {
            fn complete(
                &self,
                _request: &crate::harness::model::ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, crate::harness::model::ModelError>
            {
                self.called.store(true, Ordering::SeqCst);
                Ok(crate::harness::model::ModelResponse {
                    raw_text: "{}".to_string(),
                })
            }
        }

        let spec = Specification::new("", "Crear una API REST");
        let config = AutonomousConstructionConfig::new(spec, "fn main() {}", 4);
        let result = AutonomousConstructionSession::run_with_model_client(
            config,
            Box::new(SpyClient {
                called: Arc::clone(&called),
            }),
        );
        assert_eq!(result.status, ConstructionStatus::InvalidSpecification);
        assert!(result.loop_result.is_none());
        assert!(!called.load(Ordering::SeqCst));
        assert!(result.tools_executed().is_empty());
    }

    #[test]
    fn valid_specification_produces_build_plan_and_artifact_link() {
        // B + C + D
        let invalid = introduce_validation_defect(&api_valid_code());
        let spec = api_construction_spec();
        let planned = plan_specification(&spec).expect("plan");
        assert_eq!(planned.specification_id, spec.id);

        let config = AutonomousConstructionConfig::new(spec.clone(), invalid, 8);
        let result = AutonomousConstructionSession::run_with_model_client(
            config,
            Box::new(MockModelClient::new()),
        );

        assert!(result.build_plan.is_some());
        assert_eq!(
            result.build_plan.as_ref().unwrap().specification_id,
            spec.id
        );
        let artifact = result.final_artifact.as_ref().expect("artifact");
        assert_eq!(
            artifact.specification_id().map(|id| id.as_str()),
            Some("spec-auto-api")
        );
        assert_eq!(artifact.id().as_str(), "artifact:spec-auto-api");
        assert!(
            result
                .loop_result
                .as_ref()
                .unwrap()
                .final_context
                .working_artifact
                .is_some()
        );
        assert!(
            result
                .loop_result
                .as_ref()
                .unwrap()
                .final_context
                .evaluation_specification
                .is_some()
        );
    }

    #[test]
    fn uses_real_agent_loop_and_action_policy() {
        // E + F
        let config = AutonomousConstructionConfig::new(compile_only_spec(), "fn main() {}", 4);
        let result = AutonomousConstructionSession::run_with_model_client(
            config,
            Box::new(MockModelClient::new()),
        );
        assert!(result.loop_result.is_some());
        assert_eq!(result.action_policy, "action_policy");
        assert!(result.loop_result.as_ref().unwrap().iterations >= 1);
    }

    #[test]
    fn rejection_produces_observation_and_agent_changes_decision() {
        // G + H
        struct CausalClient;
        impl ModelClient for CausalClient {
            fn complete(
                &self,
                request: &crate::harness::model::ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, crate::harness::model::ModelError>
            {
                use crate::harness::model::serialize_decision;
                let decision = match &request.last_observation {
                    None => ModelDecision::Finish {
                        summary: "premature".to_string(),
                    },
                    Some(obs) if obs.kind == "action_rejected" => ModelDecision::Compile {
                        code: request.working_code.clone().unwrap_or_default(),
                    },
                    Some(obs)
                        if obs.kind == "criterion_evaluated"
                            && obs.evaluation_verdict.as_deref() == Some("Pass") =>
                    {
                        ModelDecision::Finish {
                            summary: "ok".to_string(),
                        }
                    }
                    _ => ModelDecision::Finish {
                        summary: "stop".to_string(),
                    },
                };
                Ok(crate::harness::model::ModelResponse {
                    raw_text: serialize_decision(&decision),
                })
            }
        }

        let config = AutonomousConstructionConfig::new(compile_only_spec(), "fn main() {}", 6);
        let result =
            AutonomousConstructionSession::run_with_model_client(config, Box::new(CausalClient));
        let history = &result.loop_result.as_ref().unwrap().history;
        assert!(matches!(
            history.proposed_actions.first(),
            Some(AgentAction::Finish { .. })
        ));
        assert!(history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::ActionRejected { constraint, .. } if constraint == "finish"
        )));
        assert!(
            history
                .proposed_actions
                .iter()
                .any(|a| matches!(a, AgentAction::Compile { .. }))
        );
        assert_ne!(
            history.proposed_actions[0], history.proposed_actions[1],
            "segunda decisión causal por Observation"
        );
    }

    #[test]
    fn validation_fail_repair_correction_pass_compile_finish_e2e() {
        // I–O + E2E + S + T + U + V
        let original_spec = api_construction_spec();
        let spec_before = original_spec.clone();
        let invalid = introduce_validation_defect(&api_valid_code());
        let config = AutonomousConstructionConfig::new(original_spec, invalid.clone(), 10);
        let result = AutonomousConstructionSession::run_with_model_client(
            config,
            Box::new(MockModelClient::new()),
        );

        assert_eq!(result.status, ConstructionStatus::Completed);
        assert_eq!(
            result.specification_evaluation.as_ref().map(|e| e.status),
            Some(SpecificationEvaluationStatus::Pass)
        );

        let loop_result = result.loop_result.as_ref().expect("loop");
        let tools = result.tools_executed();
        assert!(tools.iter().any(|t| t == VALIDATE));
        assert!(tools.iter().any(|t| t == REPAIR_DIAGNOSTIC));
        assert!(tools.iter().any(|t| t == APPLY_CORRECTION));
        assert!(tools.iter().any(|t| t == COMPILE));

        // Secuencia observable: Validate antes de Repair, Repair antes de Correction, etc.
        let validate_pos = tools.iter().position(|t| t == VALIDATE).unwrap();
        let repair_pos = tools.iter().position(|t| t == REPAIR_DIAGNOSTIC).unwrap();
        let correct_pos = tools.iter().position(|t| t == APPLY_CORRECTION).unwrap();
        let compile_pos = tools.iter().position(|t| t == COMPILE).unwrap();
        assert!(validate_pos < repair_pos);
        assert!(repair_pos < correct_pos);
        assert!(correct_pos < compile_pos);

        assert!(loop_result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                kind: CriterionKind::Validate,
                verdict: EvaluationVerdict::Fail,
                ..
            }
        )));
        assert!(loop_result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                kind: CriterionKind::Validate,
                verdict: EvaluationVerdict::Pass,
                ..
            }
        )));
        assert!(loop_result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                kind: CriterionKind::Compile,
                verdict: EvaluationVerdict::Pass,
                ..
            }
        )));
        assert!(loop_result.history.observations.iter().any(|o| {
            matches!(o, AgentObservation::ToolOutcome { tool_name, .. } if tool_name == REPAIR_DIAGNOSTIC)
                && o.repairer_feedback().iter().any(|f| !f.is_empty())
                    || matches!(o, AgentObservation::ToolOutcome { tool_name, success: true, .. } if tool_name == REPAIR_DIAGNOSTIC)
        }));

        let final_artifact = result.final_artifact.as_ref().expect("artifact");
        assert_eq!(final_artifact.id().as_str(), "artifact:spec-auto-api");
        assert_ne!(final_artifact.source(), invalid);
        assert!(final_artifact.revision() >= 1);
        assert_eq!(spec_before, api_construction_spec());
        assert_eq!(result.specification_id.as_str(), "spec-auto-api");
        assert!(matches!(
            loop_result.history.proposed_actions.last(),
            Some(AgentAction::Finish { .. })
        ));
        assert_eq!(loop_result.status, LoopStatus::Completed);
    }

    #[test]
    fn insufficient_evidence_and_fail_block_completed_status() {
        // P + Q — FinishConstraint + evaluation final
        struct AlwaysFinish;
        impl ModelClient for AlwaysFinish {
            fn complete(
                &self,
                _request: &crate::harness::model::ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, crate::harness::model::ModelError>
            {
                Ok(crate::harness::model::ModelResponse {
                    raw_text: crate::harness::model::serialize_decision(&ModelDecision::Finish {
                        summary: "no evidence".to_string(),
                    }),
                })
            }
        }

        let config = AutonomousConstructionConfig::new(compile_only_spec(), "fn main() {}", 3);
        let result =
            AutonomousConstructionSession::run_with_model_client(config, Box::new(AlwaysFinish));
        assert_ne!(result.status, ConstructionStatus::Completed);
        assert_eq!(result.status, ConstructionStatus::MaxIterations);
        assert_ne!(
            result.specification_evaluation.as_ref().map(|e| e.status),
            Some(SpecificationEvaluationStatus::Pass)
        );
    }

    #[test]
    fn max_iterations_is_respected() {
        // R
        struct SpamCompile;
        impl ModelClient for SpamCompile {
            fn complete(
                &self,
                request: &crate::harness::model::ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, crate::harness::model::ModelError>
            {
                Ok(crate::harness::model::ModelResponse {
                    raw_text: crate::harness::model::serialize_decision(&ModelDecision::Compile {
                        code: request.working_code.clone().unwrap_or_default(),
                    }),
                })
            }
        }
        let config = AutonomousConstructionConfig::new(compile_only_spec(), "fn main() {}", 3);
        let result =
            AutonomousConstructionSession::run_with_model_client(config, Box::new(SpamCompile));
        assert_eq!(result.status, ConstructionStatus::MaxIterations);
        assert_eq!(result.iterations(), 3);
    }

    #[test]
    fn policy_injection_works() {
        let policy = ActionPolicy::default_session_policy();
        let config = AutonomousConstructionConfig::new(compile_only_spec(), "fn main() {}", 5);
        let mut agent = AiAgent::new(
            Box::new(MockModelClient::new()),
            AiSessionConfig {
                user_request: "Crear una API REST".to_string(),
                plan_kind: "Api".to_string(),
            },
        );
        let result = AutonomousConstructionSession::run_with_policy(config, &mut agent, policy);
        assert_eq!(result.action_policy, "action_policy");
        assert!(result.loop_result.is_some());
    }

    #[test]
    fn agent_and_model_client_do_not_execute_tools() {
        // W + X
        let executed = Arc::new(AtomicBool::new(false));
        struct TrackingAgent {
            inner: AiAgent,
            flag: Arc<AtomicBool>,
        }
        impl Agent for TrackingAgent {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                let action = self.inner.propose(ctx);
                // propose no debe haber ejecutado Tools del harness
                assert!(!self.flag.load(Ordering::SeqCst));
                action
            }
        }
        let mut agent = TrackingAgent {
            inner: AiAgent::new(
                Box::new(MockModelClient::new()),
                AiSessionConfig {
                    user_request: "Crear una API REST".to_string(),
                    plan_kind: "Api".to_string(),
                },
            ),
            flag: Arc::clone(&executed),
        };
        let _ = agent.propose(&AgentContext::new("w").with_working_code("fn main() {}"));
        assert!(!executed.load(Ordering::SeqCst));
    }
}
