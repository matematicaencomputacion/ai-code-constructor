//! Wrapper de [`ModelClient`] con retry simple para errores transitorios.
//!
//! Observabilidad causal:
//! - [`RetryingModelClient::last_retry_count`]: retries del **último** `complete()` (semántica histórica).
//! - [`ModelRetryObservability::total`]: suma de retries de todos los `complete()` finalizados.
//! - [`ModelRetryObservability::per_call`]: retries por cada `complete()`, en orden causal.
//!
//! AiAgent / AgentLoop no conocen esta observabilidad; el orquestador retiene el handle.

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

#[derive(Debug, Default)]
struct RetryMetricsState {
    last: u32,
    total: u32,
    per_call: Vec<u32>,
}

/// Handle clonable de observabilidad de retries (transporte/modelo).
///
/// Retenido por el orquestador de sesión; no forma parte de AgentObservation.
#[derive(Debug, Clone)]
pub struct ModelRetryObservability {
    inner: Arc<Mutex<RetryMetricsState>>,
}

impl ModelRetryObservability {
    fn new(inner: Arc<Mutex<RetryMetricsState>>) -> Self {
        Self { inner }
    }

    /// Retries del último `complete()` finalizado (misma semántica que
    /// [`RetryingModelClient::last_retry_count`]).
    pub fn last(&self) -> u32 {
        self.inner.lock().map(|state| state.last).unwrap_or(0)
    }

    /// Suma de retries de todos los `complete()` finalizados en la vida del cliente.
    pub fn total(&self) -> u32 {
        self.inner.lock().map(|state| state.total).unwrap_or(0)
    }

    /// Retries por cada `complete()` finalizado, en orden causal.
    pub fn per_call(&self) -> Vec<u32> {
        self.inner
            .lock()
            .map(|state| state.per_call.clone())
            .unwrap_or_default()
    }
}

/// Envuelve un [`ModelClient`] y reintenta errores transitorios (429, timeout, 5xx).
pub struct RetryingModelClient {
    inner: Box<dyn ModelClient>,
    config: RetryConfig,
    metrics: Arc<Mutex<RetryMetricsState>>,
}

impl RetryingModelClient {
    pub fn new(inner: Box<dyn ModelClient>) -> Self {
        Self::with_config(inner, RetryConfig::default())
    }

    pub fn with_config(inner: Box<dyn ModelClient>, config: RetryConfig) -> Self {
        Self {
            inner,
            config,
            metrics: Arc::new(Mutex::new(RetryMetricsState::default())),
        }
    }

    /// Cantidad de retries del **último** `complete()` finalizado.
    ///
    /// No es el total de sesión; ver [`Self::observability`].
    pub fn last_retry_count(&self) -> u32 {
        self.metrics.lock().map(|state| state.last).unwrap_or(0)
    }

    /// Handle compartido para proyectar métricas hacia LiveSession / ConstructionObservability.
    pub fn observability(&self) -> ModelRetryObservability {
        ModelRetryObservability::new(Arc::clone(&self.metrics))
    }

    fn record_finished_complete(&self, retries: u32) {
        if let Ok(mut state) = self.metrics.lock() {
            state.last = retries;
            state.total = state.total.saturating_add(retries);
            state.per_call.push(retries);
        }
    }
}

impl ModelClient for RetryingModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let mut attempt = 0_u32;
        loop {
            match self.inner.complete(request) {
                Ok(response) => {
                    self.record_finished_complete(attempt);
                    return Ok(response);
                }
                Err(error) if attempt < self.config.max_retries && error.is_retryable() => {
                    attempt += 1;
                    if !self.config.backoff.is_zero() {
                        thread::sleep(self.config.backoff);
                    }
                }
                Err(error) => {
                    self.record_finished_complete(attempt);
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

    /// Programa fallos por grupo de `complete()` externo (índice = completes finalizados).
    struct ScheduledFlaky {
        schedule: Vec<u32>,
        done_groups: AtomicU32,
        fails_emitted: AtomicU32,
        always_fail_final: bool,
    }

    impl ScheduledFlaky {
        fn new(schedule: Vec<u32>) -> Self {
            Self {
                schedule,
                done_groups: AtomicU32::new(0),
                fails_emitted: AtomicU32::new(0),
                always_fail_final: false,
            }
        }

        fn always_fail(planned_fails_before_exhaust: u32) -> Self {
            Self {
                schedule: vec![planned_fails_before_exhaust],
                done_groups: AtomicU32::new(0),
                fails_emitted: AtomicU32::new(0),
                always_fail_final: true,
            }
        }
    }

    impl ModelClient for ScheduledFlaky {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            let group = self.done_groups.load(Ordering::SeqCst) as usize;
            let planned = self.schedule.get(group).copied().unwrap_or(0);
            let emitted = self.fails_emitted.load(Ordering::SeqCst);
            if emitted < planned {
                self.fails_emitted.fetch_add(1, Ordering::SeqCst);
                return Err(ModelError::Timeout);
            }
            if self.always_fail_final {
                // Keep failing so Retrying exhausts max_retries.
                return Err(ModelError::Timeout);
            }
            self.fails_emitted.store(0, Ordering::SeqCst);
            self.done_groups.fetch_add(1, Ordering::SeqCst);
            Ok(ModelResponse {
                raw_text: r#"{"action":"finish","summary":"ok"}"#.to_string(),
            })
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
            artifact_primary_path: Some("main.rs".to_string()),
            artifact_files: vec![crate::harness::model::ArtifactFileSnapshot {
                path: "main.rs".to_string(),
                source: "fn main() {}".to_string(),
            }],
            system_prompt: system_prompt_v1().to_string(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: None,
            goal_gap: None,
        }
    }

    fn zero_backoff(max_retries: u32) -> RetryConfig {
        RetryConfig {
            max_retries,
            backoff: Duration::from_millis(0),
        }
    }

    #[test]
    fn retries_rate_limited_then_succeeds() {
        let inner = FlakyModelClient::new(
            2,
            ModelError::RateLimited("slow".to_string()),
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::with_config(Box::new(inner), zero_backoff(2));
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
        let client = RetryingModelClient::with_config(Box::new(inner), zero_backoff(2));
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
        let client = RetryingModelClient::with_config(Box::new(inner), zero_backoff(2));
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
        let client = RetryingModelClient::with_config(Box::new(inner), zero_backoff(2));
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
        let client = RetryingModelClient::with_config(Box::new(inner), zero_backoff(2));
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::Http { status: 403, .. }));
        assert_eq!(client.last_retry_count(), 0);
    }

    #[test]
    fn observability_zero_retries_on_first_success() {
        // A
        let inner = FlakyModelClient::new(
            0,
            ModelError::Timeout,
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::with_config(Box::new(inner), zero_backoff(2));
        let obs = client.observability();
        client.complete(&sample_request()).expect("ok");
        assert_eq!(obs.last(), 0);
        assert_eq!(obs.total(), 0);
        assert_eq!(obs.per_call(), vec![0]);
        assert_eq!(client.last_retry_count(), 0);
    }

    #[test]
    fn observability_records_successful_retries() {
        // B
        let inner = FlakyModelClient::new(
            2,
            ModelError::Timeout,
            r#"{"action":"finish","summary":"ok"}"#,
        );
        let client = RetryingModelClient::with_config(Box::new(inner), zero_backoff(2));
        let obs = client.observability();
        client.complete(&sample_request()).expect("ok");
        assert_eq!(obs.last(), 2);
        assert_eq!(obs.total(), 2);
        assert_eq!(obs.per_call(), vec![2]);
    }

    #[test]
    fn observability_accumulates_across_completes() {
        // C: call1 → 1 retry, call2 → 0, call3 → 2
        let client = RetryingModelClient::with_config(
            Box::new(ScheduledFlaky::new(vec![1, 0, 2])),
            zero_backoff(3),
        );
        let obs = client.observability();
        client.complete(&sample_request()).expect("c1");
        client.complete(&sample_request()).expect("c2");
        client.complete(&sample_request()).expect("c3");
        assert_eq!(obs.last(), 2);
        assert_eq!(obs.total(), 3);
        assert_eq!(obs.per_call(), vec![1, 0, 2]);
        assert_eq!(client.last_retry_count(), 2);
    }

    #[test]
    fn observability_records_final_error_retries_once() {
        // D: exhaust retries → one per_call entry
        let client = RetryingModelClient::with_config(
            Box::new(ScheduledFlaky::always_fail(2)),
            zero_backoff(2),
        );
        let obs = client.observability();
        let err = client.complete(&sample_request()).unwrap_err();
        assert!(matches!(err, ModelError::Timeout));
        assert_eq!(obs.last(), 2);
        assert_eq!(obs.total(), 2);
        assert_eq!(obs.per_call(), vec![2]);
        assert_eq!(client.last_retry_count(), 2);
    }
}
