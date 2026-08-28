//! Tests de aislamiento: Quality Tools evalúan el RustArtifact, no el workspace del repo.

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::agent::Agent;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_materialization::ArtifactMaterialization;
    use crate::harness::autonomous_construction::{
        AutonomousConstructionConfig, AutonomousConstructionSession, ConstructionStatus,
    };
    use crate::harness::constraint::Constraint;
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::evaluation_engine::EvaluationEngine;
    use crate::harness::observation::AgentObservation;
    use crate::harness::runtime::Harness;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tool::Tool;
    use crate::harness::tools::{
        CHECK_FORMAT, ClippyTool, CompileTool, CorrectionTool, FmtTool, RUN_CLIPPY, RUN_TESTS,
        RepairDiagnosticTool, TestTool, ValidationTool,
    };
    use std::fs;

    fn artifact(id: &str, source: &str) -> RustArtifact {
        RustArtifact::with_id(ArtifactId::new(id), "main.rs", source)
    }

    fn ctx_with(id: &str, source: &str) -> AgentContext {
        AgentContext::new("quality-scope").with_working_artifact(artifact(id, source))
    }

    fn passing_tests_source() -> &'static str {
        r#"
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn isolation_pass() {
        assert_eq!(1 + 1, 2);
    }
}
"#
    }

    fn failing_tests_source() -> &'static str {
        r#"
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn isolation_must_fail() {
        assert_eq!(1 + 1, 3, "artifact isolation failure marker");
    }
}
"#
    }

    fn clippy_clean_source() -> &'static str {
        "fn main() {}\n"
    }

    fn clippy_dirty_source() -> &'static str {
        // unused_variables → warning; clippy -D warnings → FAIL
        "fn main() {\n    let unused_for_isolation = 1;\n}\n"
    }

    fn fmt_clean_source() -> &'static str {
        "fn main() {}\n"
    }

    fn fmt_dirty_source() -> &'static str {
        "fn main(){let x=1;println!(\"{}\",x);}\n"
    }

    fn evidence_artifact_id(result: &crate::harness::tool::ToolResult) -> Option<&str> {
        result
            .evidence
            .iter()
            .find(|item| item.label == "artifact_id")
            .map(|item| item.detail.as_str())
    }

    #[test]
    fn missing_artifact_does_not_use_workspace() {
        // A — sin Artifact: fail controlado, sin cargo sobre el repo
        for (tool_name, result) in [
            (
                "run_tests",
                TestTool.execute("", &AgentContext::new("no-art")),
            ),
            (
                "run_clippy",
                ClippyTool.execute("", &AgentContext::new("no-art")),
            ),
            (
                "check_format",
                FmtTool.execute("", &AgentContext::new("no-art")),
            ),
        ] {
            assert!(
                !result.success,
                "{tool_name} no debe ejecutarse sin Artifact"
            );
            assert!(
                result
                    .evidence
                    .iter()
                    .any(|e| e.label == "missing_artifact"),
                "{tool_name}: {:?}",
                result.evidence
            );
            assert!(
                !result.evidence.iter().any(|e| e.label == "exit_status"),
                "{tool_name} no debe haber corrido cargo"
            );
        }
    }

    #[test]
    fn test_tool_uses_materialized_artifact_not_workspace() {
        // B + F + G — Artifact con assert fallido → FAIL aunque el repo esté sano
        let result = TestTool.execute("", &ctx_with("art-test-fail", failing_tests_source()));
        assert!(
            !result.success,
            "debe fallar el Artifact: {}",
            result.output
        );
        assert!(result.output.contains("isolation_must_fail") || !result.success);
        assert_eq!(evidence_artifact_id(&result), Some("art-test-fail"));

        let pass = TestTool.execute("", &ctx_with("art-test-pass", passing_tests_source()));
        assert!(pass.success, "Artifact válido debe pasar: {}", pass.output);
        assert_eq!(evidence_artifact_id(&pass), Some("art-test-pass"));
    }

    #[test]
    fn clippy_tool_uses_materialized_artifact_not_workspace() {
        // C + G
        let dirty = ClippyTool.execute("", &ctx_with("art-clippy-bad", clippy_dirty_source()));
        assert!(
            !dirty.success,
            "clippy debe fallar sobre Artifact sucio: {}",
            dirty.output
        );
        assert_eq!(evidence_artifact_id(&dirty), Some("art-clippy-bad"));

        let clean = ClippyTool.execute("", &ctx_with("art-clippy-ok", clippy_clean_source()));
        assert!(
            clean.success,
            "clippy debe pasar sobre Artifact limpio: {}",
            clean.output
        );
        assert_eq!(evidence_artifact_id(&clean), Some("art-clippy-ok"));
    }

    #[test]
    fn fmt_tool_uses_materialized_artifact_not_workspace() {
        // D + G
        let dirty = FmtTool.execute("", &ctx_with("art-fmt-bad", fmt_dirty_source()));
        assert!(
            !dirty.success,
            "fmt --check debe fallar sobre Artifact mal formateado: {}",
            dirty.output
        );
        assert_eq!(evidence_artifact_id(&dirty), Some("art-fmt-bad"));

        let clean = FmtTool.execute("", &ctx_with("art-fmt-ok", fmt_clean_source()));
        assert!(
            clean.success,
            "fmt --check debe pasar sobre Artifact formateado: {}",
            clean.output
        );
        assert_eq!(evidence_artifact_id(&clean), Some("art-fmt-ok"));
    }

    #[test]
    fn evidence_contains_correct_artifact_id_field() {
        // E
        let result = TestTool.execute("", &ctx_with("art-id-field", passing_tests_source()));
        let entry = result
            .evidence
            .iter()
            .find(|e| e.label == "artifact_id")
            .expect("artifact_id label");
        assert_eq!(entry.detail, "art-id-field");
        assert_eq!(
            entry.artifact_id.as_ref().map(|id| id.as_str()),
            Some("art-id-field")
        );
    }

    #[test]
    fn two_artifacts_produce_independent_evidence() {
        // F
        let a = TestTool.execute("", &ctx_with("art-independent-a", failing_tests_source()));
        let b = TestTool.execute("", &ctx_with("art-independent-b", passing_tests_source()));
        assert!(!a.success);
        assert!(b.success);
        assert_eq!(evidence_artifact_id(&a), Some("art-independent-a"));
        assert_eq!(evidence_artifact_id(&b), Some("art-independent-b"));
        assert_ne!(evidence_artifact_id(&a), evidence_artifact_id(&b));
    }

    #[test]
    fn workspace_repo_remains_intact_after_quality_tools() {
        // H
        let cargo_toml = fs::read_to_string("Cargo.toml").expect("Cargo.toml del repo");
        let main_rs = fs::read_to_string("src/main.rs").expect("src/main.rs del repo");
        let _ = TestTool.execute("", &ctx_with("art-ws-intact", failing_tests_source()));
        let _ = ClippyTool.execute("", &ctx_with("art-ws-intact-2", clippy_dirty_source()));
        let _ = FmtTool.execute("", &ctx_with("art-ws-intact-3", fmt_dirty_source()));
        assert_eq!(
            fs::read_to_string("Cargo.toml").unwrap(),
            cargo_toml,
            "Cargo.toml del repo no debe mutar"
        );
        assert_eq!(
            fs::read_to_string("src/main.rs").unwrap(),
            main_rs,
            "src/main.rs del repo no debe mutar"
        );
    }

    #[test]
    fn temporary_materialization_is_cleaned() {
        // I — cubierto también en artifact_materialization::tests; refuerzo aquí
        let art = artifact("art-cleanup", "fn main() {}");
        let path = {
            let mat = ArtifactMaterialization::from_artifact(&art).expect("mat");
            mat.root().to_path_buf()
        };
        assert!(!path.exists(), "temp debe limpiarse: {}", path.display());
    }

    #[test]
    fn artifact_revision_is_reflected_on_next_execution() {
        // J
        let mut art = artifact(
            "art-rev",
            r#"
fn main() {}
#[cfg(test)]
mod tests {
    #[test]
    fn v0() { assert!(true); }
}
"#,
        );
        let first = TestTool.execute(
            "",
            &AgentContext::new("rev").with_working_artifact(art.clone()),
        );
        assert!(first.success, "{}", first.output);

        art.replace_source(
            r#"
fn main() {}
#[cfg(test)]
mod tests {
    #[test]
    fn v1_must_fail() { assert!(false, "revision marker"); }
}
"#,
        );
        assert_eq!(art.revision(), 1);
        let second = TestTool.execute(
            "",
            &AgentContext::new("rev").with_working_artifact(art.clone()),
        );
        assert!(
            !second.success,
            "la revisión nueva debe fallar: {}",
            second.output
        );
        assert_eq!(evidence_artifact_id(&second), Some("art-rev"));
    }

    #[test]
    fn artifact_tool_evidence_evaluation_traceability() {
        // K
        let result = TestTool.execute("", &ctx_with("art-trace", failing_tests_source()));
        assert!(!result.success);
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &result.evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.artifact_id.as_ref().map(|id| id.as_str()) == Some("art-trace"))
        );
        assert!(
            evaluation
                .evidence_used
                .iter()
                .any(|e| e.label == "tool" && e.detail == RUN_TESTS)
        );
    }

    #[test]
    fn e2e_autonomous_construction_quality_tools_use_session_artifact() {
        // E2E mínimo: Agent propone RunTests/Clippy/Fmt sobre Artifact de sesión.
        fn quality_spec() -> Specification {
            Specification::new("spec-artifact-quality", "Crear una API REST")
                .with_requirements(vec![
                    Requirement::new("req-t", "tests"),
                    Requirement::new("req-l", "clippy"),
                    Requirement::new("req-f", "format"),
                ])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests)
                        .satisfying([crate::harness::RequirementId::new("req-t")]),
                    AcceptanceCriterion::new("ac-clippy", "clippy", CriterionKind::Clippy)
                        .satisfying([crate::harness::RequirementId::new("req-l")]),
                    AcceptanceCriterion::new("ac-fmt", "format", CriterionKind::CheckFormat)
                        .satisfying([crate::harness::RequirementId::new("req-f")]),
                ])
        }

        fn has_pass(ctx: &AgentContext, kind: CriterionKind) -> bool {
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

        struct QualityArtifactAgent;

        impl Agent for QualityArtifactAgent {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                if !has_pass(ctx, CriterionKind::RunTests) {
                    return AgentAction::RunTests {
                        filter: String::new(),
                    };
                }
                if !has_pass(ctx, CriterionKind::Clippy) {
                    return AgentAction::RunClippy;
                }
                if !has_pass(ctx, CriterionKind::CheckFormat) {
                    return AgentAction::CheckFormat;
                }
                AgentAction::Finish {
                    summary: "artifact quality criteria satisfied".to_string(),
                }
            }
        }

        // Source con test real, clippy limpio y formato rustfmt-compatible (sin newline inicial).
        let source = "\
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn session_artifact_test() {
        assert_eq!(2 * 2, 4);
    }
}
";

        let policy = ActionPolicy::default_session_policy();
        let policy_name = policy.name().to_string();
        let mut harness = Harness::new(12);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(TestTool));
        harness.register_tool(Box::new(ClippyTool));
        harness.register_tool(Box::new(FmtTool));
        harness.register_constraint(Box::new(policy));

        let mut agent = QualityArtifactAgent;
        let result = AutonomousConstructionSession::run_with_harness(
            AutonomousConstructionConfig::new(quality_spec(), 10).with_initial_source(source),
            &mut agent,
            policy_name,
            harness,
        );

        assert_eq!(result.status, ConstructionStatus::Completed);
        let tools = &result.observability.tools_executed_sequence;
        assert!(tools.iter().any(|t| t == RUN_TESTS));
        assert!(tools.iter().any(|t| t == RUN_CLIPPY));
        assert!(tools.iter().any(|t| t == CHECK_FORMAT));

        let loop_result = result.loop_result.as_ref().expect("loop");
        let test_step = loop_result
            .history
            .steps
            .iter()
            .find(|step| step.tool_name.as_deref() == Some(RUN_TESTS))
            .expect("run_tests step");
        let artifact_id = result.artifact_id.as_ref().expect("session artifact id");
        assert!(
            test_step
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == artifact_id.as_str()),
            "Evidence de RunTests debe llevar el ArtifactId de sesión"
        );
        assert!(test_step.evidence.iter().any(|e| {
            e.artifact_id.as_ref().map(|id| id.as_str()) == Some(artifact_id.as_str())
        }));
        let test_output = test_step
            .tool_result
            .as_ref()
            .map(|r| r.output.as_str())
            .unwrap_or("");
        assert!(
            !test_output.contains("planner::tests::"),
            "no debe ejecutar tests del workspace anfitrión"
        );
    }

    fn multi_file_passing_artifact(id: &str) -> RustArtifact {
        use crate::harness::artifact_path::ArtifactPath;
        RustArtifact::try_from_files(
            ArtifactId::new(id),
            "main.rs",
            ArtifactPath::parse("src/main.rs").unwrap(),
            [
                (
                    ArtifactPath::parse("src/main.rs").unwrap(),
                    "\
mod helper;

fn main() {
    let _ = helper::answer();
}

#[cfg(test)]
mod tests {
    use super::helper;

    #[test]
    fn multi_file_pass() {
        assert_eq!(helper::answer(), 42);
    }
}
"
                    .to_string(),
                ),
                (
                    ArtifactPath::parse("src/helper.rs").unwrap(),
                    "pub fn answer() -> i32 {\n    42\n}\n".to_string(),
                ),
            ],
        )
        .expect("multi-file artifact")
    }

    #[test]
    fn multi_file_test_clippy_fmt_tools() {
        // J + K + L
        let art = multi_file_passing_artifact("art-multi-quality");
        let ctx = AgentContext::new("multi").with_working_artifact(art.clone());
        let tests = TestTool.execute("", &ctx);
        assert!(tests.success, "{}", tests.output);
        assert_eq!(evidence_artifact_id(&tests), Some("art-multi-quality"));

        let clippy = ClippyTool.execute("", &ctx);
        assert!(clippy.success, "{}", clippy.output);

        let fmt = FmtTool.execute("", &ctx);
        assert!(fmt.success, "{}", fmt.output);
    }

    #[test]
    fn multi_file_invalid_artifact_fails_while_repo_stays_healthy() {
        // M + N
        use crate::harness::artifact_path::ArtifactPath;
        let cargo_toml = fs::read_to_string("Cargo.toml").unwrap();
        let main_rs = fs::read_to_string("src/main.rs").unwrap();
        let art = RustArtifact::try_from_files(
            ArtifactId::new("art-multi-broken"),
            "main.rs",
            ArtifactPath::parse("src/main.rs").unwrap(),
            [
                (
                    ArtifactPath::parse("src/main.rs").unwrap(),
                    "mod missing_mod;\nfn main() {}\n".to_string(),
                ),
                (
                    ArtifactPath::parse("src/helper.rs").unwrap(),
                    "pub fn unused() {}\n".to_string(),
                ),
            ],
        )
        .unwrap();
        let result = TestTool.execute("", &AgentContext::new("broken").with_working_artifact(art));
        assert!(!result.success, "crate inválido debe fallar");
        assert_eq!(fs::read_to_string("Cargo.toml").unwrap(), cargo_toml);
        assert_eq!(fs::read_to_string("src/main.rs").unwrap(), main_rs);
    }

    #[test]
    fn multi_file_revision_preserves_siblings() {
        // O
        use crate::harness::artifact_path::ArtifactPath;
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let mut art = multi_file_passing_artifact("art-multi-rev");
        let helper_before = art.file(&helper).unwrap().to_string();
        art.replace_source(
            "\
mod helper;
fn main() {}
#[cfg(test)]
mod tests {
    #[test]
    fn now_fails() { assert_eq!(helper::answer(), 0); }
}
",
        );
        assert_eq!(art.revision(), 1);
        assert_eq!(art.file(&helper), Some(helper_before.as_str()));
        let result = TestTool.execute("", &AgentContext::new("rev").with_working_artifact(art));
        assert!(!result.success, "{}", result.output);
    }

    #[test]
    fn multi_file_e2e_tool_evidence_evaluation() {
        // Q
        let art = multi_file_passing_artifact("art-multi-e2e");
        let result = TestTool.execute("", &AgentContext::new("e2e").with_working_artifact(art));
        assert!(result.success, "{}", result.output);
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &result.evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
        assert_eq!(evidence_artifact_id(&result), Some("art-multi-e2e"));
    }

    #[test]
    fn single_file_autonomous_construction_still_works() {
        // P — regression mínima con quality tools sobre Artifact single-file
        fn quality_spec() -> Specification {
            Specification::new("spec-sf-still", "Crear una API REST")
                .with_requirements(vec![Requirement::new("req-t", "tests")])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests)
                        .satisfying([crate::harness::RequirementId::new("req-t")]),
                ])
        }

        struct FinishAfterTests;
        impl Agent for FinishAfterTests {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                let passed = ctx.observation_history.iter().any(|obs| {
                    matches!(
                        obs,
                        AgentObservation::CriterionEvaluated {
                            kind: CriterionKind::RunTests,
                            verdict: EvaluationVerdict::Pass,
                            ..
                        }
                    )
                });
                if passed {
                    AgentAction::Finish {
                        summary: "ok".to_string(),
                    }
                } else {
                    AgentAction::RunTests {
                        filter: String::new(),
                    }
                }
            }
        }

        let source = "\
fn main() {}
#[cfg(test)]
mod tests {
    #[test]
    fn ok() { assert_eq!(1, 1); }
}
";
        let policy = ActionPolicy::default_session_policy();
        let policy_name = policy.name().to_string();
        let mut harness = Harness::new(8);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(TestTool));
        harness.register_tool(Box::new(ClippyTool));
        harness.register_tool(Box::new(FmtTool));
        harness.register_constraint(Box::new(policy));

        let result = AutonomousConstructionSession::run_with_harness(
            AutonomousConstructionConfig::new(quality_spec(), 6).with_initial_source(source),
            &mut FinishAfterTests,
            policy_name,
            harness,
        );
        assert_eq!(result.status, ConstructionStatus::Completed);
        assert_eq!(result.final_artifact.as_ref().unwrap().file_count(), 1);
    }

    #[test]
    fn compile_tool_requires_working_artifact() {
        // A
        let result = CompileTool.execute("fn main() {}", &AgentContext::new("no-art"));
        assert!(!result.success);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "missing_artifact")
        );
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "compile_status" && e.detail == "error")
        );
    }

    #[test]
    fn compile_tool_single_and_multi_file_pass() {
        // B + C + D
        let single = CompileTool.execute(
            "",
            &AgentContext::new("sf").with_working_code("fn main() {}\n"),
        );
        assert!(single.success, "{}", single.output);

        let multi = multi_file_passing_artifact("art-compile-multi");
        let result = CompileTool.execute("", &AgentContext::new("mf").with_working_artifact(multi));
        assert!(
            result.success,
            "CompileTool debe ver helper.rs materializado: {}",
            result.output
        );
        assert_eq!(evidence_artifact_id(&result), Some("art-compile-multi"));
    }

    #[test]
    fn compile_tool_multi_file_invalid_fails_host_intact() {
        // E + I
        use crate::harness::artifact_path::ArtifactPath;
        let cargo_toml = fs::read_to_string("Cargo.toml").unwrap();
        let main_rs = fs::read_to_string("src/main.rs").unwrap();
        let art = RustArtifact::try_from_files(
            ArtifactId::new("art-compile-broken"),
            "main.rs",
            ArtifactPath::parse("src/main.rs").unwrap(),
            [
                (
                    ArtifactPath::parse("src/main.rs").unwrap(),
                    "mod missing_sibling;\nfn main() {}\n".to_string(),
                ),
                (
                    ArtifactPath::parse("src/helper.rs").unwrap(),
                    "pub fn x() {}\n".to_string(),
                ),
            ],
        )
        .unwrap();
        let result =
            CompileTool.execute("", &AgentContext::new("broken").with_working_artifact(art));
        assert!(!result.success, "{}", result.output);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "compile_status" && e.detail == "error")
        );
        assert_eq!(fs::read_to_string("Cargo.toml").unwrap(), cargo_toml);
        assert_eq!(fs::read_to_string("src/main.rs").unwrap(), main_rs);
    }

    #[test]
    fn compile_tool_independent_artifacts_and_revision() {
        // F + G + H
        let a = artifact("art-c-a", "fn main() { /*a*/ }\n");
        let mut b = artifact("art-c-b", "fn main() { /*b*/ }\n");
        let ra = CompileTool.execute("", &AgentContext::new("a").with_working_artifact(a));
        let rb = CompileTool.execute("", &AgentContext::new("b").with_working_artifact(b.clone()));
        assert!(ra.success && rb.success);
        assert_eq!(evidence_artifact_id(&ra), Some("art-c-a"));
        assert_eq!(evidence_artifact_id(&rb), Some("art-c-b"));

        b.replace_source("fn main() { let x: = 1; }\n");
        assert_eq!(b.revision(), 1);
        let r_bad = CompileTool.execute("", &AgentContext::new("b2").with_working_artifact(b));
        assert!(
            !r_bad.success,
            "debe compilar la revisión actual: {}",
            r_bad.output
        );
        assert_eq!(evidence_artifact_id(&r_bad), Some("art-c-b"));
    }

    #[test]
    fn compile_tool_temp_cleaned_and_evaluation_pass_fail() {
        // J + K + L
        let art = artifact("art-c-eval", "fn main() {}\n");
        let path = {
            let mat = ArtifactMaterialization::from_artifact(&art).unwrap();
            mat.root().to_path_buf()
        };
        assert!(!path.exists());

        let pass = CompileTool.execute("", &AgentContext::new("pass").with_working_artifact(art));
        let engine = EvaluationEngine::new();
        let criterion = AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile);
        assert_eq!(
            engine
                .evaluate_criterion(&criterion, &pass.evidence)
                .verdict,
            EvaluationVerdict::Pass
        );

        let fail = CompileTool.execute(
            "",
            &AgentContext::new("fail").with_working_code("fn main() {"),
        );
        assert_eq!(
            engine
                .evaluate_criterion(&criterion, &fail.evidence)
                .verdict,
            EvaluationVerdict::Fail
        );
    }

    #[test]
    fn compile_tool_harness_multi_file_integration() {
        // N
        use crate::harness::artifact_path::ArtifactPath;
        let art = multi_file_passing_artifact("art-compile-harness");
        let mut harness = Harness::new(4);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(ActionPolicy::default_session_policy()));
        let mut ctx = AgentContext::new("harness-mf").with_working_artifact(art);
        let outcome = harness.execute_step(
            AgentAction::Compile {
                code: String::new(),
            },
            &mut ctx,
        );
        assert!(outcome.permitted);
        assert!(outcome.tool_executed);
        let tool = outcome.tool_result.as_ref().expect("tool result");
        assert!(tool.success, "{}", tool.output);
        assert!(
            tool.evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-compile-harness")
        );
        // primary untouched siblings
        assert!(
            ctx.working_artifact
                .as_ref()
                .unwrap()
                .file(&ArtifactPath::parse("src/helper.rs").unwrap())
                .is_some()
        );
    }
}
