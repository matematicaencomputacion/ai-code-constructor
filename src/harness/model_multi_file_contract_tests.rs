//! Tests: contrato multi-file en ModelRequest y serialización al proveedor.

#[cfg(test)]
mod tests {
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::artifact_path::ArtifactPath;
    use crate::harness::context::AgentContext;
    use crate::harness::model::{
        AiSessionConfig, append_artifact_files_to_message_parts, model_request_from_context,
    };

    fn two_file_artifact() -> RustArtifact {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let lib = ArtifactPath::parse("src/lib.rs").unwrap();
        RustArtifact::try_from_files(
            ArtifactId::new("art-contract"),
            "main.rs",
            main.clone(),
            [
                (main, "mod lib;\nfn main() {}\n".to_string()),
                (lib, "pub fn answer() -> i32 { 42 }\n".to_string()),
            ],
        )
        .unwrap()
    }

    #[test]
    fn model_request_exposes_full_artifact_tree() {
        let session = AiSessionConfig {
            user_request: "Validar proyecto".to_string(),
            plan_kind: "Api".to_string(),
        };
        let ctx = AgentContext::new("ai").with_working_artifact(two_file_artifact());
        let request = model_request_from_context(&ctx, &session).expect("request");

        assert_eq!(request.artifact_files.len(), 2);
        assert!(
            request
                .artifact_files
                .iter()
                .any(|file| file.path == "src/lib.rs" && file.source.contains("42"))
        );
        assert_eq!(
            request.working_code.as_deref(),
            Some("mod lib;\nfn main() {}\n")
        );
    }

    #[test]
    fn serialized_user_message_lists_every_artifact_file() {
        let session = AiSessionConfig {
            user_request: "Validar proyecto".to_string(),
            plan_kind: "Api".to_string(),
        };
        let ctx = AgentContext::new("ai").with_working_artifact(two_file_artifact());
        let request = model_request_from_context(&ctx, &session).expect("request");

        let mut parts = Vec::new();
        append_artifact_files_to_message_parts(
            &mut parts,
            request.artifact_primary_path.as_deref(),
            &request.artifact_files,
        );
        let message = parts.join("\n");
        assert!(message.contains("artifact_file_count=2"));
        assert!(message.contains("artifact_file_1_source=pub fn answer() -> i32 { 42 }\n"));
    }
}
