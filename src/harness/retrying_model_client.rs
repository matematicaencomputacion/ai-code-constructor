//! Wrapper de [`ModelClient`] con retry simple para errores transitorios.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::harness::model::{ModelClient, ModelError, ModelRequest, ModelResponse};

const DEFAULT_MAX_RETRIES: u32 = 2;
const DEFAULT_BACKOFF: Duration = Duration::from_millis(50);

/// Configuración de retry para el cliente HTTP/modelo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            backoff: DEFAULT_BACKOFF,
        }
    }
}

/// Envuelve un [`ModelClient`] y reintenta errores transitorios (429, timeout, 5xx).
pub struct RetryingModelClient {
    inner: Box<dyn ModelClient>,
    config: RetryConfig,
    last_retry_count: Arc<Mutex<u32>>,
}

impl RetryingModelClient {
    pub fn new(inner: Box<dyn ModelClient>) -> Self {
        Self::with_config(inner, RetryConfig::default())
    }

    pub fn with_config(inner: Box<dyn ModelClient>, config: RetryConfig) -> Self {
        Self {
            inner,
            config,
            last_retry_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn last_retry_count(&self) -> u32 {
        self.last_retry_count.lock().map(|v| *v).unwrap_or(0)
    }
}

impl ModelClient for RetryingModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let mut attempt = 0_u32;
        loop {
            match self.inner.complete(request) {
                Ok(response) => {
                    if let Ok(mut slot) = self.last_retry_count.lock() {
                        *slot = attempt;
                    }
                    return Ok(response);
                }
                Err(error) if attempt < self.config.max_retries && error.is_retryable() => {
                    attempt += 1;
                    thread::sleep(self.config.backoff);
                }
                Err(error) => {
                    if let Ok(mut slot) = self.last_retry_count.lock() {
                        *slot = attempt;
                    }
                    return Err(error);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::agent_prompt::system_prompt_v1;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct FlakyModelClient {
        fail_times: u32,
        calls: AtomicU32,
        error: ModelError,
        success_body: String,
    }

    impl FlakyModelClient {
        fn new(fail_times: u32, error: ModelError, success_body: impl Into<String>) -> Self {
            Self {
                fail_times,
                calls: AtomicU32::new(0),
                error,
                success_body: success_body.into(),
            }
        }
    }

    impl ModelClient for FlakyModelClient {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.fail_times {
                Err(self.error.clone())
            } else {
                Ok(ModelResponse {
                    raw_text: self.success_body.clone(),
                })
            }
        }
    }

    fn sample_request() -> ModelRequest {
        ModelRequest {
            goal: "retry".to_string(),
            step: 1,
            user_request: "Crear una API REST".to_string(),
            plan_kind: Some("Api".to_string()),
            working_code: Some("fn main() {}".to_string()),
            artifact_id: Some("artifact:main.rs".to_string()),
            artifact_language: Some("Rust".to_string()),
            artifact_revision: Some(0),
            system_prompt: system_prompt_v1().to_string(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
        }
    }

    #[test]
    fn retries_rate_limited_then_succeeds() {
        let inner = FlakyModelClient::new(
            2,
            ModelError::RateLimited("slow".to_string()),
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::with_config(
            Box::new(inner),
            RetryConfig {
                max_retries: 2,
                backoff: Duration::from_millis(1),
            },
        );
        client.complete(&sample_request()).expect("success");
        assert_eq!(client.last_retry_count(), 2);
    }

    #[test]
    fn retries_timeout_then_succeeds() {
        let inner = FlakyModelClient::new(
            1,
            ModelError::Timeout,
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::with_config(
            Box::new(inner),
            RetryConfig {
                max_retries: 2,
                backoff: Duration::from_millis(1),
            },
        );
        client.complete(&sample_request()).expect("success");
        assert_eq!(client.last_retry_count(), 1);
    }

    #[test]
    fn retries_http_500_then_succeeds() {
        let inner = FlakyModelClient::new(
            1,
            ModelError::Http {
                status: 500,
                category: "server".to_string(),
            },
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::with_config(
            Box::new(inner),
            RetryConfig {
                max_retries: 2,
                backoff: Duration::from_millis(1),
            },
        );
        client.complete(&sample_request()).expect("success");
        assert_eq!(client.last_retry_count(), 1);
    }

    #[test]
    fn authentication_is_not_retried() {
        let inner = FlakyModelClient::new(
            3,
            ModelError::Authentication("denied".to_string()),
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::new(Box::new(inner));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::Authentication(_)));
        assert_eq!(client.last_retry_count(), 0);
    }

    #[test]
    fn forbidden_is_not_retried() {
        let inner = FlakyModelClient::new(
            3,
            ModelError::Http {
                status: 403,
                category: "forbidden".to_string(),
            },
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::new(Box::new(inner));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::Http { status: 403, .. }));
        assert_eq!(client.last_retry_count(), 0);
    }
}
