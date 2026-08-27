//! Tests: Builder → Initial Artifact multi-file por PlanKind.

#[cfg(test)]
mod tests {
    use crate::builder::{initial_artifact_definition_for_kind, initial_source_for_kind};
    use crate::harness::ArtifactMaterialization;
    use crate::harness::ArtifactPath;
    use crate::harness::tools::CompileTool;
    use crate::harness::{
        AcceptanceCriterion, AgentContext, AutonomousConstructionConfig,
        AutonomousConstructionSession, COMPILE, ConstructionStatus, Correction, CriterionKind,
        EvaluationEngine, EvaluationVerdict, MockModelClient, Requirement, Specification,
        SpecificationEvaluationStatus, SpecificationId, Tool, initial_artifact_from_plan,
        plan_specification,
    };
    use crate::planner::PlanKind;
    use std::process::Command;
    use std::sync::Mutex;

    static COMPILE_LOCK: Mutex<()> = Mutex::new(());

    fn auth_spec() -> Specification {
        Specification::new("spec-builder-auth", "Crear un sistema de autenticación")
            .with_requirements(vec![Requirement::new("req-auth", "login")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-auth")]),
            ])
    }

    #[test]
    fn single_file_plan_kinds_still_valid() {
        // A + P (single-file kinds)
        for kind in [PlanKind::Api, PlanKind::Calculator, PlanKind::Generic] {
            let def = initial_artifact_definition_for_kind(kind);
            assert_eq!(def.file_count(), 1);
            assert_eq!(def.primary_path, "src/main.rs");
            assert_eq!(def.primary_source(), initial_source_for_kind(kind));
        }
    }

    #[test]
    fn authentication_generates_expected_files() {
        // B
        let def = initial_artifact_definition_for_kind(PlanKind::Authentication);
        assert_eq!(def.file_count(), 2);
        let paths: Vec<_> = def.files().map(|(path, _)| path).collect();
        assert!(paths.contains(&"src/main.rs"));
        assert!(paths.contains(&"src/auth.rs"));
    }

    #[test]
    fn definition_is_deterministic() {
        // C
        let first = initial_artifact_definition_for_kind(PlanKind::Authentication);
        let second = initial_artifact_definition_for_kind(PlanKind::Authentication);
        assert_eq!(first, second);
    }

    #[test]
    fn primary_is_valid_and_source_matches() {
        // D + E
        let spec_id = SpecificationId::new("spec-primary");
        let plan = crate::planner::plan_from_goal("Crear un sistema de autenticación");
        let artifact = initial_artifact_from_plan(spec_id, &plan, "main.rs");
        assert_eq!(artifact.primary_path().as_str(), "src/main.rs");
        assert_eq!(
            artifact.source(),
            initial_source_for_kind(PlanKind::Authentication)
        );
        assert!(artifact.source().contains("mod auth"));
    }

    #[test]
    fn siblings_exist_in_files() {
        // F
        let plan = crate::planner::plan_from_goal("Crear un sistema de autenticación");
        let artifact =
            initial_artifact_from_plan(SpecificationId::new("spec-siblings"), &plan, "main.rs");
        let auth = ArtifactPath::parse("src/auth.rs").unwrap();
        assert_eq!(artifact.file_count(), 2);
        assert!(
            artifact
                .file(&auth)
                .unwrap()
                .contains("validar_credenciales")
        );
    }

    #[test]
    fn initial_artifact_from_plan_preserves_ids_and_revision() {
        // G
        let spec = auth_spec();
        let planned = plan_specification(&spec).expect("plan");
        let artifact = initial_artifact_from_plan(spec.id.clone(), &planned.plan, "main.rs");
        assert_eq!(artifact.id().as_str(), "artifact:spec-builder-auth");
        assert_eq!(
            artifact.specification_id().map(|id| id.as_str()),
            Some("spec-builder-auth")
        );
        assert_eq!(artifact.revision(), 0);
    }

    #[test]
    fn materialization_writes_all_files() {
        // H
        let plan = crate::planner::plan_from_goal("Crear un sistema de autenticación");
        let artifact =
            initial_artifact_from_plan(SpecificationId::new("spec-mat"), &plan, "main.rs");
        let mat = ArtifactMaterialization::from_artifact(&artifact).expect("materialize");
        assert!(mat.root().join("src/main.rs").is_file());
        assert!(mat.root().join("src/auth.rs").is_file());
        assert!(mat.root().join("Cargo.toml").is_file());
    }

    #[test]
    fn builder_multi_file_compiles_via_materialized_crate() {
        // I + J
        let _guard = COMPILE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let plan = crate::planner::plan_from_goal("Crear un sistema de autenticación");
        let artifact =
            initial_artifact_from_plan(SpecificationId::new("spec-compile"), &plan, "main.rs");
        let mat = ArtifactMaterialization::from_artifact(&artifact).expect("materialize");
        let output = Command::new("cargo")
            .args(["check", "--quiet", "--manifest-path"])
            .arg(mat.root().join("Cargo.toml"))
            .output()
            .expect("cargo check");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(artifact.source().contains("auth::validar_credenciales"));
    }

    #[test]
    fn sibling_correction_only_mutates_target_file() {
        // K + L
        let plan = crate::planner::plan_from_goal("Crear un sistema de autenticación");
        let mut artifact =
            initial_artifact_from_plan(SpecificationId::new("spec-corr"), &plan, "main.rs");
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let auth = ArtifactPath::parse("src/auth.rs").unwrap();
        let main_before = artifact.file(&main).unwrap().to_string();
        let rev = artifact.revision();

        Correction::replace_file_text(auth.clone(), "password", "secret")
            .apply_to_artifact(&mut artifact)
            .unwrap();

        assert_eq!(artifact.file(&main), Some(main_before.as_str()));
        assert!(artifact.file(&auth).unwrap().contains("secret"));
        assert_eq!(artifact.revision(), rev + 1);
    }

    #[test]
    fn single_file_autonomous_construction_still_works() {
        // M
        let spec = Specification::new("spec-builder-api", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compila")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);
        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(spec, 8),
            Box::new(MockModelClient::new()),
        );
        assert_eq!(result.status, ConstructionStatus::Completed);
        assert_eq!(
            result
                .final_artifact
                .as_ref()
                .expect("artifact")
                .file_count(),
            1
        );
    }

    #[test]
    fn autonomous_construction_without_override_starts_multi_file() {
        // N
        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(auth_spec(), 8),
            Box::new(MockModelClient::new()),
        );
        assert_eq!(result.status, ConstructionStatus::Completed);
        let artifact = result.final_artifact.as_ref().expect("artifact");
        assert_eq!(artifact.file_count(), 2);
        assert!(artifact.source().contains("mod auth"));
    }

    #[test]
    fn e2e_specification_plan_builder_compile_evaluation() {
        // O
        let spec = auth_spec();
        let planned = plan_specification(&spec).expect("plan");
        assert_eq!(planned.plan.kind, PlanKind::Authentication);

        let initial = initial_artifact_from_plan(spec.id.clone(), &planned.plan, "main.rs");
        assert_eq!(initial.file_count(), 2);

        let result = AutonomousConstructionSession::run_with_model_client(
            AutonomousConstructionConfig::new(spec, 8),
            Box::new(MockModelClient::new()),
        );
        assert_eq!(result.status, ConstructionStatus::Completed);
        assert_eq!(
            result.build_plan.as_ref().map(|p| p.plan.kind),
            Some(PlanKind::Authentication)
        );
        let final_artifact = result.final_artifact.as_ref().expect("artifact");
        assert_eq!(final_artifact.file_count(), 2);
        assert!(result.tools_executed().iter().any(|name| name == COMPILE));
        assert_eq!(
            result.specification_evaluation.as_ref().map(|e| e.status),
            Some(SpecificationEvaluationStatus::Pass)
        );

        // EvaluationEngine consume el artifact vía Evidence del CompileTool
        let _guard = COMPILE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ctx = AgentContext::new("eval").with_working_artifact(final_artifact.clone());
        let tool_result = CompileTool.execute("", &ctx);
        let evaluation = EvaluationEngine::new().evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile),
            &tool_result.evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
        assert!(tool_result.success);
        assert!(
            tool_result
                .evidence
                .iter()
                .any(|e| e.label == "compile_status" && e.detail == "ok")
        );
    }

    #[test]
    fn all_plan_kinds_regression_via_initial_artifact_from_plan() {
        // P
        let cases = [
            ("Crear una API REST", PlanKind::Api, 1),
            ("Crear una calculadora", PlanKind::Calculator, 1),
            (
                "Crear un sistema de autenticación",
                PlanKind::Authentication,
                2,
            ),
            ("Crear inventario", PlanKind::Generic, 1),
        ];
        for (goal, expected_kind, expected_files) in cases {
            let spec = Specification::new(format!("spec-{expected_kind:?}"), goal);
            let planned = plan_specification(&spec).expect("plan");
            assert_eq!(planned.plan.kind, expected_kind);
            let artifact = initial_artifact_from_plan(spec.id.clone(), &planned.plan, "main.rs");
            assert_eq!(artifact.file_count(), expected_files);
            assert_eq!(artifact.revision(), 0);
            assert!(!artifact.source().trim().is_empty());
            assert_eq!(artifact.source(), initial_source_for_kind(expected_kind));
        }
    }
}
