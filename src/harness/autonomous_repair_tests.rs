//! Tests: capacidad de reparación autónoma con contexto diagnóstico en ModelRequest.

#[cfg(test)]
mod tests {
    use crate::harness::Evidence;
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_path::ArtifactPath;
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::goal_driven::{
        Goal, GoalDrivenLoop, GoalDrivenStatus, GoalEvaluator, RecommendedAction,
        select_primary_recommendation,
    };
    use crate::harness::live_session::build_validate_compile_harness_with_policy;
    use crate::harness::model::{
        AiSessionConfig, DiagnosticContextModelClient, MockModelClient, ModelDecision,
        append_diagnostic_context_to_message_parts, append_recent_evidence_to_message_parts,
        model_request_from_context,
    };
    use crate::harness::observation::AgentObservation;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tools::{COMPILE, REPAIR_DIAGNOSTIC};

    fn compile_only_spec(id: &str) -> Specification {
        Specification::new(id, "El código debe compilar")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    fn broken_helper_artifact() -> RustArtifact {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        RustArtifact::try_from_files(
            ArtifactId::new("artifact:autonomous-repair"),
            "main.rs",
            main.clone(),
            [
                (
                    main,
                    "mod helper;\nfn main() {\n    println!(\"{}\", helper::value());\n}\n"
                        .to_string(),
                ),
                (
                    helper,
                    "pub fn value() -> i32 {\n    broken\n}\n".to_string(),
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn compile_fail_preserves_diagnostic_context_in_model_request() {
        let session = AiSessionConfig::new("compilar", "Generic");
        let mut ctx = AgentContext::new("diag-preserve")
            .with_working_code("fn main() { broken")
            .with_evaluation_specification(compile_only_spec("spec-diag"));
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: false,
            output: "error".to_string(),
            evidence: vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
                Evidence::new("compiler_stderr", "error: expected `}`, found `broken`"),
            ],
            verdict: EvaluationVerdict::Fail,
        });
        ctx.push_observation(AgentObservation::CriterionEvaluated {
            specification_id: crate::harness::SpecificationId::new("spec-diag"),
            criterion_id: crate::harness::AcceptanceCriterionId::new("ac-compile"),
            kind: CriterionKind::Compile,
            verdict: EvaluationVerdict::Fail,
            message: "compilación fallida".to_string(),
            evidence: vec![
                Evidence::new("compile_status", "error"),
                Evidence::new("compiler_stderr", "error: expected `}`, found `broken`"),
            ],
        });

        let request = model_request_from_context(&ctx, &session).expect("request");
        assert!(
            request
                .diagnostic_context
                .compiler_stderr
                .iter()
                .any(|item| item.contains("found `broken`")),
            "compiler_stderr debe preservarse en diagnostic_context"
        );
        assert_eq!(
            request.diagnostic_context.compile_status.as_deref(),
            Some("error")
        );
        let last = request.last_observation.expect("last_observation");
        assert!(
            last.evidence_details
                .iter()
                .any(|(label, _)| label == "compiler_stderr")
        );
    }

    #[test]
    fn recommended_action_repair_diagnostic_after_compile_fail() {
        let goal = Goal::from_specification(compile_only_spec("spec-rec-repair"));
        let evidence = vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "error"),
            Evidence::new("compiler_stderr", "expected `}`"),
        ];
        let evaluation = GoalEvaluator::new().evaluate(&goal, &evidence);
        let rec = select_primary_recommendation(&evaluation);
        assert!(matches!(
            rec,
            RecommendedAction::RepairDiagnostic {
                kind: CriterionKind::Compile,
                ..
            }
        ));
    }

    #[test]
    fn user_message_serializes_compiler_stderr_and_recent_evidence() {
        let session = AiSessionConfig::new("compilar", "Generic");
        let mut ctx = AgentContext::new("msg-serialize")
            .with_working_code("fn main() { broken")
            .with_evaluation_specification(compile_only_spec("spec-msg"));
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: false,
            output: "fail".to_string(),
            evidence: vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
                Evidence::new("compiler_stderr", "error: found `broken` in helper"),
            ],
            verdict: EvaluationVerdict::Fail,
        });

        let request = model_request_from_context(&ctx, &session).expect("request");
        let mut parts = Vec::new();
        append_diagnostic_context_to_message_parts(&mut parts, &request.diagnostic_context);
        append_recent_evidence_to_message_parts(&mut parts, &request.recent_evidence);
        let message = parts.join("\n");
        assert!(message.contains("diagnostic_compiler_stderr_0="));
        assert!(message.contains("found `broken`"));
        assert!(message.contains("recent_evidence_0_label=tool"));
    }

    #[test]
    fn autonomous_compile_repair_e2e_diagnostic_driven_client() {
        let spec = compile_only_spec("spec-autonomous-repair");
        let goal = Goal::from_specification(spec.clone());
        let initial_artifact = broken_helper_artifact();
        let helper_path = ArtifactPath::parse("src/helper.rs").unwrap();

        let session = AiSessionConfig::new("compilar helper", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(DiagnosticContextModelClient::new()), session);
        let mut loop_ = GoalDrivenLoop::with_defaults(12);
        let harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());

        let run_ctx = AgentContext::new("autonomous-repair-e2e")
            .with_working_artifact(initial_artifact)
            .with_evaluation_specification(spec);

        let result = loop_.run(&harness, &mut agent, &goal, run_ctx);

        assert_eq!(result.status, GoalDrivenStatus::GoalSatisfied);
        assert_eq!(result.loop_result.status, LoopStatus::Completed);
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|t| t == COMPILE)
        );
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|t| t == REPAIR_DIAGNOSTIC)
        );
        assert!(
            result
                .loop_result
                .history
                .proposed_actions
                .iter()
                .any(|a| matches!(a, AgentAction::ApplyCorrection { .. }))
        );

        let compile_fail_request = agent
            .trace
            .requests
            .iter()
            .find(|req| {
                req.last_observation.as_ref().is_some_and(|obs| {
                    obs.kind == "criterion_evaluated"
                        && obs.evaluation_verdict.as_deref() == Some("Fail")
                })
            })
            .expect("debe existir ModelRequest tras compile FAIL");
        assert!(
            !compile_fail_request
                .diagnostic_context
                .compiler_stderr
                .is_empty()
                || compile_fail_request
                    .diagnostic_context
                    .evidence_pairs
                    .iter()
                    .any(|(label, _)| label == "compiler_stderr"),
            "diagnóstico de compile debe fluir al ModelRequest"
        );

        let final_artifact = result
            .loop_result
            .final_context
            .working_artifact
            .as_ref()
            .expect("artifact final");
        let final_helper = final_artifact.file(&helper_path).expect("helper final");
        assert!(!final_helper.contains("broken"));
        assert!(final_helper.contains('0'));

        assert!(agent.trace.parsed_decisions.iter().any(|d| matches!(
            d.as_ref().ok(),
            Some(ModelDecision::RepairDiagnostic { .. })
        )));
    }

    #[test]
    fn mock_model_client_still_passes_validation_e2e_regression() {
        use crate::harness::bridge::introduce_validation_defect;
        use crate::harness::tools::{APPLY_CORRECTION, VALIDATE};

        let invalid = introduce_validation_defect("fn main() {\n    println!(\"HTTP\");\n}\n");
        let mut harness = crate::harness::Harness::new(12);
        harness.register_tool(Box::new(crate::harness::tools::ValidationTool));
        harness.register_tool(Box::new(crate::harness::tools::RepairDiagnosticTool));
        harness.register_tool(Box::new(crate::harness::tools::CorrectionTool));
        harness.register_tool(Box::new(crate::harness::tools::CompileTool));
        harness.register_constraint(Box::new(
            crate::harness::ToolPermissionConstraint::default_constructor_tools(),
        ));

        let session = AiSessionConfig::new("Crear una API REST", "Api");
        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let result = AgentLoop::new(10).run(
            &harness,
            &mut agent,
            AgentContext::new("regression-mock").with_working_code(invalid),
        );

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(result.tools_executed().iter().any(|t| t == VALIDATE));
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == REPAIR_DIAGNOSTIC)
        );
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == APPLY_CORRECTION)
        );
    }

    #[test]
    fn openai_user_message_includes_diagnostic_context_fields() {
        let session = AiSessionConfig::new("compilar", "Generic");
        let mut ctx = AgentContext::new("openai-diag")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_only_spec("spec-openai-diag"));
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: false,
            output: "err".to_string(),
            evidence: vec![
                Evidence::new("compile_status", "error"),
                Evidence::new("compiler_stderr", "syntax error"),
            ],
            verdict: EvaluationVerdict::Fail,
        });
        let request = model_request_from_context(&ctx, &session).expect("request");

        let mut parts = Vec::new();
        append_diagnostic_context_to_message_parts(&mut parts, &request.diagnostic_context);
        let message = parts.join("\n");
        assert!(message.contains("diagnostic_compile_status=error"));
        assert!(message.contains("diagnostic_compiler_stderr_0=syntax error"));
    }

    #[test]
    fn mock_infers_compile_correction_from_artifact_files_without_stderr() {
        use crate::harness::model::{
            ArtifactFileSnapshot, MockModelClient, ModelClient, ModelDecision, ModelRequest,
            SerializedCriterionGap, SerializedDiagnosticContext, SerializedGoalEvaluation,
            SerializedGoalGap, SerializedObservation, SerializedRecommendedAction,
        };

        let helper = "pub fn value() -> i32 {\n    broken\n}\n".to_string();
        let request = ModelRequest {
            goal: "compilar".to_string(),
            step: 4,
            user_request: "compilar helper".to_string(),
            plan_kind: Some("Generic".to_string()),
            working_code: Some(
                "mod helper;\nfn main() { println!(\"{}\", helper::value()); }\n".to_string(),
            ),
            artifact_id: Some("artifact:test".to_string()),
            artifact_language: Some("Rust".to_string()),
            artifact_revision: Some(1),
            artifact_primary_path: Some("src/main.rs".to_string()),
            artifact_files: vec![
                ArtifactFileSnapshot {
                    path: "src/main.rs".to_string(),
                    source: "mod helper;\nfn main() { println!(\"{}\", helper::value()); }\n"
                        .to_string(),
                },
                ArtifactFileSnapshot {
                    path: "src/helper.rs".to_string(),
                    source: helper,
                },
            ],
            last_observation: Some(SerializedObservation {
                kind: "tool_outcome".to_string(),
                tool_name: Some(REPAIR_DIAGNOSTIC.to_string()),
                success: Some(true),
                summary: "tool:repair_diagnostic:ok".to_string(),
                validator_errors: Vec::new(),
                repairer_feedback: vec!["revisar compile".to_string()],
                evidence_labels: Vec::new(),
                evidence_details: Vec::new(),
                evaluation_verdict: None,
                specification_id: None,
                criterion_id: None,
                criterion_kind: None,
                evaluation_message: None,
            }),
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec".to_string(),
                status: "Unsatisfied".to_string(),
                criteria_total: 1,
                criteria_pass: 0,
                criteria_fail: 1,
                criteria_insufficient: 0,
                message: "compile fail".to_string(),
            }),
            goal_gap: Some(SerializedGoalGap {
                unsatisfied_count: 1,
                gaps: vec![SerializedCriterionGap {
                    criterion_id: "ac-compile".to_string(),
                    kind: "Compile".to_string(),
                    verdict: "Fail".to_string(),
                    message: "compilación fallida".to_string(),
                    suggested_action: Some(COMPILE.to_string()),
                }],
            }),
            recommended_action: Some(SerializedRecommendedAction {
                kind: "RepairDiagnostic".to_string(),
                tool_name: Some(REPAIR_DIAGNOSTIC.to_string()),
                criterion_id: Some("ac-compile".to_string()),
                criterion_kind: Some("Compile".to_string()),
                priority: 0,
                reason: "compilación fallida".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext {
                compile_status: Some("error".to_string()),
                ..SerializedDiagnosticContext::default()
            },
            system_prompt: String::new(),
        };

        let decision = MockModelClient::new()
            .complete(&request)
            .expect("mock response");
        let parsed =
            crate::harness::model::parse_model_response(&decision.raw_text).expect("parse");
        assert!(
            matches!(parsed, ModelDecision::ApplyCorrection { .. }),
            "sin stderr explícito debe inferir corrección desde artifact_files: {parsed:?}"
        );
    }
}
