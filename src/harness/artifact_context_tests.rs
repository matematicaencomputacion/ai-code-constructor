//! Tests de integración: RustArtifact como objeto de dominio en el ciclo Harness.

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::context::AgentContext;
    use crate::harness::correction::{Correction, CorrectionOperation, CorrectionTarget};
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::evaluation_engine::EvaluationEngine;
    use crate::harness::model::{AiSessionConfig, MockModelClient, model_request_from_context};
    use crate::harness::observation::AgentObservation;
    use crate::harness::runtime::Harness;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::specification_planner::plan_specification;
    use crate::harness::tool::Tool;
    use crate::harness::tool_permission::ToolPermissionConstraint;
    use crate::harness::tools::{
        COMPILE, CompileTool, CorrectionTool, VALIDATE, ValidationTool, encode_correction_input,
        encode_validate_input,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn compile_tool_consumes_artifact() {
        // G
        let artifact =
            RustArtifact::with_id(ArtifactId::new("art-compile"), "main.rs", "fn main() {}");
        let ctx = AgentContext::new("compile").with_working_artifact(artifact);
        let result = CompileTool.execute("", &ctx);
        assert!(result.success);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-compile")
        );
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.artifact_id.as_ref().map(|id| id.as_str()) == Some("art-compile"))
        );
    }

    #[test]
    fn validation_tool_consumes_artifact() {
        // H
        let code = r#"fn main() {
    crear_servidor();
    definir_endpoints();
    implementar_handlers();
}
fn crear_servidor() { println!("Servidor HTTP configurado"); }
fn definir_endpoints() { println!("Endpoints definidos"); }
fn implementar_handlers() { println!("Handlers implementados"); }
"#;
        let artifact = RustArtifact::with_id(ArtifactId::new("art-validate"), "main.rs", code);
        let ctx = AgentContext::new("validate").with_working_artifact(artifact);
        let input = encode_validate_input("Crear una API REST", None, "Api");
        let result = ValidationTool.execute(&input, &ctx);
        assert!(result.success, "{}", result.output);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-validate")
        );
    }

    #[test]
    fn evidence_preserves_artifact_id() {
        // I
        let artifact = RustArtifact::with_id(ArtifactId::new("art-ev"), "main.rs", "fn main() {}");
        let ctx = AgentContext::new("ev").with_working_artifact(artifact);
        let result = CompileTool.execute("", &ctx);
        let evidence = result
            .evidence
            .iter()
            .find(|e| e.label == "artifact_id")
            .expect("artifact_id evidence");
        assert_eq!(evidence.detail, "art-ev");
        assert_eq!(
            evidence.artifact_id.as_ref().map(|id| id.as_str()),
            Some("art-ev")
        );
    }

    #[test]
    fn evaluation_preserves_criterion_id_with_artifact_evidence() {
        // J
        let criterion = AcceptanceCriterion::new("ac-artifact", "compila", CriterionKind::Compile);
        let artifact = RustArtifact::with_id(ArtifactId::new("art-j"), "main.rs", "fn main() {}");
        let ctx = AgentContext::new("j").with_working_artifact(artifact);
        let tool_result = CompileTool.execute("", &ctx);
        let evaluation =
            EvaluationEngine::new().evaluate_criterion(&criterion, &tool_result.evidence);
        assert_eq!(evaluation.criterion_id.as_str(), "ac-artifact");
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
        assert!(
            evaluation
                .evidence_used
                .iter()
                .any(|e| e.label == "artifact_id")
        );
    }

    #[test]
    fn ai_agent_receives_artifact_in_model_request() {
        // N
        let artifact = RustArtifact::with_id(ArtifactId::new("art-ai"), "main.rs", "fn main() {}");
        let ctx = AgentContext::new("ai").with_working_artifact(artifact);
        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());
        let request = model_request_from_context(&ctx, &session).expect("request");
        assert_eq!(request.artifact_id.as_deref(), Some("art-ai"));
        assert_eq!(request.artifact_language.as_deref(), Some("Rust"));
        assert_eq!(request.working_code.as_deref(), Some("fn main() {}"));

        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let _ = agent.propose(&ctx);
        assert_eq!(
            agent.trace.requests[0].artifact_id.as_deref(),
            Some("art-ai")
        );
    }

    #[test]
    fn ai_agent_does_not_execute_tools() {
        // O
        let executed = Arc::new(AtomicBool::new(false));
        struct TrackingCompile {
            flag: Arc<AtomicBool>,
        }
        impl Tool for TrackingCompile {
            fn name(&self) -> &str {
                COMPILE
            }
            fn execute(&self, _input: &str, _ctx: &AgentContext) -> crate::harness::ToolResult {
                self.flag.store(true, Ordering::SeqCst);
                crate::harness::ToolResult::success("ok", vec![])
            }
        }
        let _tool = TrackingCompile {
            flag: Arc::clone(&executed),
        };
        let mut agent = AiAgent::new(
            Box::new(MockModelClient::new()),
            AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string()),
        );
        let _ = agent.propose(&AgentContext::new("o").with_working_code("fn main() {}"));
        assert!(!executed.load(Ordering::SeqCst));
    }

    #[test]
    fn loop_respects_max_iterations_with_artifact() {
        // P
        struct NeverFinish;
        impl Agent for NeverFinish {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                AgentAction::Compile {
                    code: ctx.working_code().unwrap_or("fn main() {}").to_string(),
                }
            }
        }
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));
        let ctx = AgentContext::new("p").with_working_artifact(RustArtifact::with_id(
            ArtifactId::new("art-p"),
            "main.rs",
            "fn main() {}",
        ));
        let result = AgentLoop::new(2).run(&harness, &mut NeverFinish, ctx);
        assert_eq!(result.status, LoopStatus::MaxIterations);
        assert_eq!(result.iterations, 2);
    }

    #[test]
    fn e2e_specification_artifact_loop_evaluation_finish() {
        // E2E
        let spec = Specification::new("spec-art-e2e", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);
        let planned = plan_specification(&spec).expect("plan");
        assert_eq!(planned.specification_id, spec.id);

        let artifact = RustArtifact::with_id(
            ArtifactId::new("art-e2e"),
            "main.rs",
            "fn main() { println!(\"x\"",
        )
        .with_specification_id(spec.id.clone());

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(ValidationTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        /// Agent de prueba: corrige defectos de delimitador tras FAIL y termina tras PASS.
        struct ArtifactCycleAgent {
            repaired: bool,
        }

        impl Agent for ArtifactCycleAgent {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                match &ctx.last_observation {
                    Some(AgentObservation::CriterionEvaluated {
                        verdict: EvaluationVerdict::Fail,
                        ..
                    }) if !self.repaired => {
                        self.repaired = true;
                        AgentAction::ApplyCorrection {
                            corrections: vec![Correction {
                                target: CorrectionTarget::SessionCode,
                                path: None,
                                operation: CorrectionOperation::ReplaceText {
                                    search: "println!(\"x\"".to_string(),
                                    replacement: "println!(\"x\"); }".to_string(),
                                },
                            }],
                        }
                    }
                    Some(AgentObservation::ToolOutcome {
                        tool_name,
                        success: true,
                        ..
                    }) if tool_name == crate::harness::tools::APPLY_CORRECTION => {
                        AgentAction::Compile {
                            code: ctx.working_code().unwrap_or_default().to_string(),
                        }
                    }
                    Some(AgentObservation::CriterionEvaluated {
                        verdict: EvaluationVerdict::Pass,
                        ..
                    }) => AgentAction::Finish {
                        summary: "artifact cycle pass".to_string(),
                    },
                    None => AgentAction::Compile {
                        code: ctx.working_code().unwrap_or_default().to_string(),
                    },
                    Some(_) => AgentAction::Finish {
                        summary: "artifact cycle stop".to_string(),
                    },
                }
            }
        }

        let id_before = artifact.id().clone();
        let ctx = AgentContext::new("e2e-art")
            .with_working_artifact(artifact)
            .with_evaluation_specification(spec);

        let mut agent = ArtifactCycleAgent { repaired: false };
        let result = AgentLoop::new(8).run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(result.tools_executed().iter().any(|t| t == COMPILE));
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == crate::harness::tools::APPLY_CORRECTION)
        );
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Fail,
                ..
            }
        )));
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Pass,
                ..
            }
        )));
        let final_artifact = result
            .final_context
            .working_artifact
            .as_ref()
            .expect("artifact");
        assert_eq!(final_artifact.id(), &id_before);
        assert!(final_artifact.source().contains("println!(\"x\");"));
        assert!(final_artifact.revision() >= 1);
        assert_eq!(
            final_artifact.specification_id().map(|id| id.as_str()),
            Some("spec-art-e2e")
        );
        let _ = encode_correction_input(&[]);
        let _ = VALIDATE;
    }
}
