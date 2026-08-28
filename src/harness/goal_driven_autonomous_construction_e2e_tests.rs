//! E2E: construcción autónoma goal-driven con reparación real de Artifact.
//!
//! Demuestra el ciclo completo:
//! Goal → Evaluation → Unsatisfied → Gap → RecommendedAction → ModelDecision →
//! AgentAction → Tools → Evidence → Artifact mutation → Re-evaluation →
//! Satisfied → FinishAllowed → Finish → Completed

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::agent_loop::LoopStatus;
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_path::ArtifactPath;
    use crate::harness::autonomous_construction::{
        AutonomousConstructionConfig, AutonomousConstructionSession, ConstructionStatus,
    };
    use crate::harness::bridge::introduce_validation_defect;
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::evaluation_engine::SpecificationEvaluationStatus;
    use crate::harness::goal_driven::{
        Goal, GoalDrivenStatus, GoalEvaluator, GoalStatus, RecommendedAction,
        collect_evidence_from_context, select_primary_recommendation,
    };
    use crate::harness::model::{
        AiSessionConfig, DiagnosticContextModelClient, MockModelClient, ModelDecision,
    };
    use crate::harness::observation::AgentObservation;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC, VALIDATE};

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
        Specification::new("spec-gd-e2e", "Crear una API REST")
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

    fn broken_helper_artifact() -> RustArtifact {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        RustArtifact::try_from_files(
            ArtifactId::new("artifact:spec-gd-helper-e2e"),
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
        .with_specification_id(crate::harness::SpecificationId::new("spec-gd-helper-e2e"))
    }

    fn compile_only_spec(id: &str) -> Specification {
        Specification::new(id, "El código debe compilar")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    /// E2E principal: defecto de validación → reparación → goal satisfecha.
    #[test]
    fn goal_driven_autonomous_construction_full_cycle_e2e() {
        let spec = api_construction_spec();
        let goal = Goal::from_specification(spec.clone());
        let invalid = introduce_validation_defect(&api_valid_code());

        // --- Fase inicial: Goal insatisfecha con gap real ---
        let ctx = AgentContext::new("gd-e2e-initial")
            .with_working_code(invalid.clone())
            .with_evaluation_specification(spec.clone());
        let initial_eval =
            GoalEvaluator::new().evaluate(&goal, &collect_evidence_from_context(&ctx));
        assert_ne!(
            initial_eval.status,
            GoalStatus::Satisfied,
            "no debe marcar Satisfied sin evidencia"
        );
        assert!(
            !initial_eval.gap.is_empty(),
            "debe existir GoalGap accionable"
        );
        let initial_rec = select_primary_recommendation(&initial_eval);
        assert!(
            matches!(
                initial_rec,
                RecommendedAction::InvokeTool {
                    kind: CriterionKind::Compile,
                    ..
                }
            ),
            "prioridad Compile ante Validate: {initial_rec:?}"
        );

        // --- Ejecución autónoma goal-driven ---
        let config =
            AutonomousConstructionConfig::new(spec, 12).with_initial_source(invalid.clone());
        let session = AiSessionConfig::new("Crear una API REST", "Api").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let result = AutonomousConstructionSession::run_goal_driven(config, &mut agent);

        // --- Goal satisfecha y loop completado ---
        assert!(
            result.is_goal_satisfied(),
            "status={:?}",
            result.construction.status
        );
        let goal_result = result.goal_result.as_ref().expect("goal_result");
        assert_eq!(goal_result.status, GoalDrivenStatus::GoalSatisfied);
        assert_eq!(result.construction.status, ConstructionStatus::Completed);
        assert_eq!(goal_result.loop_result.status, LoopStatus::Completed);
        assert_eq!(goal_result.final_evaluation.status, GoalStatus::Satisfied);
        assert_eq!(
            result
                .construction
                .specification_evaluation
                .as_ref()
                .map(|e| e.status),
            Some(SpecificationEvaluationStatus::Pass)
        );

        // --- Historial goal-driven: insatisfecha → satisfecha ---
        assert!(goal_result.history.evaluations.len() >= 2);
        assert_ne!(
            goal_result.history.evaluations[0].status,
            GoalStatus::Satisfied
        );
        assert_eq!(
            goal_result.history.evaluations.last().unwrap().status,
            GoalStatus::Satisfied
        );
        assert!(!goal_result.history.gaps[0].is_empty());

        // --- Tools ejecutadas (no simuladas) ---
        let tools = result
            .construction
            .observability
            .tools_executed_sequence
            .clone();
        assert!(tools.iter().any(|t| t == COMPILE), "tools={tools:?}");
        assert!(tools.iter().any(|t| t == VALIDATE), "tools={tools:?}");
        assert!(
            tools.iter().any(|t| t == REPAIR_DIAGNOSTIC),
            "tools={tools:?}"
        );
        assert!(
            tools.iter().any(|t| t == APPLY_CORRECTION),
            "tools={tools:?}"
        );

        let validate_pos = tools.iter().position(|t| t == VALIDATE).unwrap();
        let repair_pos = tools.iter().position(|t| t == REPAIR_DIAGNOSTIC).unwrap();
        let correct_pos = tools.iter().position(|t| t == APPLY_CORRECTION).unwrap();
        assert!(validate_pos < repair_pos);
        assert!(repair_pos < correct_pos);

        // --- Artifact mutado (reparación real, no estado inicial) ---
        let final_artifact = result
            .construction
            .final_artifact
            .as_ref()
            .expect("artifact");
        assert_ne!(
            final_artifact.source(),
            invalid.as_str(),
            "el artifact debe cambiar tras ApplyCorrection"
        );
        assert!(
            final_artifact.source().contains("HTTP"),
            "corrección debe restaurar marcadores Api"
        );
        assert!(!final_artifact.source().contains("NET"));

        // --- Trazas: RecommendedAction → ModelDecision → AgentAction ---
        let first_request = agent.trace.requests.first().expect("request inicial");
        assert!(first_request.goal_evaluation.is_some());
        assert!(first_request.goal_gap.is_some());
        assert!(first_request.recommended_action.is_some());
        assert!(
            agent.trace.parsed_decisions.iter().any(|d| matches!(
                d.as_ref().ok(),
                Some(ModelDecision::RepairDiagnostic { .. })
            )),
            "debe haber ModelDecision RepairDiagnostic tras compile/validate fail"
        );
        assert!(
            agent
                .trace
                .parsed_decisions
                .iter()
                .any(|d| matches!(d.as_ref().ok(), Some(ModelDecision::ApplyCorrection { .. }))),
            "debe haber ModelDecision ApplyCorrection"
        );

        // --- Timeline de criterios: Fail → Pass ---
        let obs = &goal_result.loop_result.history;
        assert!(obs.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                kind: CriterionKind::Validate,
                verdict: EvaluationVerdict::Fail,
                ..
            }
        )));
        assert!(obs.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                kind: CriterionKind::Compile,
                verdict: EvaluationVerdict::Pass,
                ..
            }
        )));
        assert!(
            result
                .construction
                .observability
                .final_criteria
                .iter()
                .all(|c| c.verdict == EvaluationVerdict::Pass)
        );
    }

    /// E2E multi-file: helper roto → compile fail → repair → correction → compile pass → finish.
    #[test]
    fn goal_driven_autonomous_construction_multi_file_repair_e2e() {
        use crate::harness::action_policy::ActionPolicy;
        use crate::harness::goal_driven::GoalDrivenLoop;
        use crate::harness::live_session::build_validate_compile_harness_with_policy;

        let spec = compile_only_spec("spec-gd-helper-e2e");
        let goal = Goal::from_specification(spec.clone());
        let initial_artifact = broken_helper_artifact();
        let helper_path = ArtifactPath::parse("src/helper.rs").unwrap();
        let initial_helper = initial_artifact
            .file(&helper_path)
            .expect("helper inicial")
            .to_string();

        let ctx = AgentContext::new("gd-helper-e2e-initial")
            .with_working_artifact(initial_artifact.clone())
            .with_evaluation_specification(spec.clone());
        let initial_eval =
            GoalEvaluator::new().evaluate(&goal, &collect_evidence_from_context(&ctx));
        assert_ne!(initial_eval.status, GoalStatus::Satisfied);
        assert!(!initial_eval.gap.is_empty());
        assert_eq!(
            initial_eval.gap.primary().unwrap().kind,
            CriterionKind::Compile
        );

        let session = AiSessionConfig::new("compilar helper", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(DiagnosticContextModelClient::new()), session);
        let mut loop_ = GoalDrivenLoop::with_defaults(10);
        let harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());

        let run_ctx = AgentContext::new("gd-helper-e2e")
            .with_working_artifact(initial_artifact)
            .with_evaluation_specification(spec);

        let result = loop_.run(&harness, &mut agent, &goal, run_ctx);

        assert_eq!(result.status, GoalDrivenStatus::GoalSatisfied);
        assert_eq!(result.loop_result.status, LoopStatus::Completed);
        assert_eq!(result.final_evaluation.status, GoalStatus::Satisfied);

        let tools = result.loop_result.tools_executed();
        assert!(tools.iter().any(|t| t == COMPILE));
        assert!(tools.iter().any(|t| t == REPAIR_DIAGNOSTIC));
        assert!(tools.iter().any(|t| t == APPLY_CORRECTION));

        let compile_fail_pos = result
            .loop_result
            .history
            .observations
            .iter()
            .position(|o| {
                matches!(
                    o,
                    AgentObservation::CriterionEvaluated {
                        kind: CriterionKind::Compile,
                        verdict: EvaluationVerdict::Fail,
                        ..
                    }
                )
            })
            .expect("debe observarse compile FAIL antes de reparar");
        let compile_pass_pos = result
            .loop_result
            .history
            .observations
            .iter()
            .position(|o| {
                matches!(
                    o,
                    AgentObservation::CriterionEvaluated {
                        kind: CriterionKind::Compile,
                        verdict: EvaluationVerdict::Pass,
                        ..
                    }
                )
            })
            .expect("debe observarse compile PASS tras reparación");
        assert!(compile_fail_pos < compile_pass_pos);

        let final_artifact = result
            .loop_result
            .final_context
            .working_artifact
            .as_ref()
            .expect("artifact final");
        let final_helper = final_artifact.file(&helper_path).expect("helper final");
        assert_ne!(final_helper, initial_helper.as_str());
        assert!(!final_helper.contains("broken"));
        assert!(
            final_helper.contains('0'),
            "corrección genérica debe producir literal numérico válido: {final_helper}"
        );

        assert!(
            result
                .loop_result
                .history
                .proposed_actions
                .iter()
                .any(|a| matches!(a, AgentAction::ApplyCorrection { .. })),
            "debe proponer ApplyCorrection derivado del contexto diagnóstico"
        );
    }
}
