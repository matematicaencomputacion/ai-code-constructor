//! Harness Core: contratos arquitectónicos mínimos para agentes autónomos.
//!
//! Esta capa es model-agnostic y convive con el ciclo Constructor existente
//! (Planner → Builder → Compiler → Validator → Repairer) sin reemplazarlo.

mod action;
mod action_policy;
mod agent;
mod agent_loop;
mod agent_prompt;
mod ai_agent;
pub mod artifact;
mod artifact_file_operation;
mod artifact_materialization;
mod artifact_mutation;
mod artifact_path;
mod autonomous_construction;
mod bridge;
mod constraint;
mod context;
mod correction;
mod correction_policy;
mod criterion;
mod evaluation;
mod evaluation_engine;
mod evaluation_observation;
mod feature_flags;
mod goal_driven;
mod live_session;
mod model;
mod observation;
mod openai_compatible_client;
mod retrying_model_client;
mod runtime;
pub mod specification;
mod specification_planner;
mod tool;
mod tool_permission;
pub mod tools;

#[cfg(test)]
mod action_policy_tests;
#[cfg(test)]
mod ai_agent_quality_actions_tests;
#[cfg(test)]
mod artifact_context_tests;
#[cfg(test)]
mod artifact_file_operations_tests;
#[cfg(test)]
mod artifact_scoped_quality_tests;
#[cfg(test)]
mod autonomous_construction_tests;
#[cfg(test)]
mod goal_driven_integration_tests;
#[cfg(test)]
mod live_session_builder_initial_artifact_tests;
#[cfg(test)]
mod model_decision_multi_file_correction_tests;
#[cfg(test)]
mod model_multi_file_contract_tests;
#[cfg(test)]
mod multi_file_correction_tests;

// API pública del harness; aún no consumida por el ciclo Constructor.
#[allow(unused_imports)]
pub use action::AgentAction;
#[allow(unused_imports)]
pub use action_policy::{
    ActionConstraint, ActionPolicy, ApplyCorrectionConstraint, ApplyFileOperationsConstraint,
    ArtifactStateConstraint, FinishConstraint, PolicyVerdict, RepairDiagnosticConstraint,
};
#[allow(unused_imports)]
pub use agent::{
    Agent, FailThenStopAgent, FirstActionAgent, MockAgent, NeverFinishAgent,
    ObservationDrivenEchoAgent, PermittedThenFinishAgent, RejectedThenFinishAgent,
    ValidateThenRepairAgent,
};
#[allow(unused_imports)]
pub use agent_loop::{AgentLoop, LoopHistory, LoopResult, LoopStatus};
#[allow(unused_imports)]
pub use agent_prompt::{SYSTEM_PROMPT_VERSION, system_prompt_v1};
#[allow(unused_imports)]
pub use ai_agent::AiAgent;
#[allow(unused_imports)]
pub use artifact::{
    ARTIFACT_CONTRACT_VERSION, ArtifactContractVersion, ArtifactFile, ArtifactId, ArtifactLanguage,
    RustArtifact,
};
#[allow(unused_imports)]
pub use artifact_materialization::ArtifactMaterialization;
#[allow(unused_imports)]
pub use artifact_path::ArtifactPath;
#[allow(unused_imports)]
pub use autonomous_construction::{
    AutonomousConstructionConfig, AutonomousConstructionSession, ConstructionObservability,
    ConstructionResult, ConstructionStatus, CriterionObservabilityEntry,
    GoalDrivenConstructionResult, ToolExecutionSummary, initial_artifact_from_plan,
};
#[allow(unused_imports)]
pub use bridge::{
    BridgeResult, BridgedCompileRepairAgent, BridgedSession, BridgedValidateRepairAgent,
    ConstructorArtifacts, ConstructorBridge, introduce_compile_defect, introduce_validation_defect,
};
#[allow(unused_imports)]
pub use constraint::{Constraint, ConstraintDecision};
#[allow(unused_imports)]
pub use context::AgentContext;
#[allow(unused_imports)]
pub use correction::{
    Correction, CorrectionOperation, CorrectionTarget, SESSION_CODE_TARGET, apply_corrections,
    apply_corrections_to_artifact,
};
#[allow(unused_imports)]
pub use correction_policy::{
    CorrectionPolicy, CorrectionPolicyError, CorrectionPolicyInput, DeterministicCorrectionPolicy,
};
#[allow(unused_imports)]
pub use criterion::CriterionKind;
#[allow(unused_imports)]
pub use evaluation::{Evaluation, EvaluationVerdict, Evidence};
#[allow(unused_imports)]
pub use evaluation_engine::{
    CriterionEvaluation, EvaluationEngine, SpecificationEvaluation, SpecificationEvaluationStatus,
};
#[allow(unused_imports)]
pub use evaluation_observation::{
    EvaluationAwareAgent, ToolEvaluationStep, criterion_kind_for_tool, evaluate_tool_evidence,
    observation_from_criterion_evaluation, observation_from_specification_evaluation,
};
#[allow(unused_imports)]
pub use goal_driven::{
    CriterionGap, EvaluationPlan, EvaluationPlanEntry, GapDrivenAgent, Goal, GoalDrivenHistory,
    GoalDrivenLoop, GoalDrivenResult, GoalDrivenStatus, GoalEscalation, GoalEvaluation,
    GoalEvaluator, GoalGap, GoalProgressTracker, GoalStatus, ProgressSignal,
    collect_evidence_from_context, evaluate_after_tool,
};
#[allow(unused_imports)]
pub use live_session::{
    LIVE_AGENT_MAX_ITERATIONS, LiveSessionConfig, LiveSessionError,
    LiveSessionFromSpecificationOptions, LiveSessionResult, LiveSessionStepRecord,
    LiveSessionTrace, build_validate_compile_harness, build_validate_compile_harness_with_policy,
    live_quality_artifact_source, live_quality_specification, run_live_agent_session,
    run_live_agent_session_with_client, run_live_agent_session_with_client_and_policy,
    run_live_agent_session_with_client_policy_and_retry_observability,
};
#[allow(unused_imports)]
pub use model::{
    AiSessionConfig, MockModelClient, ModelClient, ModelDecision, ModelError,
    ModelInteractionTrace, ModelRequest, ModelResponse, ModelResponseError, SerializedCriterionGap,
    SerializedGoalEvaluation, SerializedGoalGap, StructuredCorrection, StructuredFileOperation,
    append_goal_context_to_message_parts, apply_gap_guidance, decision_from_goal_gap,
    model_request_from_context, parse_model_response, redact_secrets, serialize_decision,
    structured_to_file_operation,
};
#[allow(unused_imports)]
pub use observation::AgentObservation;
#[allow(unused_imports)]
pub use openai_compatible_client::{
    ModelCallMetadata, ModelClientConfig, OpenAICompatibleModelClient,
};
#[allow(unused_imports)]
pub use retrying_model_client::{ModelRetryObservability, RetryConfig, RetryingModelClient};
#[allow(unused_imports)]
pub use runtime::{Harness, HarnessResult, StepOutcome};
#[allow(unused_imports)]
pub use specification::{
    AcceptanceCriterion, AcceptanceCriterionId, Requirement, RequirementId,
    SPECIFICATION_CONTRACT_VERSION, Specification, SpecificationId, SpecificationValidationError,
    SpecificationVersion,
};
#[allow(unused_imports)]
pub use specification_planner::{
    SpecificationBuildPlan, SpecificationPlannerError, plan_specification,
};
#[allow(unused_imports)]
pub use tool::{Tool, ToolResult};
#[allow(unused_imports)]
pub use tool_permission::ToolPermissionConstraint;
#[allow(unused_imports)]
pub use tools::{
    APPLY_CORRECTION, APPLY_FILE_OPERATIONS, CHECK_FORMAT, COMPILE, ClippyTool, CompileTool,
    CorrectionTool, FileOperationsTool, FmtTool, REPAIR_DIAGNOSTIC, RUN_CLIPPY, RUN_TESTS,
    RepairDiagnosticTool, TestTool, VALIDATE, ValidationTool, encode_correction_input,
    encode_file_operations_input, encode_repair_diagnostic_input, encode_validate_input,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn execute(&self, input: &str, _ctx: &AgentContext) -> ToolResult {
            ToolResult::success(input.to_string(), vec![Evidence::new("echo_output", input)])
        }
    }

    struct TrackingTool {
        name: &'static str,
        executed: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }

    impl Tool for TrackingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn execute(&self, input: &str, _ctx: &AgentContext) -> ToolResult {
            self.executed.store(true, Ordering::SeqCst);
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success(
                input.to_string(),
                vec![Evidence::new("tracking", self.name)],
            )
        }
    }

    struct ForbidFinishConstraint;

    impl Constraint for ForbidFinishConstraint {
        fn name(&self) -> &str {
            "forbid_finish"
        }

        fn check(&self, action: &AgentAction, _ctx: &AgentContext) -> ConstraintDecision {
            match action {
                AgentAction::Finish { .. } => ConstraintDecision::Reject {
                    reason: "Finish no permitido en esta fase".to_string(),
                },
                AgentAction::Compile { .. }
                | AgentAction::RunTests { .. }
                | AgentAction::RunClippy
                | AgentAction::CheckFormat
                | AgentAction::Validate { .. }
                | AgentAction::RepairDiagnostic { .. }
                | AgentAction::ApplyCorrection { .. }
                | AgentAction::ApplyFileOperations { .. }
                | AgentAction::InvokeTool { .. }
                | AgentAction::NoOp => ConstraintDecision::Allow,
            }
        }
    }

    #[test]
    fn harness_processes_valid_agent_action() {
        let mut harness = Harness::new(5);
        harness.register_tool(Box::new(EchoTool));

        let mut agent = MockAgent::new(vec![
            AgentAction::InvokeTool {
                tool_name: "echo".to_string(),
                input: "hola".to_string(),
            },
            AgentAction::Finish {
                summary: "listo".to_string(),
            },
        ]);

        let result = harness.run(&mut agent, AgentContext::new("demo"));

        assert!(result.completed);
        assert_eq!(result.actions_executed.len(), 2);
        assert!(matches!(
            result.actions_executed[0],
            AgentAction::InvokeTool { .. }
        ));
        assert!(result.rejected_actions.is_empty());
    }

    #[test]
    fn constraint_can_reject_an_action() {
        let mut harness = Harness::new(1);
        harness.register_constraint(Box::new(ForbidFinishConstraint));

        let mut agent = MockAgent::new(vec![AgentAction::Finish {
            summary: "temprano".to_string(),
        }]);

        let result = harness.run(&mut agent, AgentContext::new("demo"));

        assert_eq!(result.rejected_actions.len(), 1);
        assert!(result.rejected_actions[0].1.contains("Finish no permitido"));
        assert!(result.actions_executed.is_empty());
        assert!(result.has_fail());
    }

    #[test]
    fn tool_produces_result_with_evidence() {
        let tool = EchoTool;
        let ctx = AgentContext::new("goal");
        let result = tool.execute("payload", &ctx);

        assert!(result.success);
        assert_eq!(result.output, "payload");
        assert_eq!(result.evidence.len(), 1);
        assert_eq!(result.evidence[0].label, "echo_output");
        assert_eq!(result.evidence[0].detail, "payload");
    }

    #[test]
    fn evaluation_represents_pass_and_fail() {
        let pass = Evaluation::pass("ok", vec![Evidence::new("check", "green")]);
        let fail = Evaluation::fail("bad", vec![Evidence::new("check", "red")]);

        assert!(pass.is_pass());
        assert!(!pass.is_fail());
        assert_eq!(pass.verdict, EvaluationVerdict::Pass);

        assert!(fail.is_fail());
        assert!(!fail.is_pass());
        assert_eq!(fail.verdict, EvaluationVerdict::Fail);
        assert!(!fail.evidence.is_empty());
    }

    #[test]
    fn harness_returns_structured_execution_result() {
        let mut harness = Harness::new(4);
        harness.register_tool(Box::new(EchoTool));

        let mut agent = MockAgent::new(vec![
            AgentAction::NoOp,
            AgentAction::InvokeTool {
                tool_name: "echo".to_string(),
                input: "dato".to_string(),
            },
            AgentAction::Finish {
                summary: "fin".to_string(),
            },
        ]);

        let result = harness.run(&mut agent, AgentContext::new("estructurado"));

        assert!(result.completed);
        assert_eq!(result.actions_executed.len(), 3);
        assert!(!result.evaluations.is_empty());
        assert_eq!(result.final_context.goal, "estructurado");
        assert!(result.final_context.step >= 3);
        assert!(!result.final_context.observations.is_empty());
        assert!(result.has_pass());
    }

    #[test]
    fn compile_tool_compiles_valid_code() {
        // A — requiere working_artifact (crate materializado)
        let tool = CompileTool;
        let ctx =
            AgentContext::new("compile-ok").with_working_code("fn main() { println!(\"ok\"); }\n");
        let result = tool.execute("", &ctx);
        assert!(result.success, "{}", result.output);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "compile_status" && e.detail == "ok")
        );
    }

    #[test]
    fn compile_tool_fails_invalid_code_with_evidence() {
        // B
        let tool = CompileTool;
        let ctx = AgentContext::new("compile-fail")
            .with_working_code("fn main() { println!(\"broken\"\n");
        let result = tool.execute("", &ctx);
        assert!(!result.success);
        assert!(result.evidence.iter().any(|e| e.label == "compiler_stderr"));
        assert!(
            result.output.contains("delimiter")
                || result.output.contains("error")
                || !result.output.is_empty()
        );
    }

    #[test]
    fn test_tool_produces_structured_result() {
        // C — Artifact con test que pasa (no el workspace del repo)
        let tool = TestTool;
        let source = r#"
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn artifact_unit_passes() {
        assert_eq!(2 + 2, 4);
    }
}
"#;
        let ctx = AgentContext::new("tests").with_working_artifact(
            crate::harness::RustArtifact::with_id(
                crate::harness::ArtifactId::new("art-test-struct"),
                "main.rs",
                source,
            ),
        );
        let result = tool.execute("", &ctx);
        assert!(result.evidence.iter().any(|e| e.label == "exit_status"));
        assert!(result.evidence.iter().any(|e| e.label == "stdout"));
        assert!(result.evidence.iter().any(|e| e.label == "stderr"));
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "tool" && e.detail == RUN_TESTS)
        );
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-test-struct")
        );
        assert!(result.success, "TestTool falló: {}", result.output);
    }

    #[test]
    fn clippy_tool_produces_structured_result() {
        // D — Artifact limpio materializado
        let tool = ClippyTool;
        let ctx = AgentContext::new("clippy").with_working_artifact(
            crate::harness::RustArtifact::with_id(
                crate::harness::ArtifactId::new("art-clippy-struct"),
                "main.rs",
                "fn main() {}\n",
            ),
        );
        let result = tool.execute("", &ctx);
        assert!(result.evidence.iter().any(|e| e.label == "exit_status"));
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "tool" && e.detail == RUN_CLIPPY)
        );
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-clippy-struct")
        );
        assert!(result.success, "ClippyTool falló: {}", result.output);
    }

    #[test]
    fn fmt_tool_produces_structured_result() {
        // E — Artifact formateado
        let tool = FmtTool;
        let ctx =
            AgentContext::new("fmt").with_working_artifact(crate::harness::RustArtifact::with_id(
                crate::harness::ArtifactId::new("art-fmt-struct"),
                "main.rs",
                "fn main() {}\n",
            ));
        let result = tool.execute("", &ctx);
        assert!(result.evidence.iter().any(|e| e.label == "exit_status"));
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "tool" && e.detail == CHECK_FORMAT)
        );
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-fmt-struct")
        );
        assert!(result.success, "FmtTool falló: {}", result.output);
    }

    #[test]
    fn validation_tool_uses_real_validator() {
        // F — mensajes característicos de validator::validate
        let tool = ValidationTool;
        let input = encode_validate_input("Crear una API REST", None, "Api");
        let result = tool.execute(&input, &AgentContext::new("validate"));

        assert!(!result.success);
        assert!(
            result.output.contains("No se generó ningún código."),
            "debe reutilizar el Validator real: {}",
            result.output
        );
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.detail.contains("No se generó ningún código."))
        );
        assert!(
            !result
                .evidence
                .iter()
                .any(|e| e.label.starts_with("repairer_feedback_")),
            "ValidationTool no debe generar feedback"
        );
        assert!(
            !result.evidence.iter().any(|e| e.label == "feedback_count"),
            "ValidationTool no debe reportar feedback_count"
        );
    }

    #[test]
    fn validation_tool_only_validates_without_feedback() {
        let tool = ValidationTool;
        let api_code = r#"fn main() { println!("hola"); }"#;
        let input = encode_validate_input("Crear una API REST", Some(api_code), "Api");
        let result = tool.execute(&input, &AgentContext::new("validate-only"));

        assert!(!result.success);
        assert!(result.evidence.iter().any(|e| e.label == "validate_status"));
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label.starts_with("validator_error_"))
        );
        assert!(
            result
                .evidence
                .iter()
                .all(|e| !e.label.starts_with("repairer_feedback_"))
        );
    }

    #[test]
    fn tool_permission_constraint_rejects_unauthorized_tool() {
        // G
        let constraint = ToolPermissionConstraint::default_constructor_tools();
        let decision = constraint.check(
            &AgentAction::InvokeTool {
                tool_name: "echo".to_string(),
                input: "x".to_string(),
            },
            &AgentContext::new("perm"),
        );

        match decision {
            ConstraintDecision::Reject { reason } => {
                assert!(reason.contains("herramienta no autorizada: echo"));
            }
            ConstraintDecision::Allow => panic!("echo no debería estar permitido"),
        }
    }

    #[test]
    fn rejected_action_does_not_execute_tool() {
        // H
        let executed = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));

        let mut harness = Harness::new(1);
        harness.register_tool(Box::new(TrackingTool {
            name: "echo",
            executed: Arc::clone(&executed),
            calls: Arc::clone(&calls),
        }));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = MockAgent::new(vec![AgentAction::InvokeTool {
            tool_name: "echo".to_string(),
            input: "no-debe-correr".to_string(),
        }]);

        let result = harness.run(&mut agent, AgentContext::new("reject-exec"));

        assert!(!executed.load(Ordering::SeqCst));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(result.rejected_actions.len(), 1);
        assert!(result.actions_executed.is_empty());
    }

    #[test]
    fn permitted_action_reaches_tool() {
        // I
        let executed = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));

        let mut harness = Harness::new(2);
        harness.register_tool(Box::new(TrackingTool {
            name: COMPILE,
            executed: Arc::clone(&executed),
            calls: Arc::clone(&calls),
        }));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = MockAgent::new(vec![
            AgentAction::Compile {
                code: "fn main() {}".to_string(),
            },
            AgentAction::Finish {
                summary: "ok".to_string(),
            },
        ]);

        let result = harness.run(&mut agent, AgentContext::new("allow-exec"));

        assert!(executed.load(Ordering::SeqCst));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(result.rejected_actions.is_empty());
        assert!(matches!(
            result.actions_executed[0],
            AgentAction::Compile { .. }
        ));
    }

    #[test]
    fn harness_result_preserves_execution_evidence() {
        // J
        let mut harness = Harness::new(3);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = MockAgent::new(vec![
            AgentAction::Compile {
                code: "fn main() { println!(\"hi\"); }\n".to_string(),
            },
            AgentAction::Finish {
                summary: "done".to_string(),
            },
        ]);

        let result = harness.run(&mut agent, AgentContext::new("evidence"));
        let evidence = result.all_evidence();

        assert!(result.completed);
        assert!(!evidence.is_empty());
        assert!(evidence.iter().any(|e| e.label == "compile_status"));
        assert!(evidence.iter().any(|e| e.label == "summary"));
    }
}
