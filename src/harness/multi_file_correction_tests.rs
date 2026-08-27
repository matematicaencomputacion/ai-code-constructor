//! Tests: Correction multi-file sobre RustArtifact.

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::{ActionPolicy, ApplyCorrectionConstraint};
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_path::ArtifactPath;
    use crate::harness::constraint::{Constraint, ConstraintDecision};
    use crate::harness::context::AgentContext;
    use crate::harness::correction::{Correction, apply_corrections_to_artifact};
    use crate::harness::runtime::Harness;
    use crate::harness::specification::SpecificationId;
    use crate::harness::tool::Tool;
    use crate::harness::tools::{CorrectionTool, encode_correction_input};

    fn multi_helper_artifact() -> RustArtifact {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        RustArtifact::try_from_files(
            ArtifactId::new("art-mf-corr"),
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
        .with_specification_id(SpecificationId::new("spec-mf-corr"))
    }

    #[test]
    fn legacy_correction_targets_primary() {
        // A + N
        let mut artifact = RustArtifact::new("main.rs", "Servidor NET");
        Correction::replace_session_text("NET", "HTTP")
            .apply_to_artifact(&mut artifact)
            .unwrap();
        assert_eq!(artifact.source(), "Servidor HTTP");
        assert_eq!(artifact.primary_path().as_str(), "src/main.rs");
    }

    #[test]
    fn explicit_path_correction_preserves_siblings_and_ids() {
        // B + C + D + E + F + K + ejemplo obligatorio
        let mut artifact = multi_helper_artifact();
        let id = artifact.id().clone();
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let main_before = artifact.file(&main).unwrap().to_string();
        let rev = artifact.revision();

        Correction::replace_file_text(helper.clone(), "1", "2")
            .apply_to_artifact(&mut artifact)
            .unwrap();

        assert_eq!(artifact.id(), &id);
        assert_eq!(
            artifact.specification_id().map(|s| s.as_str()),
            Some("spec-mf-corr")
        );
        assert_eq!(artifact.file(&main), Some(main_before.as_str()));
        assert_eq!(
            artifact.file(&helper),
            Some("pub fn value() -> i32 {\n    2\n}\n")
        );
        assert_eq!(artifact.revision(), rev + 1);
    }

    #[test]
    fn primary_correction_preserves_helper() {
        // L
        let mut artifact = multi_helper_artifact();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let helper_before = artifact.file(&helper).unwrap().to_string();
        Correction::replace_session_text("helper::value()", "helper::value() /*x*/")
            .apply_to_artifact(&mut artifact)
            .unwrap();
        assert_eq!(artifact.file(&helper), Some(helper_before.as_str()));
        assert!(artifact.source().contains("/*x*/"));
    }

    #[test]
    fn successive_corrections_on_distinct_files() {
        // M
        let mut artifact = multi_helper_artifact();
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        apply_corrections_to_artifact(
            &mut artifact,
            &[
                Correction::replace_file_text(helper.clone(), "1", "2"),
                Correction::replace_file_text(main.clone(), "println!", "eprintln!"),
            ],
        )
        .unwrap();
        assert!(artifact.file(&helper).unwrap().contains('2'));
        assert!(artifact.file(&main).unwrap().contains("eprintln!"));
        assert_eq!(artifact.revision(), 2);
    }

    #[test]
    fn missing_file_and_search_fail() {
        // G + H
        let mut artifact = multi_helper_artifact();
        let missing = ArtifactPath::parse("src/absent.rs").unwrap();
        assert!(
            Correction::replace_file_text(missing, "1", "2")
                .apply_to_artifact(&mut artifact)
                .is_err()
        );
        assert!(
            Correction::replace_file_text(
                ArtifactPath::parse("src/helper.rs").unwrap(),
                "zzz",
                "2",
            )
            .apply_to_artifact(&mut artifact)
            .is_err()
        );
    }

    #[test]
    fn apply_correction_constraint_rejects_bad_path_and_search() {
        // I + J
        let ctx = AgentContext::new("c").with_working_artifact(multi_helper_artifact());
        let missing = ApplyCorrectionConstraint.check(
            &AgentAction::ApplyCorrection {
                corrections: vec![Correction::replace_file_text(
                    ArtifactPath::parse("src/absent.rs").unwrap(),
                    "1",
                    "2",
                )],
            },
            &ctx,
        );
        assert!(matches!(missing, ConstraintDecision::Reject { .. }));

        let bad_search = ApplyCorrectionConstraint.check(
            &AgentAction::ApplyCorrection {
                corrections: vec![Correction::replace_file_text(
                    ArtifactPath::parse("src/helper.rs").unwrap(),
                    "zzz",
                    "2",
                )],
            },
            &ctx,
        );
        assert!(matches!(bad_search, ConstraintDecision::Reject { .. }));
    }

    #[test]
    fn correction_tool_evidence_keeps_artifact_id() {
        // O
        let ctx = AgentContext::new("ev").with_working_artifact(multi_helper_artifact());
        let input = encode_correction_input(&[Correction::replace_file_text(
            ArtifactPath::parse("src/helper.rs").unwrap(),
            "1",
            "2",
        )]);
        let result = CorrectionTool.execute(&input, &ctx);
        assert!(result.success, "{}", result.output);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "artifact_id" && e.detail == "art-mf-corr")
        );
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label == "correction_paths" && e.detail.contains("src/helper.rs"))
        );
    }

    #[test]
    fn harness_apply_correction_multi_file() {
        // P
        let mut harness = Harness::new(4);
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_constraint(Box::new(ActionPolicy::default_session_policy()));
        let mut ctx = AgentContext::new("h").with_working_artifact(multi_helper_artifact());
        let id_before = ctx.working_artifact.as_ref().unwrap().id().clone();
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let main_before = ctx
            .working_artifact
            .as_ref()
            .unwrap()
            .file(&main)
            .unwrap()
            .to_string();

        let outcome = harness.execute_step(
            AgentAction::ApplyCorrection {
                corrections: vec![Correction::replace_file_text(helper.clone(), "1", "2")],
            },
            &mut ctx,
        );
        assert!(outcome.permitted);
        assert!(outcome.tool_executed);
        assert!(outcome.tool_result.as_ref().unwrap().success);

        let artifact = ctx.working_artifact.as_ref().unwrap();
        assert_eq!(artifact.id(), &id_before);
        assert_eq!(artifact.file(&main), Some(main_before.as_str()));
        assert_eq!(
            artifact.file(&helper),
            Some("pub fn value() -> i32 {\n    2\n}\n")
        );
        assert_eq!(artifact.revision(), 1);
    }
}
