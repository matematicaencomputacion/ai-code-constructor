//! Cliente HTTP compatible con APIs estilo OpenAI (`POST /chat/completions`).
//!
//! Encapsula transporte, autenticación y extracción de texto. No ejecuta Tools.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::harness::model::{
    ModelClient, ModelError, ModelRequest, ModelResponse, append_artifact_files_to_message_parts,
    append_goal_context_to_message_parts, redact_secrets,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const ENV_BASE_URL: &str = "MODEL_BASE_URL";
const ENV_API_KEY: &str = "MODEL_API_KEY";
const ENV_MODEL_NAME: &str = "MODEL_NAME";
const ENV_TIMEOUT_MS: &str = "MODEL_TIMEOUT_MS";

/// Configuración explícita del cliente HTTP (sin secretos en logs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelClientConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
}

impl ModelClientConfig {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key,
            timeout,
        }
    }

    pub fn from_env() -> Result<Self, ModelError> {
        let base_url = std::env::var(ENV_BASE_URL).map_err(|_| {
            ModelError::Configuration(format!("variable {ENV_BASE_URL} no definida"))
        })?;
        let model = std::env::var(ENV_MODEL_NAME).map_err(|_| {
            ModelError::Configuration(format!("variable {ENV_MODEL_NAME} no definida"))
        })?;
        let api_key = std::env::var(ENV_API_KEY).ok();
        let timeout = std::env::var(ENV_TIMEOUT_MS)
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TIMEOUT);

        Ok(Self {
            base_url,
            model,
            api_key,
            timeout,
        })
    }

    pub fn require_api_key(&self) -> Result<&str, ModelError> {
        self.api_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| ModelError::Configuration(format!("variable {ENV_API_KEY} no definida")))
    }
}

/// Metadatos seguros de la última llamada HTTP (sin secretos).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelCallMetadata {
    pub provider: String,
    pub model: String,
    pub request_id: Option<String>,
    pub latency_ms: u64,
    pub http_status: Option<u16>,
    pub error_category: Option<String>,
}

/// Cliente real compatible con endpoints OpenAI-style.
pub struct OpenAICompatibleModelClient {
    config: ModelClientConfig,
    provider: String,
    last_call: Arc<Mutex<Option<ModelCallMetadata>>>,
}

impl OpenAICompatibleModelClient {
    pub fn new(config: ModelClientConfig) -> Self {
        Self {
            provider: "openai-compatible".to_string(),
            config,
            last_call: Arc::new(Mutex::new(None)),
        }
    }

    pub fn from_env() -> Result<Self, ModelError> {
        Ok(Self::new(ModelClientConfig::from_env()?))
    }

    pub fn last_call_metadata(&self) -> Option<ModelCallMetadata> {
        self.last_call.lock().ok()?.clone()
    }

    fn chat_completions_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        format!("{base}/chat/completions")
    }

    fn build_http_body(&self, request: &ModelRequest) -> String {
        let user = build_user_message(request);
        format!(
            "{{\"model\":{},\"messages\":[{{\"role\":\"system\",\"content\":{}}},{{\"role\":\"user\",\"content\":{}}}],\"response_format\":{{\"type\":\"json_object\"}}}}",
            json_string(&self.config.model),
            json_string(&request.system_prompt),
            json_string(&user),
        )
    }

    fn record_call(&self, metadata: ModelCallMetadata) {
        if let Ok(mut slot) = self.last_call.lock() {
            *slot = Some(metadata);
        }
    }
}

impl ModelClient for OpenAICompatibleModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let api_key = self.config.require_api_key()?;
        let url = self.chat_completions_url();
        let body = self.build_http_body(request);
        let started = Instant::now();

        let agent = ureq::AgentBuilder::new()
            .timeout(self.config.timeout)
            .build();

        let response = agent
            .post(&url)
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|error| map_ureq_error(error, started.elapsed(), self.config.timeout))?;

        let status = response.status();
        let request_id = response.header("x-request-id").map(str::to_string);

        let response_text = response
            .into_string()
            .map_err(|error| ModelError::Transport(redact_secrets(&error.to_string())))?;

        let latency_ms = started.elapsed().as_millis() as u64;
        self.record_call(ModelCallMetadata {
            provider: self.provider.clone(),
            model: self.config.model.clone(),
            request_id,
            latency_ms,
            http_status: Some(status),
            error_category: None,
        });

        if !(200..300).contains(&status) {
            let category = http_error_category(status);
            self.record_call(ModelCallMetadata {
                provider: self.provider.clone(),
                model: self.config.model.clone(),
                request_id: None,
                latency_ms,
                http_status: Some(status),
                error_category: Some(category.clone()),
            });
            return Err(map_http_status(status, &response_text));
        }

        let raw_text = extract_message_content(&response_text)
            .map_err(|message| ModelError::InvalidResponse(redact_secrets(&message)))?;

        Ok(ModelResponse { raw_text })
    }
}

fn build_user_message(request: &ModelRequest) -> String {
    let mut parts = vec![
        format!("goal={}", request.goal),
        format!("step={}", request.step),
        format!("user_request={}", request.user_request),
    ];
    if let Some(plan_kind) = &request.plan_kind {
        parts.push(format!("plan_kind={plan_kind}"));
    }
    if let Some(code) = &request.working_code {
        parts.push(format!("working_code_bytes={}", code.len()));
        parts.push(format!("working_code={code}"));
    }
    append_artifact_files_to_message_parts(
        &mut parts,
        request.artifact_primary_path.as_deref(),
        &request.artifact_files,
    );
    append_goal_context_to_message_parts(&mut parts, request);
    if let Some(artifact_id) = &request.artifact_id {
        parts.push(format!("artifact_id={artifact_id}"));
    }
    if let Some(language) = &request.artifact_language {
        parts.push(format!("artifact_language={language}"));
    }
    if let Some(revision) = request.artifact_revision {
        parts.push(format!("artifact_revision={revision}"));
    }
    if let Some(obs) = &request.last_observation {
        parts.push(format!("last_observation_summary={}", obs.summary));
        if let Some(verdict) = &obs.evaluation_verdict {
            parts.push(format!("evaluation_verdict={verdict}"));
        }
        if let Some(specification_id) = &obs.specification_id {
            parts.push(format!("specification_id={specification_id}"));
        }
        if let Some(criterion_id) = &obs.criterion_id {
            parts.push(format!("criterion_id={criterion_id}"));
        }
        if let Some(criterion_kind) = &obs.criterion_kind {
            parts.push(format!("criterion_kind={criterion_kind}"));
        }
        if let Some(message) = &obs.evaluation_message {
            parts.push(format!("evaluation_message={message}"));
        }
        if !obs.evidence_labels.is_empty() {
            parts.push(format!(
                "evaluation_evidence_labels={}",
                obs.evidence_labels.join(",")
            ));
        }
        if !obs.validator_errors.is_empty() {
            parts.push(format!(
                "validator_errors={}",
                obs.validator_errors.join(" | ")
            ));
        }
        if !obs.repairer_feedback.is_empty() {
            parts.push(format!(
                "repairer_feedback={}",
                obs.repairer_feedback.join(" | ")
            ));
        }
    }
    parts.join("\n")
}

fn extract_message_content(raw: &str) -> Result<String, String> {
    if let Some(content) = extract_json_string_field(raw, "content") {
        return Ok(content);
    }
    Err("no se encontró choices[0].message.content".to_string())
}

fn extract_json_string_field(raw: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\":");
    let start = raw.find(&pattern)? + pattern.len();
    let slice = raw[start..].trim_start();
    parse_json_string(slice)
}

fn parse_json_string(raw: &str) -> Option<String> {
    if !raw.starts_with('"') {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    for ch in raw.chars().skip(1) {
        if escaped {
            value.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(value),
            other => value.push(other),
        }
    }
    None
}

fn json_string(value: &str) -> String {
    let mut out = String::from('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn map_ureq_error(error: ureq::Error, elapsed: Duration, timeout: Duration) -> ModelError {
    if elapsed >= timeout.saturating_sub(Duration::from_millis(10)) {
        return ModelError::Timeout;
    }
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            map_http_status(code, &body)
        }
        ureq::Error::Transport(transport) => {
            let message = transport.to_string().to_ascii_lowercase();
            if message.contains("timeout") || message.contains("timed out") {
                ModelError::Timeout
            } else {
                ModelError::Transport(redact_secrets(&transport.to_string()))
            }
        }
    }
}

fn map_http_status(status: u16, body: &str) -> ModelError {
    let safe_body = redact_secrets(body);
    let category = http_error_category(status);
    match status {
        401 | 403 => ModelError::Authentication(category),
        429 => ModelError::RateLimited(category),
        500..=599 => ModelError::Http {
            status,
            category: safe_body,
        },
        _ => ModelError::Http {
            status,
            category: safe_body,
        },
    }
}

fn http_error_category(status: u16) -> String {
    match status {
        401 => "unauthorized".to_string(),
        403 => "forbidden".to_string(),
        429 => "rate_limited".to_string(),
        500..=599 => "server_error".to_string(),
        other => format!("http_{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::agent::Agent;
    use crate::harness::context::AgentContext;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    struct MockHttpServer {
        base_url: String,
        captured_bodies: Arc<Mutex<Vec<String>>>,
        captured_auth: Arc<Mutex<Vec<String>>>,
    }

    impl MockHttpServer {
        fn spawn(status_line: &str, response_body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
            listener.set_nonblocking(true).expect("nonblocking");
            let addr = listener.local_addr().expect("addr");
            let captured_bodies = Arc::new(Mutex::new(Vec::new()));
            let captured_auth = Arc::new(Mutex::new(Vec::new()));
            let bodies = Arc::clone(&captured_bodies);
            let auth = Arc::clone(&captured_auth);
            let status = status_line.to_string();
            let body = response_body.to_string();

            thread::spawn(move || {
                for _ in 0..500 {
                    if let Ok((mut stream, _)) = listener.accept() {
                        handle_mock_connection(&mut stream, &status, &body, &bodies, &auth);
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            });

            Self {
                base_url: format!("http://{addr}"),
                captured_bodies,
                captured_auth,
            }
        }

        fn captured_bodies(&self) -> Vec<String> {
            self.captured_bodies.lock().expect("lock").clone()
        }

        fn captured_auth(&self) -> Vec<String> {
            self.captured_auth.lock().expect("lock").clone()
        }
    }

    fn handle_mock_connection(
        stream: &mut TcpStream,
        status_line: &str,
        response_body: &str,
        bodies: &Arc<Mutex<Vec<String>>>,
        auth: &Arc<Mutex<Vec<String>>>,
    ) {
        let mut buffer = [0_u8; 8192];
        let read = stream.read(&mut buffer).unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        if let Some(header_end) = request.find("\r\n\r\n") {
            let headers = &request[..header_end];
            if let Some(auth_line) = headers
                .lines()
                .find(|line| line.starts_with("Authorization:"))
            {
                auth.lock().expect("lock").push(auth_line.to_string());
            }
            bodies
                .lock()
                .expect("lock")
                .push(request[header_end + 4..].to_string());
        }
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    fn sample_request() -> ModelRequest {
        ModelRequest {
            goal: "ai-test".to_string(),
            step: 1,
            user_request: "Crear una API REST".to_string(),
            plan_kind: Some("Api".to_string()),
            working_code: Some("fn main() {}".to_string()),
            artifact_id: Some("artifact:main.rs".to_string()),
            artifact_language: Some("Rust".to_string()),
            artifact_revision: Some(0),
            artifact_primary_path: Some("main.rs".to_string()),
            artifact_files: vec![crate::harness::model::ArtifactFileSnapshot {
                path: "main.rs".to_string(),
                source: "fn main() {}".to_string(),
            }],
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: None,
            goal_gap: None,
            recommended_action: None,
            system_prompt: crate::harness::agent_prompt::system_prompt_v1().to_string(),
        }
    }

    fn test_config(base_url: &str) -> ModelClientConfig {
        ModelClientConfig::new(
            base_url,
            "test-model",
            Some("test-api-key".to_string()),
            Duration::from_secs(2),
        )
    }

    fn success_response(content: &str) -> String {
        format!(
            r#"{{"id":"req_test","choices":[{{"message":{{"content":{}}}}}]}}"#,
            json_string(content)
        )
    }

    #[test]
    fn config_from_explicit_values() {
        let config = ModelClientConfig::new(
            "http://localhost:8080/v1",
            "gpt-test",
            Some("test-api-key".to_string()),
            Duration::from_secs(5),
        );
        assert_eq!(config.base_url, "http://localhost:8080/v1");
        assert_eq!(config.model, "gpt-test");
        assert!(config.require_api_key().is_ok());
    }

    #[test]
    fn missing_api_key_returns_configuration_error() {
        let client = OpenAICompatibleModelClient::new(ModelClientConfig::new(
            "http://localhost:8080/v1",
            "gpt-test",
            None,
            Duration::from_secs(1),
        ));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::Configuration(_)));
    }

    #[test]
    fn http_request_includes_multi_file_artifact_context() {
        let server = MockHttpServer::spawn(
            "200 OK",
            &success_response(r#"{"action":"finish","summary":"ok"}"#),
        );
        let mut request = sample_request();
        request.artifact_primary_path = Some("src/main.rs".to_string());
        request.artifact_files = vec![
            crate::harness::model::ArtifactFileSnapshot {
                path: "src/main.rs".to_string(),
                source: "fn main() {}".to_string(),
            },
            crate::harness::model::ArtifactFileSnapshot {
                path: "src/helper.rs".to_string(),
                source: "pub fn ok() {}".to_string(),
            },
        ];
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        client.complete(&request).expect("complete");

        let bodies = server.captured_bodies();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("artifact_primary_path=src/main.rs"));
        assert!(bodies[0].contains("artifact_file_count=2"));
        assert!(bodies[0].contains("artifact_file_1_path=src/helper.rs"));
        assert!(bodies[0].contains("artifact_file_1_source=pub fn ok() {}"));
    }

    #[test]
    fn http_request_includes_recommended_action_from_goal_context() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::model::{AiSessionConfig, model_request_from_context};
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-openai-rec", "compilar")
            .with_requirements(vec![Requirement::new("req", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req")]),
            ]);
        let session = AiSessionConfig::new("compilar", "Generic");
        let ctx = AgentContext::new("openai-rec")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);
        let request = model_request_from_context(&ctx, &session).expect("request");
        assert!(request.recommended_action.is_some());

        let server = MockHttpServer::spawn(
            "200 OK",
            &success_response(r#"{"action":"finish","summary":"ok"}"#),
        );
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        client.complete(&request).expect("complete");

        let bodies = server.captured_bodies();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("recommended_action_kind=InvokeTool"));
        assert!(bodies[0].contains("recommended_action_tool=compile"));
        assert!(
            bodies[0].contains("recommended_action_directive=MUST_FOLLOW_WHEN_GOAL_UNSATISFIED")
        );
    }

    #[test]
    fn http_request_is_well_formed() {
        let server = MockHttpServer::spawn(
            "200 OK",
            &success_response(r#"{"action":"finish","summary":"ok"}"#),
        );
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        client.complete(&sample_request()).expect("complete");

        let bodies = server.captured_bodies();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("\"model\":\"test-model\""));
        assert!(bodies[0].contains("\"role\":\"system\""));
        assert!(bodies[0].contains("\"role\":\"user\""));
        assert!(bodies[0].contains("Crear una API REST"));
    }

    #[test]
    fn valid_http_response_becomes_model_response() {
        let server = MockHttpServer::spawn(
            "200 OK",
            &success_response(r#"{"action":"finish","summary":"ok"}"#),
        );
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        let response = client.complete(&sample_request()).expect("response");
        assert!(response.raw_text.contains("finish"));
    }

    #[test]
    fn invalid_http_response_body_returns_invalid_response() {
        let server = MockHttpServer::spawn("200 OK", r#"{"choices":[]}"#);
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::InvalidResponse(_)));
    }

    #[test]
    fn http_401_returns_authentication_error() {
        let server = MockHttpServer::spawn("401 Unauthorized", r#"{"error":"unauthorized"}"#);
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::Authentication(_)));
        assert!(!err.to_string().contains("test-api-key"));
    }

    #[test]
    fn http_429_returns_rate_limited_error() {
        let server = MockHttpServer::spawn("429 Too Many Requests", r#"{"error":"slow down"}"#);
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::RateLimited(_)));
    }

    #[test]
    fn http_500_returns_http_error() {
        let server = MockHttpServer::spawn("500 Internal Server Error", r#"{"error":"boom"}"#);
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::Http { .. }));
    }

    #[test]
    fn api_key_never_appears_in_public_errors() {
        let server = MockHttpServer::spawn("403 Forbidden", "Bearer test-api-key leaked");
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        let err = client.complete(&sample_request()).unwrap_err();
        let rendered = err.to_string();
        assert!(!rendered.contains("test-api-key"));
    }

    #[test]
    fn authorization_header_is_not_logged_in_metadata() {
        let server = MockHttpServer::spawn(
            "200 OK",
            &success_response(r#"{"action":"finish","summary":"ok"}"#),
        );
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        client.complete(&sample_request()).expect("complete");
        let auth = server.captured_auth();
        assert!(auth.iter().any(|line| line.contains("Bearer test-api-key")));
        let metadata = client.last_call_metadata().expect("metadata");
        assert_eq!(metadata.provider, "openai-compatible");
        assert_eq!(metadata.model, "test-model");
    }

    #[test]
    fn model_client_does_not_execute_tools() {
        let server = MockHttpServer::spawn(
            "200 OK",
            &success_response(r#"{"action":"finish","summary":"ok"}"#),
        );
        let client = OpenAICompatibleModelClient::new(test_config(&server.base_url));
        let _ = client.complete(&sample_request());
        let bodies = server.captured_bodies();
        assert!(!bodies[0].contains("cargo build"));
        assert!(!bodies[0].contains("cargo run"));
        assert!(!bodies[0].contains("cargo check"));
    }

    #[test]
    fn redact_secrets_hides_authorization() {
        let redacted = redact_secrets("Authorization: Bearer secret-key");
        assert!(!redacted.contains("secret-key"));
        assert!(redacted.contains("REDACTED"));
    }

    #[test]
    fn timeout_against_unresponsive_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        // No accept → client should timeout
        let client = OpenAICompatibleModelClient::new(ModelClientConfig::new(
            format!("http://{addr}"),
            "test-model",
            Some("test-api-key".to_string()),
            Duration::from_millis(200),
        ));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(
            err,
            ModelError::Timeout | ModelError::Transport(_)
        ));
    }

    #[test]
    fn ai_agent_works_with_openai_compatible_client_via_trait() {
        let server = MockHttpServer::spawn(
            "200 OK",
            &success_response(
                r#"{"action":"repair_diagnostic","errors":["El código no contiene la implementación esperada de API REST"]}"#,
            ),
        );
        let client: Box<dyn ModelClient> = Box::new(OpenAICompatibleModelClient::new(test_config(
            &server.base_url,
        )));
        let session = crate::harness::model::AiSessionConfig::new(
            "Crear una API REST".to_string(),
            "Api".to_string(),
        );
        let mut agent = crate::harness::AiAgent::new(client, session);
        let mut ctx = AgentContext::new("openai-client");
        ctx.step = 1;
        let action = agent.propose(&ctx);
        assert!(matches!(
            action,
            crate::harness::AgentAction::RepairDiagnostic { .. }
        ));
    }

    /// Prueba manual opcional (NO CI): requiere MODEL_BASE_URL, MODEL_API_KEY, MODEL_NAME.
    #[test]
    #[ignore = "requiere endpoint real y API key configurada por el operador"]
    fn manual_live_openai_compatible_client() {
        let client = OpenAICompatibleModelClient::from_env().expect("config");
        let request = sample_request();
        let response = client.complete(&request).expect("live response");
        assert!(!response.raw_text.is_empty());
    }
}
