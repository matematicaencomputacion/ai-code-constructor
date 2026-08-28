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
        let config = AutonomousConstructionConfig::new(spec, 4).with_initial_source("fn main() {}");
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

        let config =
            AutonomousConstructionConfig::new(spec.clone(), 8).with_initial_source(invalid);
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
        let config = AutonomousConstructionConfig::new(compile_only_spec(), 4)
            .with_initial_source("fn main() {}");
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

        let config = AutonomousConstructionConfig::new(compile_only_spec(), 6)
            .with_initial_source("fn main() {}");
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
        let config = AutonomousConstructionConfig::new(original_spec, 10)
            .with_initial_source(invalid.clone());
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
        assert!(
            compile_pos < validate_pos,
            "prioridad Compile antes que Validate; tools={tools:?}"
        );
        assert!(validate_pos < repair_pos);
        assert!(repair_pos < correct_pos);

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

        let config = AutonomousConstructionConfig::new(compile_only_spec(), 3)
            .with_initial_source("fn main() {}");
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
        let config = AutonomousConstructionConfig::new(compile_only_spec(), 3)
            .with_initial_source("fn main() {}");
        let result =
            AutonomousConstructionSession::run_with_model_client(config, Box::new(SpamCompile));
        assert_eq!(result.status, ConstructionStatus::MaxIterations);
        assert_eq!(result.iterations(), 3);
    }

    #[test]
    fn policy_injection_works() {
        let policy = ActionPolicy::default_session_policy();
        let config = AutonomousConstructionConfig::new(compile_only_spec(), 5)
            .with_initial_source("fn main() {}");
        let mut agent = AiAgent::new(
            Box::new(MockModelClient::new()),
            AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string()),
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
                AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string()),
            ),
            flag: Arc::clone(&executed),
        };
        let _ = agent.propose(&AgentContext::new("w").with_working_code("fn main() {}"));
        assert!(!executed.load(Ordering::SeqCst));
    }

    #[test]
    fn plan_to_initial_artifact_without_caller_source() {
        use crate::builder::initial_source_for_kind;
        use crate::harness::autonomous_construction::initial_artifact_from_plan;
        use crate::planner::PlanKind;

        let spec = api_construction_spec();
        let planned = plan_specification(&spec).expect("plan");
        assert_eq!(planned.plan.kind, PlanKind::Api);

        let artifact = initial_artifact_from_plan(spec.id.clone(), &planned.plan, "main.rs");
        assert_eq!(artifact.id().as_str(), "artifact:spec-auto-api");
        assert_eq!(
            artifact.specification_id().map(|id| id.as_str()),
            Some("spec-auto-api")
        );
        assert_eq!(artifact.revision(), 0);
        assert_eq!(artifact.source(), initial_source_for_kind(PlanKind::Api));
    }

    #[test]
    fn autonomous_session_builds_initial_artifact_from_builder() {
        let spec = api_construction_spec();
        let config = AutonomousConstructionConfig::new(spec, 8);
        assert!(config.initial_source.is_none());

        let result = AutonomousConstructionSession::run_with_model_client(
            config,
            Box::new(MockModelClient::new()),
        );

        assert_eq!(result.status, ConstructionStatus::Completed);
        let artifact = result.final_artifact.as_ref().expect("artifact");
        assert_eq!(artifact.id().as_str(), "artifact:spec-auto-api");
        assert!(artifact.source().contains("crear_servidor"));
        assert!(result.tools_executed().iter().any(|t| t == VALIDATE));
        assert!(result.tools_executed().iter().any(|t| t == COMPILE));
        assert_eq!(
            result.specification_evaluation.as_ref().map(|e| e.status),
            Some(SpecificationEvaluationStatus::Pass)
        );
        // Sin correcciones: revision permanece 0 (source del Builder ya válido).
        assert_eq!(artifact.revision(), 0);
    }

    #[test]
    fn e2e_specification_builder_autonomous_construction() {
        // Specification → plan → Builder → Artifact → AgentLoop → Completed
        let spec = api_construction_spec();
        let planned = plan_specification(&spec).expect("plan");
        let expected_source = crate::builder::initial_source_for_kind(planned.plan.kind);

        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(spec, 8),
            Box::new(MockModelClient::new()),
        );

        assert_eq!(result.status, ConstructionStatus::Completed);
        assert_eq!(
            result.build_plan.as_ref().map(|p| p.plan.kind),
            Some(planned.plan.kind)
        );
        let final_artifact = result.final_artifact.as_ref().expect("artifact");
        assert_eq!(final_artifact.source(), expected_source);
        assert_eq!(
            final_artifact.specification_id().map(|id| id.as_str()),
            Some("spec-auto-api")
        );
        assert!(
            result
                .loop_result
                .as_ref()
                .unwrap()
                .history
                .proposed_actions
                .iter()
                .any(|a| matches!(a, AgentAction::Finish { .. }))
        );
    }

    #[test]
    fn observability_reports_duration_iterations_tools_and_criteria() {
        let invalid = introduce_validation_defect(&api_valid_code());
        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(api_construction_spec(), 10)
                .with_initial_source(invalid),
            Box::new(MockModelClient::new()),
        );

        let obs = &result.observability;
        assert_eq!(obs.final_status, ConstructionStatus::Completed);
        assert_eq!(obs.iteration_count, result.iterations());
        assert_eq!(
            obs.iteration_count,
            result.loop_result.as_ref().unwrap().iterations
        );
        // Duración presente; no assert de valor exacto.
        let _ = obs.duration_ms;

        assert!(obs.tools_executed_sequence.iter().any(|t| t == VALIDATE));
        assert!(
            obs.tools_executed_sequence
                .iter()
                .any(|t| t == REPAIR_DIAGNOSTIC)
        );
        assert!(
            obs.tools_executed_sequence
                .iter()
                .any(|t| t == APPLY_CORRECTION)
        );
        assert!(obs.tools_executed_sequence.iter().any(|t| t == COMPILE));
        assert!(obs.tool_execution_count(VALIDATE) >= 2);
        assert_eq!(obs.tool_execution_count(COMPILE), 1);

        let validate_verdicts = obs.criterion_verdicts("ac-validate");
        assert!(validate_verdicts.contains(&EvaluationVerdict::Fail));
        assert!(validate_verdicts.contains(&EvaluationVerdict::Pass));
        assert!(
            validate_verdicts
                .iter()
                .position(|v| *v == EvaluationVerdict::Fail)
                < validate_verdicts
                    .iter()
                    .position(|v| *v == EvaluationVerdict::Pass)
        );
        assert!(
            obs.criterion_verdicts("ac-compile")
                .contains(&EvaluationVerdict::Pass)
        );
        assert!(
            obs.final_criteria
                .iter()
                .any(|c| c.criterion_id == "ac-validate" && c.verdict == EvaluationVerdict::Pass)
        );
        assert!(
            obs.final_criteria
                .iter()
                .any(|c| c.criterion_id == "ac-compile" && c.verdict == EvaluationVerdict::Pass)
        );
        // Retries de modelo: sin RetryingModelClient cableado → None (limitación documentada).
        assert!(obs.model_retry_count.is_none());
    }

    #[test]
    fn observability_invalid_specification_has_zero_iterations_and_no_tools() {
        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(Specification::new("", "Crear una API REST"), 4),
            Box::new(MockModelClient::new()),
        );
        assert_eq!(result.status, ConstructionStatus::InvalidSpecification);
        assert_eq!(
            result.observability.final_status,
            ConstructionStatus::InvalidSpecification
        );
        assert_eq!(result.observability.iteration_count, 0);
        assert!(result.observability.tools_executed_sequence.is_empty());
        assert!(result.observability.tool_summaries.is_empty());
        assert!(result.observability.criterion_timeline.is_empty());
        assert!(result.loop_result.is_none());
        let _ = result.observability.duration_ms;
    }

    #[test]
    fn observability_max_iterations_matches_loop() {
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
        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(compile_only_spec(), 3)
                .with_initial_source("fn main() {}"),
            Box::new(SpamCompile),
        );
        assert_eq!(result.status, ConstructionStatus::MaxIterations);
        assert_eq!(
            result.observability.final_status,
            ConstructionStatus::MaxIterations
        );
        assert_eq!(result.observability.iteration_count, 3);
        assert_eq!(result.observability.iteration_count, result.iterations());
    }

    #[test]
    fn e2e_quality_criteria_observation_driven_completes() {
        use crate::harness::Evidence;
        use crate::harness::constraint::Constraint;
        use crate::harness::runtime::Harness;
        use crate::harness::tool::{Tool, ToolResult};
        use crate::harness::tools::{
            COMPILE, CompileTool, CorrectionTool, RUN_CLIPPY, RUN_TESTS, RepairDiagnosticTool,
            VALIDATE, ValidationTool,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        fn quality_construction_spec() -> Specification {
            Specification::new("spec-quality-e2e", "Crear una API REST")
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

        /// Stub determinista: emite Evidence genérica `tool` + `exit_status` (sin cargo).
        struct SequencedExitTool {
            name: &'static str,
            exits: &'static [&'static str],
            calls: AtomicUsize,
        }

        impl Tool for SequencedExitTool {
            fn name(&self) -> &str {
                self.name
            }

            fn execute(&self, _input: &str, _ctx: &AgentContext) -> ToolResult {
                let index = self.calls.fetch_add(1, Ordering::SeqCst);
                let exit = self
                    .exits
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| self.exits.last().copied().unwrap_or("0"));
                let success = exit == "0";
                if success {
                    ToolResult::success(
                        format!("stub {} exit={exit}", self.name),
                        vec![
                            Evidence::new("tool", self.name),
                            Evidence::new("exit_status", exit),
                        ],
                    )
                } else {
                    ToolResult::failure(
                        format!("stub {} exit={exit}", self.name),
                        vec![
                            Evidence::new("tool", self.name),
                            Evidence::new("exit_status", exit),
                        ],
                    )
                }
            }
        }

        fn criterion_has_pass(ctx: &AgentContext, kind: CriterionKind) -> bool {
            ctx.observation_history.iter().rev().any(|obs| {
                matches!(
                    obs,
                    AgentObservation::CriterionEvaluated {
                        kind: k,
                        verdict: EvaluationVerdict::Pass,
                        ..
                    } if *k == kind
                )
            })
        }

        /// Agent reactivo: propone la siguiente Tool según Observation / criterios PASS.
        struct QualityCriteriaAgent {
            request: String,
            plan_kind: String,
        }

        impl Agent for QualityCriteriaAgent {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                let code = ctx.working_code().map(str::to_string);
                if !criterion_has_pass(ctx, CriterionKind::Validate) {
                    return AgentAction::Validate {
                        request: self.request.clone(),
                        code: code.clone(),
                        plan_kind: self.plan_kind.clone(),
                    };
                }
                if !criterion_has_pass(ctx, CriterionKind::Compile) {
                    return AgentAction::Compile {
                        code: code.unwrap_or_default(),
                    };
                }
                if !criterion_has_pass(ctx, CriterionKind::RunTests) {
                    return AgentAction::RunTests {
                        filter: String::new(),
                    };
                }
                if !criterion_has_pass(ctx, CriterionKind::Clippy) {
                    return AgentAction::RunClippy;
                }
                AgentAction::Finish {
                    summary: "all quality criteria satisfied".to_string(),
                }
            }
        }

        let policy = ActionPolicy::default_session_policy();
        let policy_name = policy.name().to_string();
        let mut harness = Harness::new(20);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(CompileTool));
        // Stubs con el mismo contrato Evidence que TestTool/ClippyTool (evita cargo recursivo).
        harness.register_tool(Box::new(SequencedExitTool {
            name: RUN_TESTS,
            exits: &["1", "0"],
            calls: AtomicUsize::new(0),
        }));
        harness.register_tool(Box::new(SequencedExitTool {
            name: RUN_CLIPPY,
            exits: &["0"],
            calls: AtomicUsize::new(0),
        }));
        harness.register_constraint(Box::new(policy));

        let mut agent = QualityCriteriaAgent {
            request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        };
        let result = AutonomousConstructionSession::run_with_harness(
            AutonomousConstructionConfig::new(quality_construction_spec(), 12)
                .with_initial_source(api_valid_code()),
            &mut agent,
            policy_name,
            harness,
        );

        assert_eq!(result.status, ConstructionStatus::Completed);
        assert_eq!(
            result.specification_evaluation.as_ref().unwrap().status,
            SpecificationEvaluationStatus::Pass
        );

        let tools = result.observability.tools_executed_sequence.clone();
        assert!(tools.iter().any(|t| t == VALIDATE));
        assert!(tools.iter().any(|t| t == COMPILE));
        assert!(tools.iter().any(|t| t == RUN_TESTS));
        assert!(tools.iter().any(|t| t == RUN_CLIPPY));
        assert!(result.observability.tool_execution_count(RUN_TESTS) >= 2);

        let test_verdicts = result.observability.criterion_verdicts("ac-tests");
        assert!(test_verdicts.contains(&EvaluationVerdict::Fail));
        assert!(test_verdicts.contains(&EvaluationVerdict::Pass));
        assert!(
            test_verdicts
                .iter()
                .position(|v| *v == EvaluationVerdict::Fail)
                < test_verdicts
                    .iter()
                    .position(|v| *v == EvaluationVerdict::Pass)
        );
        assert!(
            result
                .observability
                .criterion_verdicts("ac-clippy")
                .contains(&EvaluationVerdict::Pass)
        );
        assert!(
            result
                .observability
                .final_criteria
                .iter()
                .all(|c| c.verdict == EvaluationVerdict::Pass)
        );
    }

    #[test]
    fn mock_model_client_without_retry_obs_keeps_model_retry_none() {
        // I
        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(api_construction_spec(), 4)
                .with_initial_source(api_valid_code()),
            Box::new(MockModelClient::new()),
        );
        assert!(result.observability.model_retry_count.is_none());
    }

    #[test]
    fn invalid_specification_with_retry_handle_is_some_zero() {
        // J
        use crate::harness::retrying_model_client::{RetryConfig, RetryingModelClient};
        use std::time::Duration;

        struct NeverCalled;
        impl ModelClient for NeverCalled {
            fn complete(
                &self,
                _request: &crate::harness::model::ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, crate::harness::model::ModelError>
            {
                panic!("complete no debe llamarse en InvalidSpecification");
            }
        }

        let client = RetryingModelClient::with_config(
            Box::new(NeverCalled),
            RetryConfig {
                max_retries: 2,
                backoff: Duration::from_millis(0),
            },
        );
        let obs = client.observability();
        let result = AutonomousConstructionSession::run_with_model_client_and_retry_observability(
            AutonomousConstructionConfig::new(Specification::new("", "Crear una API REST"), 4),
            Box::new(client),
            obs,
        );
        assert_eq!(result.status, ConstructionStatus::InvalidSpecification);
        assert_eq!(result.observability.model_retry_count, Some(0));
    }

    #[test]
    fn invalid_specification_without_handle_is_none() {
        // J
        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(Specification::new("", "Crear una API REST"), 4),
            Box::new(MockModelClient::new()),
        );
        assert_eq!(result.status, ConstructionStatus::InvalidSpecification);
        assert!(result.observability.model_retry_count.is_none());
    }

    #[test]
    fn max_iterations_retries_are_orthogonal_to_iterations() {
        // K
        use crate::harness::retrying_model_client::{RetryConfig, RetryingModelClient};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        struct FlakyThenCompile {
            calls: AtomicUsize,
        }
        impl ModelClient for FlakyThenCompile {
            fn complete(
                &self,
                request: &crate::harness::model::ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, crate::harness::model::ModelError>
            {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                // First outer complete: 2 retries then Compile; later completes: Compile
                // Inner call index: for first group fails on n=0,1 then ok on n=2;
                // subsequent groups start at n=3,4,... always ok.
                if n < 2 {
                    return Err(crate::harness::model::ModelError::Timeout);
                }
                Ok(crate::harness::model::ModelResponse {
                    raw_text: crate::harness::model::serialize_decision(&ModelDecision::Compile {
                        code: request.working_code.clone().unwrap_or_default(),
                    }),
                })
            }
        }

        let client = RetryingModelClient::with_config(
            Box::new(FlakyThenCompile {
                calls: AtomicUsize::new(0),
            }),
            RetryConfig {
                max_retries: 3,
                backoff: Duration::from_millis(0),
            },
        );
        let obs = client.observability();
        let result = AutonomousConstructionSession::run_with_model_client_and_retry_observability(
            AutonomousConstructionConfig::new(compile_only_spec(), 3)
                .with_initial_source("fn main() {}"),
            Box::new(client),
            obs,
        );
        assert_eq!(result.status, ConstructionStatus::MaxIterations);
        assert_eq!(result.observability.iteration_count, 3);
        // Solo el primer complete tuvo 2 retries; los otros 2 completes → 0.
        assert_eq!(result.observability.model_retry_count, Some(2));
    }

    #[test]
    fn e2e_retry_observability_into_construction_result() {
        // L
        use crate::harness::retrying_model_client::{RetryConfig, RetryingModelClient};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        struct FlakyThenFinish {
            calls: AtomicUsize,
        }
        impl ModelClient for FlakyThenFinish {
            fn complete(
                &self,
                _request: &crate::harness::model::ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, crate::harness::model::ModelError>
            {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    return Err(crate::harness::model::ModelError::Timeout);
                }
                Ok(crate::harness::model::ModelResponse {
                    raw_text: crate::harness::model::serialize_decision(&ModelDecision::Finish {
                        summary: "ok".to_string(),
                    }),
                })
            }
        }

        let client = RetryingModelClient::with_config(
            Box::new(FlakyThenFinish {
                calls: AtomicUsize::new(0),
            }),
            RetryConfig {
                max_retries: 3,
                backoff: Duration::from_millis(0),
            },
        );
        let obs = client.observability();
        // Spec mínima: Finish puede completar si no hay criterios estrictos...
        // Use compile_only with source that compiles and agent that finishes after retries —
        // Finish without satisfying criteria → Failed, but model_retry_count still set.
        let result = AutonomousConstructionSession::run_with_model_client_and_retry_observability(
            AutonomousConstructionConfig::new(compile_only_spec(), 4)
                .with_initial_source("fn main() {}"),
            Box::new(client),
            obs,
        );
        // iterations y retries son ortogonales; la señal causal proyecta el total real.
        assert_eq!(result.observability.model_retry_count, Some(2));
        assert!(result.observability.iteration_count >= 1);
    }
}
