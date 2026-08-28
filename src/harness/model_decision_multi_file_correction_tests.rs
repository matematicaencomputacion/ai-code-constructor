//! Tests: ModelDecision path → AiAgent → Correction.path (multi-file).

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::{ActionPolicy, ApplyCorrectionConstraint};
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_path::ArtifactPath;
    use crate::harness::constraint::{Constraint, ConstraintDecision};
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::model::{
        AiSessionConfig, ModelClient, ModelDecision, ModelError, ModelRequest, ModelResponse,
        ModelResponseError, StructuredCorrection, parse_model_response, serialize_decision,
        structured_to_correction, validate_apply_correction,
    };
    use crate::harness::observation::AgentObservation;
    use crate::harness::runtime::Harness;
    use crate::harness::specification::SpecificationId;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tools::{APPLY_CORRECTION, CompileTool, CorrectionTool};

    fn broken_helper_artifact() -> RustArtifact {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        RustArtifact::try_from_files(
            ArtifactId::new("art-md-path"),
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
        .with_specification_id(SpecificationId::new("spec-md-path"))
    }

    fn multi_helper_artifact() -> RustArtifact {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        RustArtifact::try_from_files(
            ArtifactId::new("art-md-path"),
            "main.rs",
            main.clone(),
            [
                (
                    main,
                    "mod helper;\nfn main() {\n    println!(\"{}\", helper::value());\n}\n"
                        .to_string(),
                ),
                (helper, "pub fn value() -> i32 {\n    1\n}\n".to_string()),
            ],
        )
        .unwrap()
        .with_specification_id(SpecificationId::new("spec-md-path"))
    }

    #[test]
    fn parse_legacy_apply_correction_without_path() {
        // A
        let parsed = parse_model_response(
            r#"{"action":"apply_correction","corrections":[{"operation":"replace_text","search":"NET","replacement":"HTTP"}]}"#,
        )
        .expect("parse");
        let ModelDecision::ApplyCorrection { corrections } = parsed else {
            panic!("expected apply_correction");
        };
        assert!(matches!(
            &corrections[0],
            StructuredCorrection::ReplaceText { path: None, .. }
        ));
    }

    #[test]
    fn parse_apply_correction_with_path() {
        // B
        let parsed = parse_model_response(
            r#"{"action":"apply_correction","corrections":[{"operation":"replace_text","path":"src/helper.rs","search":"1","replacement":"2"}]}"#,
        )
        .expect("parse");
        let ModelDecision::ApplyCorrection { corrections } = parsed else {
            panic!("expected apply_correction");
        };
        match &corrections[0] {
            StructuredCorrection::ReplaceText {
                path: Some(path),
                search,
                replacement,
            } => {
                assert_eq!(path, "src/helper.rs");
                assert_eq!(search, "1");
                assert_eq!(replacement, "2");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn serialize_parse_round_trip_preserves_path() {
        // C
        let decision = ModelDecision::ApplyCorrection {
            corrections: vec![StructuredCorrection::ReplaceText {
                path: Some("src/helper.rs".to_string()),
                search: "old".to_string(),
                replacement: "new".to_string(),
            }],
        };
        let raw = serialize_decision(&decision);
        let parsed = parse_model_response(&raw).expect("round-trip");
        assert_eq!(parsed, decision);
    }

    #[test]
    fn structured_to_correction_maps_valid_path() {
        // D
        let item = StructuredCorrection::ReplaceText {
            path: Some("src/helper.rs".to_string()),
            search: "1".to_string(),
            replacement: "2".to_string(),
        };
        let correction = structured_to_correction(&item).expect("map");
        assert_eq!(
            correction.path.as_ref().map(ArtifactPath::as_str),
            Some("src/helper.rs")
        );
    }

    #[test]
    fn structured_to_correction_legacy_none_path() {
        // E
        let item = StructuredCorrection::ReplaceText {
            path: None,
            search: "a".to_string(),
            replacement: "b".to_string(),
        };
        let correction = structured_to_correction(&item).expect("map");
        assert!(correction.path.is_none());
    }

    #[test]
    fn invalid_model_path_is_controlled_error() {
        // F + G
        let item = StructuredCorrection::ReplaceText {
            path: Some("../etc/passwd".to_string()),
            search: "x".to_string(),
            replacement: "y".to_string(),
        };
        let err = structured_to_correction(&item).unwrap_err();
        assert!(matches!(err, ModelResponseError::InvalidCorrection(_)));

        let missing_file = validate_apply_correction(
            &[StructuredCorrection::ReplaceText {
                path: Some("src/missing.rs".to_string()),
                search: "x".to_string(),
                replacement: "y".to_string(),
            }],
            Some(&multi_helper_artifact()),
        )
        .unwrap_err();
        assert!(matches!(
            missing_file,
            ModelResponseError::InvalidCorrection(_)
        ));
    }

    #[test]
    fn ai_agent_maps_path_to_correction() {
        // D via AiAgent
        struct ScriptModelClient {
            raw: String,
        }
        impl ModelClient for ScriptModelClient {
            fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
                Ok(ModelResponse {
                    raw_text: self.raw.clone(),
                })
            }
        }

        let decision = ModelDecision::ApplyCorrection {
            corrections: vec![StructuredCorrection::ReplaceText {
                path: Some("src/helper.rs".to_string()),
                search: "1".to_string(),
                replacement: "2".to_string(),
            }],
        };
        let mut agent = AiAgent::new(
            Box::new(ScriptModelClient {
                raw: serialize_decision(&decision),
            }),
            AiSessionConfig::new("r".to_string(), "Api".to_string()),
        );
        let action =
            agent.propose(&AgentContext::new("a").with_working_artifact(multi_helper_artifact()));
        match action {
            AgentAction::ApplyCorrection { corrections } => {
                assert_eq!(
                    corrections[0].path.as_ref().map(ArtifactPath::as_str),
                    Some("src/helper.rs")
                );
            }
            other => panic!("expected ApplyCorrection, got {other:?}"),
        }
    }

    #[test]
    fn helper_correction_preserves_primary_and_metadata() {
        // H + I + K + L + M
        let mut artifact = multi_helper_artifact();
        let id = artifact.id().clone();
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let main_before = artifact.file(&main).unwrap().to_string();
        let rev = artifact.revision();

        let correction = structured_to_correction(&StructuredCorrection::ReplaceText {
            path: Some("src/helper.rs".to_string()),
            search: "1".to_string(),
            replacement: "2".to_string(),
        })
        .expect("map");
        correction.apply_to_artifact(&mut artifact).unwrap();

        assert_eq!(artifact.id(), &id);
        assert_eq!(
            artifact.specification_id().map(|s| s.as_str()),
            Some("spec-md-path")
        );
        assert_eq!(artifact.file(&main), Some(main_before.as_str()));
        assert_eq!(
            artifact.file(&helper),
            Some("pub fn value() -> i32 {\n    2\n}\n")
        );
        assert_eq!(artifact.revision(), rev + 1);

        // legacy primary
        let mut single = RustArtifact::new("main.rs", "alpha");
        structured_to_correction(&StructuredCorrection::ReplaceText {
            path: None,
            search: "alpha".to_string(),
            replacement: "beta".to_string(),
        })
        .expect("legacy")
        .apply_to_artifact(&mut single)
        .unwrap();
        assert_eq!(single.source(), "beta");
    }

    #[test]
    fn nonexistent_file_not_created_by_mapping() {
        // J
        let mut artifact = multi_helper_artifact();
        let count_before = artifact.file_count();
        let correction = structured_to_correction(&StructuredCorrection::ReplaceText {
            path: Some("src/new.rs".to_string()),
            search: "x".to_string(),
            replacement: "y".to_string(),
        })
        .expect("valid path syntax");
        assert!(correction.apply_to_artifact(&mut artifact).is_err());
        assert_eq!(artifact.file_count(), count_before);
    }

    #[test]
    fn apply_correction_constraint_validates_target_file() {
        // N
        let ctx = AgentContext::new("c").with_working_artifact(multi_helper_artifact());
        let bad_file = ApplyCorrectionConstraint.check(
            &AgentAction::ApplyCorrection {
                corrections: vec![
                    structured_to_correction(&StructuredCorrection::ReplaceText {
                        path: Some("src/absent.rs".to_string()),
                        search: "1".to_string(),
                        replacement: "2".to_string(),
                    })
                    .expect("path syntax ok"),
                ],
            },
            &ctx,
        );
        assert!(matches!(bad_file, ConstraintDecision::Reject { .. }));

        let bad_search = ApplyCorrectionConstraint.check(
            &AgentAction::ApplyCorrection {
                corrections: vec![
                    structured_to_correction(&StructuredCorrection::ReplaceText {
                        path: Some("src/helper.rs".to_string()),
                        search: "zzz".to_string(),
                        replacement: "2".to_string(),
                    })
                    .expect("map"),
                ],
            },
            &ctx,
        );
        assert!(matches!(bad_search, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn legacy_regression_without_path() {
        // O
        struct ScriptModelClient {
            raw: String,
        }
        impl ModelClient for ScriptModelClient {
            fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
                Ok(ModelResponse {
                    raw_text: self.raw.clone(),
                })
            }
        }

        let decision = ModelDecision::ApplyCorrection {
            corrections: vec![StructuredCorrection::ReplaceText {
                path: None,
                search: "NET".to_string(),
                replacement: "HTTP".to_string(),
            }],
        };
        let mut agent = AiAgent::new(
            Box::new(ScriptModelClient {
                raw: serialize_decision(&decision),
            }),
            AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string()),
        );
        let mut ctx = AgentContext::new("legacy").with_working_code("Servidor NET");
        ctx.step = 1;
        let action = agent.propose(&ctx);
        match action {
            AgentAction::ApplyCorrection { corrections } => {
                assert!(corrections.iter().all(|c| c.path.is_none()));
            }
            other => panic!("expected ApplyCorrection, got {other:?}"),
        }
    }

    #[test]
    fn e2e_observation_driven_multi_file_correction() {
        // P
        struct ObservationDrivenAgent {
            corrected: bool,
        }
        impl Agent for ObservationDrivenAgent {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                let compile_pass = ctx.observation_history.iter().any(|obs| {
                    matches!(
                        obs,
                        AgentObservation::CriterionEvaluated {
                            kind: CriterionKind::Compile,
                            verdict: EvaluationVerdict::Pass,
                            ..
                        }
                    )
                });
                if compile_pass {
                    return AgentAction::Finish {
                        summary: "helper fixed".to_string(),
                    };
                }
                let compile_failed = ctx.observation_history.iter().any(|obs| {
                    matches!(
                        obs,
                        AgentObservation::CriterionEvaluated {
                            kind: CriterionKind::Compile,
                            verdict: EvaluationVerdict::Fail,
                            ..
                        }
                    )
                });
                if compile_failed && !self.corrected {
                    self.corrected = true;
                    return AgentAction::ApplyCorrection {
                        corrections: vec![
                            structured_to_correction(&StructuredCorrection::ReplaceText {
                                path: Some("src/helper.rs".to_string()),
                                search: "broken".to_string(),
                                replacement: "2".to_string(),
                            })
                            .expect("map"),
                        ],
                    };
                }
                AgentAction::Compile {
                    code: String::new(),
                }
            }
        }

        let spec = Specification::new("spec-e2e-path", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compila")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);

        let mut harness = Harness::new(8);
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_constraint(Box::new(ActionPolicy::default_session_policy()));

        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let ctx = AgentContext::new("e2e-path")
            .with_working_artifact(broken_helper_artifact())
            .with_evaluation_specification(spec);

        let result = AgentLoop::new(8).run(
            &harness,
            &mut ObservationDrivenAgent { corrected: false },
            ctx,
        );
        assert_eq!(result.status, LoopStatus::Completed);
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == APPLY_CORRECTION)
        );
        let final_artifact = result
            .final_context
            .working_artifact
            .as_ref()
            .expect("artifact");
        assert_eq!(
            final_artifact.file(&helper),
            Some("pub fn value() -> i32 {\n    2\n}\n")
        );
        assert!(final_artifact.source().contains("helper::value()"));
    }
}
