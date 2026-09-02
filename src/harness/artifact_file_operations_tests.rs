//! Tests: operaciones estructurales sobre RustArtifact.

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::{ActionPolicy, ApplyFileOperationsConstraint};
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_file_operation::{
        ArtifactFileOperation, apply_file_operations_to_artifact, validate_file_operations,
    };
    use crate::harness::artifact_materialization::ArtifactMaterialization;
    use crate::harness::artifact_path::ArtifactPath;
    use crate::harness::constraint::{Constraint, ConstraintDecision};
    use crate::harness::context::AgentContext;
    use crate::harness::correction::Correction;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::model::{
        ModelDecision, StructuredFileOperation, parse_model_response, serialize_decision,
        structured_to_file_operation,
    };
    use crate::harness::observation::AgentObservation;
    use crate::harness::runtime::Harness;
    use crate::harness::specification::SpecificationId;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tools::{
        APPLY_FILE_OPERATIONS, CompileTool, CorrectionTool, FileOperationsTool,
    };
    use crate::harness::{COMPILE, Tool};
    use std::process::Command;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static COMPILE_LOCK: Mutex<()> = Mutex::new(());

    fn single_main() -> RustArtifact {
        RustArtifact::with_id(ArtifactId::new("art-fo"), "main.rs", "fn main() {}")
            .with_specification_id(SpecificationId::new("spec-fo"))
    }

    fn helper_path() -> ArtifactPath {
        ArtifactPath::parse("src/helper.rs").unwrap()
    }

    fn main_path() -> ArtifactPath {
        ArtifactPath::parse("src/main.rs").unwrap()
    }

    #[test]
    fn create_file_valid() {
        // A
        let mut artifact = single_main();
        artifact
            .create_file(helper_path(), "pub fn h() {}")
            .expect("create");
        assert_eq!(artifact.file_count(), 2);
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn create_file_duplicate_rejected() {
        // B
        let mut artifact = single_main();
        artifact.create_file(helper_path(), "a").unwrap();
        assert!(artifact.create_file(helper_path(), "b").is_err());
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn delete_file_valid() {
        // C
        let mut artifact = single_main();
        artifact.create_file(helper_path(), "x").unwrap();
        artifact.delete_file(&helper_path()).expect("delete");
        assert_eq!(artifact.file_count(), 1);
        assert_eq!(artifact.revision(), 2);
    }

    #[test]
    fn delete_missing_rejected() {
        // D
        let mut artifact = single_main();
        assert!(artifact.delete_file(&helper_path()).is_err());
        assert_eq!(artifact.revision(), 0);
    }

    #[test]
    fn rename_file_valid() {
        // E
        let mut artifact = single_main();
        artifact.create_file(helper_path(), "v").unwrap();
        let dest = ArtifactPath::parse("src/util.rs").unwrap();
        artifact.rename_file(helper_path(), dest.clone()).unwrap();
        assert!(artifact.file(&dest).is_some());
        assert!(artifact.file(&helper_path()).is_none());
    }

    #[test]
    fn rename_missing_source() {
        // F
        let mut artifact = single_main();
        assert!(
            artifact
                .rename_file(helper_path(), ArtifactPath::parse("src/x.rs").unwrap())
                .is_err()
        );
    }

    #[test]
    fn rename_destination_collision() {
        // G
        let mut artifact = single_main();
        artifact.create_file(helper_path(), "a").unwrap();
        let other = ArtifactPath::parse("src/other.rs").unwrap();
        artifact.create_file(other.clone(), "b").unwrap();
        assert!(artifact.rename_file(helper_path(), other).is_err());
    }

    #[test]
    fn rename_primary_updates_primary() {
        // H
        let mut artifact = single_main();
        let lib = ArtifactPath::parse("src/lib.rs").unwrap();
        artifact
            .rename_file(main_path(), lib.clone())
            .expect("rename primary");
        assert_eq!(artifact.primary_path(), &lib);
        assert_eq!(artifact.source(), "fn main() {}");
    }

    #[test]
    fn delete_primary_rejected() {
        // I
        let mut artifact = single_main();
        assert!(artifact.delete_file(&main_path()).is_err());
    }

    #[test]
    fn invalid_paths_rejected() {
        // J
        assert!(ArtifactPath::parse("../etc/passwd").is_err());
        let err = structured_to_file_operation(&StructuredFileOperation::CreateFile {
            path: "../x".to_string(),
            source: "a".to_string(),
        })
        .unwrap_err();
        assert!(matches!(
            err,
            crate::harness::ModelResponseError::InvalidFileOperation(_)
        ));
    }

    #[test]
    fn identity_and_spec_preserved() {
        // K + L + M
        let mut artifact = single_main();
        let id = artifact.id().clone();
        let name = artifact.name().to_string();
        artifact.create_file(helper_path(), "h").unwrap();
        assert_eq!(artifact.id(), &id);
        assert_eq!(artifact.name(), name);
        assert_eq!(
            artifact.specification_id().map(|s| s.as_str()),
            Some("spec-fo")
        );
    }

    #[test]
    fn revision_semantics_single_ops() {
        // N
        let mut artifact = single_main();
        assert_eq!(artifact.revision(), 0);
        artifact.create_file(helper_path(), "a").unwrap();
        assert_eq!(artifact.revision(), 1);
        assert!(artifact.create_file(helper_path(), "b").is_err());
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn batch_multiple_create() {
        // O
        let mut artifact = single_main();
        apply_file_operations_to_artifact(
            &mut artifact,
            &[
                ArtifactFileOperation::CreateFile {
                    path: ArtifactPath::parse("src/a.rs").unwrap(),
                    source: "a".to_string(),
                },
                ArtifactFileOperation::CreateFile {
                    path: ArtifactPath::parse("src/b.rs").unwrap(),
                    source: "b".to_string(),
                },
            ],
        )
        .unwrap();
        assert_eq!(artifact.file_count(), 3);
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn batch_heterogeneous_ops() {
        // P
        let mut artifact = single_main();
        apply_file_operations_to_artifact(
            &mut artifact,
            &[
                ArtifactFileOperation::CreateFile {
                    path: helper_path(),
                    source: "h".to_string(),
                },
                ArtifactFileOperation::RenameFile {
                    from: helper_path(),
                    to: ArtifactPath::parse("src/util.rs").unwrap(),
                },
            ],
        )
        .unwrap();
        assert_eq!(artifact.file_count(), 2);
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn batch_atomic_on_failure() {
        // Q + R + S
        let mut artifact = single_main();
        let rev = artifact.revision();
        let snap = artifact.files_snapshot();
        let err = apply_file_operations_to_artifact(
            &mut artifact,
            &[
                ArtifactFileOperation::CreateFile {
                    path: helper_path(),
                    source: "ok".to_string(),
                },
                ArtifactFileOperation::CreateFile {
                    path: helper_path(),
                    source: "dup".to_string(),
                },
            ],
        )
        .unwrap_err();
        assert!(err.contains("ya existe"));
        assert_eq!(artifact.files_snapshot(), snap);
        assert_eq!(artifact.revision(), rev);
    }

    #[test]
    fn batch_success_single_revision_increment() {
        // T
        let mut artifact = single_main();
        apply_file_operations_to_artifact(
            &mut artifact,
            &[
                ArtifactFileOperation::CreateFile {
                    path: ArtifactPath::parse("src/a.rs").unwrap(),
                    source: "1".to_string(),
                },
                ArtifactFileOperation::CreateFile {
                    path: ArtifactPath::parse("src/b.rs").unwrap(),
                    source: "2".to_string(),
                },
            ],
        )
        .unwrap();
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn policy_accepts_valid_create() {
        // U
        let ctx = AgentContext::new("p").with_working_artifact(single_main());
        let decision = ApplyFileOperationsConstraint.check(
            &AgentAction::ApplyFileOperations {
                operations: vec![ArtifactFileOperation::CreateFile {
                    path: helper_path(),
                    source: "x".to_string(),
                }],
            },
            &ctx,
        );
        assert!(matches!(decision, ConstraintDecision::Allow));
    }

    #[test]
    fn policy_rejects_collisions_and_primary() {
        // V + W + X + Y
        let mut artifact = single_main();
        artifact.create_file(helper_path(), "x").unwrap();
        let ctx = AgentContext::new("p").with_working_artifact(artifact);
        let dup = ApplyFileOperationsConstraint.check(
            &AgentAction::ApplyFileOperations {
                operations: vec![ArtifactFileOperation::CreateFile {
                    path: helper_path(),
                    source: "y".to_string(),
                }],
            },
            &ctx,
        );
        assert!(matches!(dup, ConstraintDecision::Reject { .. }));

        let del_missing = ApplyFileOperationsConstraint.check(
            &AgentAction::ApplyFileOperations {
                operations: vec![ArtifactFileOperation::DeleteFile {
                    path: ArtifactPath::parse("src/missing.rs").unwrap(),
                }],
            },
            &ctx,
        );
        assert!(matches!(del_missing, ConstraintDecision::Reject { .. }));

        let del_primary = ApplyFileOperationsConstraint.check(
            &AgentAction::ApplyFileOperations {
                operations: vec![ArtifactFileOperation::DeleteFile { path: main_path() }],
            },
            &ctx,
        );
        assert!(matches!(del_primary, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn model_json_create_delete_rename_round_trip() {
        // Z + AA + AB + AC
        for (raw, kind) in [
            (
                r#"{"action":"apply_file_operations","operations":[{"operation":"create_file","path":"src/helper.rs","source":"pub fn h(){}"}]}"#,
                "create",
            ),
            (
                r#"{"action":"apply_file_operations","operations":[{"operation":"delete_file","path":"src/helper.rs"}]}"#,
                "delete",
            ),
            (
                r#"{"action":"apply_file_operations","operations":[{"operation":"rename_file","from":"src/a.rs","to":"src/b.rs"}]}"#,
                "rename",
            ),
        ] {
            let parsed = parse_model_response(raw).expect(kind);
            let ModelDecision::ApplyFileOperations { operations } = parsed else {
                panic!("expected apply_file_operations for {kind}");
            };
            let round = serialize_decision(&ModelDecision::ApplyFileOperations { operations });
            assert!(parse_model_response(&round).is_ok());
        }
    }

    #[test]
    fn model_path_traversal_rejected() {
        // AD
        let raw = r#"{"action":"apply_file_operations","operations":[{"operation":"create_file","path":"../x.rs","source":"a"}]}"#;
        let parsed = parse_model_response(raw).expect("parse");
        let ModelDecision::ApplyFileOperations { operations } = parsed else {
            panic!("expected ops");
        };
        assert!(structured_to_file_operation(&operations[0]).is_err());
    }

    #[test]
    fn harness_mutates_canonical_artifact() {
        // AG + AH + AI + AJ + AK
        let mut harness = Harness::new(4);
        harness.register_tool(Box::new(FileOperationsTool));
        harness.register_constraint(Box::new(ActionPolicy::default_session_policy()));
        let mut ctx = AgentContext::new("h").with_working_artifact(single_main());
        let ops = vec![ArtifactFileOperation::CreateFile {
            path: helper_path(),
            source: "pub fn h() {}".to_string(),
        }];
        let outcome = harness.execute_step(
            AgentAction::ApplyFileOperations {
                operations: ops.clone(),
            },
            &mut ctx,
        );
        assert!(outcome.tool_executed);
        assert!(outcome.tool_result.as_ref().unwrap().success);
        assert!(
            ctx.working_artifact
                .as_ref()
                .unwrap()
                .file(&helper_path())
                .is_some()
        );
        assert!(
            outcome
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-fo")
        );

        let bad = harness.execute_step(
            AgentAction::ApplyFileOperations {
                operations: vec![ArtifactFileOperation::DeleteFile {
                    path: ArtifactPath::parse("src/missing.rs").unwrap(),
                }],
            },
            &mut ctx,
        );
        assert!(!bad.permitted || !bad.tool_result.as_ref().is_some_and(|r| r.success));
        assert!(
            ctx.working_artifact
                .as_ref()
                .unwrap()
                .file(&helper_path())
                .is_some()
        );
    }

    #[test]
    fn create_sibling_materializes_and_compiles_after_mod_fix() {
        // AL + AM + AN + AO
        let _guard = COMPILE_LOCK.lock().unwrap();
        let mut artifact =
            RustArtifact::with_id(ArtifactId::new("art-int"), "main.rs", "fn main() {}");
        apply_file_operations_to_artifact(
            &mut artifact,
            &[ArtifactFileOperation::CreateFile {
                path: helper_path(),
                source: "pub fn value() -> i32 { 1 }".to_string(),
            }],
        )
        .unwrap();
        Correction::replace_file_text(main_path(), "fn main() {}", "mod helper;\nfn main() {}")
            .apply_to_artifact(&mut artifact)
            .unwrap();

        let mat = ArtifactMaterialization::from_artifact(&artifact).expect("mat");
        assert!(mat.root().join("src/helper.rs").is_file());
        let output = Command::new("cargo")
            .args(["check", "--quiet", "--manifest-path"])
            .arg(mat.root().join("Cargo.toml"))
            .output()
            .expect("cargo");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn delete_leaves_dangling_mod_compile_fails() {
        // AP
        let _guard = COMPILE_LOCK.lock().unwrap();
        let main = main_path();
        let helper = helper_path();
        let mut artifact = RustArtifact::try_from_files(
            ArtifactId::new("art-del"),
            "main.rs",
            main.clone(),
            [
                (main.clone(), "mod helper;\nfn main() {}".to_string()),
                (helper.clone(), "pub fn x() {}".to_string()),
            ],
        )
        .unwrap();
        artifact.delete_file(&helper).unwrap();
        let ctx = AgentContext::new("c").with_working_artifact(artifact);
        let result = CompileTool.execute("", &ctx);
        assert!(!result.success);
    }

    #[test]
    fn e2e_observation_driven_create_then_correct_then_compile() {
        // AQ + E2E
        struct FileOpsAgent {
            created: AtomicBool,
            corrected: AtomicBool,
        }
        impl Agent for FileOpsAgent {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                if ctx.observation_history.iter().any(|obs| {
                    matches!(
                        obs,
                        AgentObservation::CriterionEvaluated {
                            kind: CriterionKind::Compile,
                            verdict: EvaluationVerdict::Pass,
                            ..
                        }
                    )
                }) {
                    return AgentAction::Finish {
                        summary: "module wired".to_string(),
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
                if compile_failed && !self.corrected.load(Ordering::SeqCst) {
                    if self.created.load(Ordering::SeqCst) {
                        self.corrected.store(true, Ordering::SeqCst);
                        return AgentAction::ApplyCorrection {
                            corrections: vec![Correction::replace_file_text(
                                main_path(),
                                "fn main() { helper::value(); }",
                                "mod helper;\nfn main() { helper::value(); }",
                            )],
                        };
                    }
                    if !self.created.load(Ordering::SeqCst) {
                        self.created.store(true, Ordering::SeqCst);
                        return AgentAction::ApplyFileOperations {
                            operations: vec![ArtifactFileOperation::CreateFile {
                                path: helper_path(),
                                source: "pub fn value() {}".to_string(),
                            }],
                        };
                    }
                }
                AgentAction::Compile {
                    code: String::new(),
                }
            }
        }

        let spec = Specification::new("spec-e2e-fo", "modular")
            .with_requirements(vec![Requirement::new("req-c", "compila")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(FileOperationsTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(ActionPolicy::default_session_policy()));

        let ctx = AgentContext::new("e2e-fo")
            .with_working_artifact(RustArtifact::new(
                "main.rs",
                "fn main() { helper::value(); }",
            ))
            .with_evaluation_specification(spec);

        let result = AgentLoop::new(8).run(
            &harness,
            &mut FileOpsAgent {
                created: AtomicBool::new(false),
                corrected: AtomicBool::new(false),
            },
            ctx,
        );
        assert_eq!(result.status, LoopStatus::Completed);
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == APPLY_FILE_OPERATIONS)
        );
        assert!(result.tools_executed().iter().any(|t| t == COMPILE));
        let final_artifact = result.final_context.working_artifact.as_ref().unwrap();
        assert!(final_artifact.file(&helper_path()).is_some());
        assert!(final_artifact.source().contains("mod helper"));
    }

    #[test]
    fn unsupported_action_still_errors() {
        // AF
        let err = parse_model_response(r#"{"action":"launch_missiles"}"#).unwrap_err();
        assert!(matches!(
            err,
            crate::harness::ModelResponseError::UnsupportedAction(_)
        ));
    }

    #[test]
    fn validate_file_operations_matches_apply() {
        let mut artifact = single_main();
        let ops = vec![ArtifactFileOperation::CreateFile {
            path: helper_path(),
            source: "z".to_string(),
        }];
        validate_file_operations(&artifact, &ops).expect("valid");
        apply_file_operations_to_artifact(&mut artifact, &ops).unwrap();
    }
}
