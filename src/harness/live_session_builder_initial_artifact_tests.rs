//! Tests: Specification → Builder → LiveSession Initial Artifact.

#[cfg(test)]
mod tests {
    use crate::builder::initial_source_for_kind;
    use crate::harness::agent_loop::LoopStatus;
    use crate::harness::artifact_path::ArtifactPath;
    use crate::harness::model::{
        ModelClient, ModelError, ModelRequest, ModelResponse, StructuredCorrection,
        serialize_decision,
    };
    use crate::harness::tools::APPLY_CORRECTION;
    use crate::harness::{
        AcceptanceCriterion, AgentContext, ArtifactMaterialization, COMPILE, CompileTool,
        CriterionKind, LiveSessionConfig, LiveSessionFromSpecificationOptions, MockModelClient,
        Requirement, Specification, Tool, run_live_agent_session_with_client,
    };
    use crate::planner::PlanKind;
    use std::process::Command;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static COMPILE_LOCK: Mutex<()> = Mutex::new(());

    fn api_spec() -> Specification {
        Specification::new("spec-live-api", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compila")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    fn calculator_spec() -> Specification {
        Specification::new("spec-live-calc", "Crear una calculadora")
            .with_requirements(vec![Requirement::new("req-c", "compila")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    fn generic_spec() -> Specification {
        Specification::new("spec-live-generic", "Crear inventario")
            .with_requirements(vec![Requirement::new("req-c", "compila")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    fn auth_spec() -> Specification {
        Specification::new("spec-live-auth", "Crear un sistema de autenticación")
            .with_requirements(vec![Requirement::new("req-c", "compila")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    #[test]
    fn api_specification_produces_live_session_config() {
        // A
        let config = LiveSessionConfig::from_specification(api_spec()).expect("config");
        assert_eq!(config.plan_kind, "Api");
        assert_eq!(config.user_request, "Crear una API REST");
        assert!(config.working_artifact.is_some());
        assert_eq!(config.working_artifact.as_ref().unwrap().file_count(), 1);
        assert!(config.source_matches_builder(PlanKind::Api));
    }

    #[test]
    fn calculator_and_generic_remain_single_file() {
        // B + C
        let calc = LiveSessionConfig::from_specification(calculator_spec()).expect("calc");
        assert_eq!(calc.plan_kind, "Calculator");
        assert_eq!(calc.working_artifact.as_ref().unwrap().file_count(), 1);

        let generic = LiveSessionConfig::from_specification(generic_spec()).expect("generic");
        assert_eq!(generic.plan_kind, "Generic");
        assert_eq!(generic.working_artifact.as_ref().unwrap().file_count(), 1);
    }

    #[test]
    fn authentication_produces_multi_file_artifact() {
        // D–G + N
        let config = LiveSessionConfig::from_specification(auth_spec()).expect("auth");
        let artifact = config.working_artifact.as_ref().expect("artifact");
        assert_eq!(config.plan_kind, "Authentication");
        assert_eq!(artifact.file_count(), 2);
        assert_eq!(artifact.primary_path().as_str(), "src/main.rs");
        assert_eq!(artifact.id().as_str(), "artifact:spec-live-auth");
        assert_eq!(
            artifact.specification_id().map(|id| id.as_str()),
            Some("spec-live-auth")
        );
        assert_eq!(artifact.revision(), 0);

        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let auth = ArtifactPath::parse("src/auth.rs").unwrap();
        assert!(artifact.file(&main).unwrap().contains("mod auth"));
        assert!(
            artifact
                .file(&auth)
                .unwrap()
                .contains("validar_credenciales")
        );
    }

    #[test]
    fn live_session_receives_working_artifact_multi_file() {
        // H
        let config = LiveSessionConfig::from_specification(auth_spec()).expect("config");
        let result =
            run_live_agent_session_with_client(Box::new(MockModelClient::new()), config, None)
                .expect("session");
        let artifact = result
            .loop_result
            .final_context
            .working_artifact
            .as_ref()
            .expect("artifact");
        assert_eq!(artifact.file_count(), 2);
    }

    #[test]
    fn compile_tool_materializes_full_crate_from_session_artifact() {
        // I
        let _guard = COMPILE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let config = LiveSessionConfig::from_specification(auth_spec()).expect("config");
        let artifact = config.working_artifact.clone().expect("artifact");
        let ctx = AgentContext::new("compile-check").with_working_artifact(artifact.clone());
        let tool_result = CompileTool.execute("", &ctx);
        assert!(tool_result.success);

        let mat = ArtifactMaterialization::from_artifact(&artifact).expect("mat");
        let output = Command::new("cargo")
            .args(["check", "--quiet", "--manifest-path"])
            .arg(mat.root().join("Cargo.toml"))
            .output()
            .expect("cargo");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn working_code_represents_primary_only() {
        // K
        let config = LiveSessionConfig::from_specification(auth_spec()).expect("config");
        let artifact = config.working_artifact.as_ref().expect("artifact");
        assert_eq!(config.working_code, artifact.source());
        assert_eq!(
            config.working_code,
            initial_source_for_kind(PlanKind::Authentication)
        );
        assert!(!config.working_code.contains("pub fn validar_credenciales"));
    }

    #[test]
    fn legacy_manual_config_still_works() {
        // L
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        );
        assert!(config.evaluation_specification.is_none());
        let result =
            run_live_agent_session_with_client(Box::new(MockModelClient::new()), config, None);
        assert!(result.is_ok());
    }

    #[test]
    fn without_evaluation_specification_option() {
        // M
        let config = LiveSessionConfig::from_specification_with_options(
            auth_spec(),
            LiveSessionFromSpecificationOptions {
                attach_evaluation_specification: false,
                ..LiveSessionFromSpecificationOptions::default()
            },
        )
        .expect("config");
        assert!(config.evaluation_specification.is_none());
        assert!(config.working_artifact.is_some());
    }

    #[test]
    fn e2e_observation_driven_correction_on_auth_sibling() {
        // J + O
        struct ObservationDrivenModelClient {
            corrected: AtomicBool,
        }

        impl ModelClient for ObservationDrivenModelClient {
            fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
                let compile_pass = request.recent_observations.iter().any(|obs| {
                    obs.kind == "criterion_evaluated"
                        && obs.evaluation_verdict.as_deref() == Some("Pass")
                });
                if compile_pass {
                    return Ok(ModelResponse {
                        raw_text: serialize_decision(&crate::harness::ModelDecision::Finish {
                            summary: "auth fixed".to_string(),
                        }),
                    });
                }

                let compile_failed = request.recent_observations.iter().any(|obs| {
                    obs.kind == "criterion_evaluated"
                        && obs.evaluation_verdict.as_deref() == Some("Fail")
                });

                if compile_failed && !self.corrected.load(Ordering::SeqCst) {
                    self.corrected.store(true, Ordering::SeqCst);
                    let decision = crate::harness::ModelDecision::ApplyCorrection {
                        corrections: vec![StructuredCorrection::ReplaceText {
                            path: Some("src/auth.rs".to_string()),
                            search: "broken".to_string(),
                            replacement: "!password.is_empty()".to_string(),
                        }],
                    };
                    return Ok(ModelResponse {
                        raw_text: serialize_decision(&decision),
                    });
                }

                Ok(ModelResponse {
                    raw_text: serialize_decision(&crate::harness::ModelDecision::Compile {
                        code: String::new(),
                    }),
                })
            }
        }

        let mut config = LiveSessionConfig::from_specification(auth_spec()).expect("config");
        let auth = ArtifactPath::parse("src/auth.rs").unwrap();
        {
            let artifact = config.working_artifact.as_mut().expect("artifact");
            artifact
                .upsert_file(
                    auth.clone(),
                    "pub fn validar_credenciales(usuario: &str, password: &str) -> bool {\n    broken\n}\n",
                )
                .expect("upsert");
            config.working_code = artifact.source().to_string();
        }

        let result = run_live_agent_session_with_client(
            Box::new(ObservationDrivenModelClient {
                corrected: AtomicBool::new(false),
            }),
            config,
            None,
        )
        .expect("session");

        assert_eq!(result.loop_result.status, LoopStatus::Completed);
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|tool| tool == APPLY_CORRECTION)
        );
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|tool| tool == COMPILE)
        );

        let final_artifact = result
            .loop_result
            .final_context
            .working_artifact
            .as_ref()
            .expect("artifact");
        assert!(
            final_artifact
                .file(&auth)
                .unwrap()
                .contains("!password.is_empty()")
        );
        assert!(final_artifact.source().contains("mod auth"));
    }

    trait LiveSessionConfigTestExt {
        fn source_matches_builder(&self, kind: PlanKind) -> bool;
    }

    impl LiveSessionConfigTestExt for LiveSessionConfig {
        fn source_matches_builder(&self, kind: PlanKind) -> bool {
            self.working_artifact
                .as_ref()
                .map(|artifact| artifact.source() == initial_source_for_kind(kind))
                .unwrap_or(false)
        }
    }
}
