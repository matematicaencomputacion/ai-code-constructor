//! Tests: AiAgent propone Quality Actions vía ModelDecision (misma cadena de decisión).

#[cfg(test)]
mod tests {
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::ai_agent::AiAgent;
    use crate::harness::artifact::{ArtifactId, RustArtifact};
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::model::{
        AiSessionConfig, ModelClient, ModelDecision, ModelError, ModelRequest, ModelResponse,
        ModelResponseError, serialize_decision,
    };
    use crate::harness::observation::AgentObservation;
    use crate::harness::openai_compatible_client::{
        ModelClientConfig, OpenAICompatibleModelClient,
    };
    use crate::harness::runtime::Harness;
    use crate::harness::specification::{
        AcceptanceCriterion, Requirement, Specification, SpecificationId,
    };
    use crate::harness::tools::{
        CHECK_FORMAT, ClippyTool, CompileTool, CorrectionTool, FmtTool, RUN_CLIPPY, RUN_TESTS,
        RepairDiagnosticTool, TestTool, ValidationTool,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn session() -> AiSessionConfig {
        AiSessionConfig {
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        }
    }

    fn artifact_source_passing_tests() -> String {
        "\
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn isolation_pass() {
        assert_eq!(1 + 1, 2);
    }
}
"
        .to_string()
    }

    fn artifact_source_failing_tests() -> String {
        "\
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn isolation_must_fail() {
        assert!(false, \"quality fail marker\");
    }
}
"
        .to_string()
    }

    /// ModelClient observation-driven para Quality Actions (sin contadores de iteración).
    struct QualityObservationClient {
        last_decisions: Arc<Mutex<Vec<String>>>,
    }

    impl QualityObservationClient {
        fn new() -> Self {
            Self {
                last_decisions: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn record(&self, label: &str) {
            self.last_decisions
                .lock()
                .expect("lock")
                .push(label.to_string());
        }
    }

    impl ModelClient for QualityObservationClient {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            let decision = match &request.last_observation {
                None => {
                    self.record("run_tests");
                    ModelDecision::RunTests {
                        filter: String::new(),
                    }
                }
                Some(obs) if obs.kind == "action_rejected" => {
                    // Tras rechazo (p. ej. Finish prematuro), seguir con quality check.
                    self.record("run_clippy_after_reject");
                    ModelDecision::RunClippy
                }
                Some(obs)
                    if obs.kind == "criterion_evaluated"
                        && obs.criterion_kind.as_deref() == Some("RunTests")
                        && obs.evaluation_verdict.as_deref() == Some("Fail") =>
                {
                    self.record("run_tests_retry_after_fail");
                    ModelDecision::RunTests {
                        filter: String::new(),
                    }
                }
                Some(obs)
                    if obs.kind == "criterion_evaluated"
                        && obs.criterion_kind.as_deref() == Some("RunTests")
                        && obs.evaluation_verdict.as_deref() == Some("Pass") =>
                {
                    self.record("run_clippy_after_tests_pass");
                    ModelDecision::RunClippy
                }
                Some(obs)
                    if obs.kind == "criterion_evaluated"
                        && obs.criterion_kind.as_deref() == Some("Clippy")
                        && obs.evaluation_verdict.as_deref() == Some("Pass") =>
                {
                    self.record("check_format_after_clippy_pass");
                    ModelDecision::CheckFormat
                }
                Some(obs)
                    if obs.kind == "criterion_evaluated"
                        && obs.criterion_kind.as_deref() == Some("CheckFormat")
                        && obs.evaluation_verdict.as_deref() == Some("Pass") =>
                {
                    self.record("finish_after_format_pass");
                    ModelDecision::Finish {
                        summary: "quality criteria satisfied via observations".to_string(),
                    }
                }
                Some(obs)
                    if obs.kind == "criterion_evaluated"
                        && obs.evaluation_verdict.as_deref() == Some("Pass") =>
                {
                    self.record("finish_after_pass");
                    ModelDecision::Finish {
                        summary: "criterion pass".to_string(),
                    }
                }
                _ => {
                    self.record("finish_fallback");
                    ModelDecision::Finish {
                        summary: "stop".to_string(),
                    }
                }
            };
            Ok(ModelResponse {
                raw_text: serialize_decision(&decision),
            })
        }
    }

    #[test]
    fn model_decision_run_tests_maps_to_agent_action() {
        // A
        let client = ScriptClient::single(serialize_decision(&ModelDecision::RunTests {
            filter: String::new(),
        }));
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("map");
        ctx.step = 1;
        assert!(matches!(
            agent.propose(&ctx),
            AgentAction::RunTests { filter } if filter.is_empty()
        ));
    }

    #[test]
    fn model_decision_run_tests_preserves_filter() {
        // B
        let client = ScriptClient::single(serialize_decision(&ModelDecision::RunTests {
            filter: "mod::case".to_string(),
        }));
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("map");
        ctx.step = 1;
        match agent.propose(&ctx) {
            AgentAction::RunTests { filter } => assert_eq!(filter, "mod::case"),
            other => panic!("expected RunTests, got {other:?}"),
        }
    }

    #[test]
    fn model_decision_run_clippy_maps_to_agent_action() {
        // C
        let client = ScriptClient::single(serialize_decision(&ModelDecision::RunClippy));
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("map");
        ctx.step = 1;
        assert!(matches!(agent.propose(&ctx), AgentAction::RunClippy));
    }

    #[test]
    fn model_decision_check_format_maps_to_agent_action() {
        // D
        let client = ScriptClient::single(serialize_decision(&ModelDecision::CheckFormat));
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("map");
        ctx.step = 1;
        assert!(matches!(agent.propose(&ctx), AgentAction::CheckFormat));
    }

    #[test]
    fn existing_decisions_still_map() {
        // E
        let client = ScriptClient::single(serialize_decision(&ModelDecision::Compile {
            code: "fn main() {}".to_string(),
        }));
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("map");
        ctx.step = 1;
        assert!(matches!(agent.propose(&ctx), AgentAction::Compile { .. }));
    }

    #[test]
    fn unknown_decision_uses_existing_finish_error_fallback() {
        // F
        let client = ScriptClient::single(r#"{"action":"teleport"}"#.to_string());
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("map");
        ctx.step = 1;
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::Finish { summary } if summary.contains("ai error")));
        assert!(matches!(
            agent.last_response_error,
            Some(ModelResponseError::UnsupportedAction(_))
        ));
    }

    #[test]
    fn criterion_evaluated_run_tests_fail_yields_next_valid_action() {
        // G
        let client = QualityObservationClient::new();
        let decisions = Arc::clone(&client.last_decisions);
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("obs").with_working_artifact(RustArtifact::with_id(
            ArtifactId::new("art-g"),
            "main.rs",
            artifact_source_failing_tests(),
        ));
        ctx.push_observation(AgentObservation::CriterionEvaluated {
            specification_id: SpecificationId::new("spec-g"),
            criterion_id: crate::harness::specification::AcceptanceCriterionId::new("ac-tests"),
            kind: CriterionKind::RunTests,
            verdict: EvaluationVerdict::Fail,
            message: "tests fallidos".to_string(),
            evidence: Vec::new(),
        });
        ctx.step = 1;
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::RunTests { .. }));
        assert!(
            decisions
                .lock()
                .unwrap()
                .iter()
                .any(|d| d == "run_tests_retry_after_fail")
        );
    }

    #[test]
    fn criterion_evaluated_pass_changes_decision() {
        // H
        let client = QualityObservationClient::new();
        let decisions = Arc::clone(&client.last_decisions);
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("obs").with_working_code("fn main() {}\n");
        ctx.push_observation(AgentObservation::CriterionEvaluated {
            specification_id: SpecificationId::new("spec-h"),
            criterion_id: crate::harness::specification::AcceptanceCriterionId::new("ac-tests"),
            kind: CriterionKind::RunTests,
            verdict: EvaluationVerdict::Pass,
            message: "tests ok".to_string(),
            evidence: Vec::new(),
        });
        ctx.step = 1;
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::RunClippy));
        assert!(
            decisions
                .lock()
                .unwrap()
                .iter()
                .any(|d| d == "run_clippy_after_tests_pass")
        );
    }

    #[test]
    fn action_rejected_returns_to_ai_agent_for_another_decision() {
        // K
        let client = QualityObservationClient::new();
        let decisions = Arc::clone(&client.last_decisions);
        let mut agent = AiAgent::new(Box::new(client), session());
        let mut ctx = AgentContext::new("reject");
        ctx.push_observation(AgentObservation::ActionRejected {
            action: AgentAction::Finish {
                summary: "premature".to_string(),
            },
            reason: "Finish bloqueado: evidencia insuficiente".to_string(),
            constraint: "finish".to_string(),
        });
        ctx.step = 1;
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::RunClippy));
        assert!(
            decisions
                .lock()
                .unwrap()
                .iter()
                .any(|d| d == "run_clippy_after_reject")
        );
    }

    #[test]
    fn openai_compatible_client_returns_quality_action_content_to_ai_agent() {
        // J — la capa HTTP entrega el JSON; AiAgent parsea con el mismo contrato.
        let server = MockQualityHttpServer::spawn(
            "200 OK",
            &success_json(r#"{"action":"run_tests","filter":"unit"}"#),
        );
        let client: Box<dyn ModelClient> =
            Box::new(OpenAICompatibleModelClient::new(ModelClientConfig::new(
                server.base_url.clone(),
                "test-model",
                Some("test-api-key".to_string()),
                Duration::from_secs(2),
            )));
        let mut agent = AiAgent::new(client, session());
        let mut ctx = AgentContext::new("openai-quality");
        ctx.step = 1;
        match agent.propose(&ctx) {
            AgentAction::RunTests { filter } => assert_eq!(filter, "unit"),
            other => panic!("expected RunTests from OpenAI-compatible payload, got {other:?}"),
        }
    }

    #[test]
    fn e2e_ai_agent_quality_actions_observation_causality() {
        // L
        let spec = Specification::new("spec-ai-quality", "Crear una API REST")
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
            ]);

        let policy = ActionPolicy::default_session_policy();
        let mut harness = Harness::new(16);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(TestTool));
        harness.register_tool(Box::new(ClippyTool));
        harness.register_tool(Box::new(FmtTool));
        harness.register_constraint(Box::new(policy));

        let client = QualityObservationClient::new();
        let decisions = Arc::clone(&client.last_decisions);
        let mut agent = AiAgent::new(Box::new(client), session());
        let ctx = AgentContext::new("e2e-quality")
            .with_working_artifact(RustArtifact::with_id(
                ArtifactId::new("art-ai-quality"),
                "main.rs",
                artifact_source_passing_tests(),
            ))
            .with_evaluation_specification(spec);

        let result = AgentLoop::new(12).run(&harness, &mut agent, ctx);
        assert_eq!(result.status, LoopStatus::Completed);
        let tools = result.tools_executed();
        assert!(tools.iter().any(|t| t == RUN_TESTS));
        assert!(tools.iter().any(|t| t == RUN_CLIPPY));
        assert!(tools.iter().any(|t| t == CHECK_FORMAT));

        assert!(result.history.observations.iter().any(|obs| {
            matches!(
                obs,
                AgentObservation::CriterionEvaluated {
                    kind: CriterionKind::RunTests,
                    verdict: EvaluationVerdict::Pass,
                    ..
                }
            )
        }));
        let recorded = decisions.lock().unwrap().clone();
        assert!(recorded.iter().any(|d| d == "run_tests"));
        assert!(recorded.iter().any(|d| d == "run_clippy_after_tests_pass"));
        assert!(
            recorded
                .iter()
                .any(|d| d == "check_format_after_clippy_pass")
        );
        assert!(recorded.iter().any(|d| d == "finish_after_format_pass"));
        // Causalidad: clippy se decidió después de tests pass, no por índice fijo.
        let tests_idx = recorded
            .iter()
            .position(|d| d == "run_tests")
            .expect("run_tests");
        let clippy_idx = recorded
            .iter()
            .position(|d| d == "run_clippy_after_tests_pass")
            .expect("clippy after pass");
        assert!(tests_idx < clippy_idx);
    }

    struct ScriptClient {
        raw: String,
    }

    impl ScriptClient {
        fn single(raw: String) -> Self {
            Self { raw }
        }
    }

    impl ModelClient for ScriptClient {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse {
                raw_text: self.raw.clone(),
            })
        }
    }

    // --- HTTP mock mínimo (mismo patrón que openai_compatible_client tests) ---

    fn success_json(content: &str) -> String {
        format!(
            r#"{{"choices":[{{"message":{{"content":{}}}}}]}}"#,
            serde_ish_string(content)
        )
    }

    fn serde_ish_string(value: &str) -> String {
        let mut out = String::from('"');
        for ch in value.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                _ => out.push(ch),
            }
        }
        out.push('"');
        out
    }

    struct MockQualityHttpServer {
        base_url: String,
        _join: Option<std::thread::JoinHandle<()>>,
    }

    impl MockQualityHttpServer {
        fn spawn(status_line: &str, body: &str) -> Self {
            use std::io::{Read, Write};
            use std::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let addr = listener.local_addr().expect("addr");
            let status = status_line.to_string();
            let body = body.to_string();
            let join = std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf);
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self {
                base_url: format!("http://{addr}"),
                _join: Some(join),
            }
        }
    }

    impl Drop for MockQualityHttpServer {
        fn drop(&mut self) {
            if let Some(handle) = self._join.take() {
                let _ = handle.join();
            }
        }
    }
}
