//! Clasificación de fallos autónomos y selección de estrategias de recovery acotado.
//!
//! Model-agnostic: opera sobre [`ModelError`] estructurado y señales HTTP/transporte,
//! sin hardcodear proveedores ni hacer matching de strings de rate-limit.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::harness::goal_driven::ProgressAssessment;
use crate::harness::model::{ModelError, ModelResponseError, TransportFailureKind, redact_secrets};

/// Taxonomía mínima justificada por la arquitectura actual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Rate limit, timeout, 5xx, transporte intermitente.
    ExternalTransient,
    /// Credenciales, configuración, endpoint/modelo inválido.
    ExternalPermanent,
    /// El servicio respondió; las decisiones no producen progreso medible.
    ModelCapability,
    /// Fallo interno del harness / wiring / política contradictoria.
    SystemFailure,
    /// Sin progreso y sin causa más específica demostrable.
    ConvergenceStalled,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExternalTransient => "external_transient",
            Self::ExternalPermanent => "external_permanent",
            Self::ModelCapability => "model_capability",
            Self::SystemFailure => "system_failure",
            Self::ConvergenceStalled => "convergence_stalled",
        }
    }
}

/// Señal estructurada mínima observable para recovery (solo campos poblables).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredRecoverySignal {
    pub http_status: Option<u16>,
    pub retry_after: Option<Duration>,
    pub transport_kind: Option<TransportFailureKind>,
}

impl StructuredRecoverySignal {
    pub fn from_model_error(error: &ModelError) -> Self {
        Self {
            http_status: error.http_status(),
            retry_after: error.retry_after(),
            transport_kind: error.transport_kind(),
        }
    }

    pub fn summary(&self) -> String {
        let status = self
            .http_status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "none".to_string());
        let retry_after = self
            .retry_after
            .map(|d| format!("{}s", d.as_secs()))
            .unwrap_or_else(|| "none".to_string());
        let transport = self
            .transport_kind
            .map(|k| k.as_str().to_string())
            .unwrap_or_else(|| "none".to_string());
        format!("http_status={status} retry_after={retry_after} transport={transport}")
    }
}

/// Evidencia estructurada mínima para clasificar (sin secretos).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureEvidence {
    pub source: FailureSource,
    pub class: FailureClass,
    pub retryable: bool,
    pub http_status: Option<u16>,
    pub category: String,
    pub detail: String,
    pub failed_action: Option<String>,
    pub signal: StructuredRecoverySignal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureSource {
    ModelApi,
    ModelResponse,
    ProgressStall,
    InternalExecution,
}

impl FailureSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelApi => "model_api",
            Self::ModelResponse => "model_response",
            Self::ProgressStall => "progress_stall",
            Self::InternalExecution => "internal_execution",
        }
    }
}

/// Estrategias implementadas (solo las justificadas por evidencia).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStrategy {
    /// Transient sin hint temporal del proveedor → backoff configurado.
    RetryWithBackoff,
    /// Transient con `Retry-After` (u hint equivalente) → espera provider-aware.
    WaitThenRetry,
    StopExternalBlocked,
    StopConfigurationBlocked,
    StopModelCapability,
    StopSystemFailure,
    StopConvergenceStalled,
}

impl RecoveryStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetryWithBackoff => "retry_with_backoff",
            Self::WaitThenRetry => "wait_then_retry",
            Self::StopExternalBlocked => "stop_external_blocked",
            Self::StopConfigurationBlocked => "stop_configuration_blocked",
            Self::StopModelCapability => "stop_model_capability",
            Self::StopSystemFailure => "stop_system_failure",
            Self::StopConvergenceStalled => "stop_convergence_stalled",
        }
    }

    pub fn is_recover(self) -> bool {
        matches!(self, Self::RetryWithBackoff | Self::WaitThenRetry)
    }
}

/// Motivo explicable de la decisión de recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPlanReason {
    ProviderRetryAfter,
    ConfiguredBackoff,
    NonRetryableClassification,
    BudgetExhausted,
    PermanentExternal,
    ModelCapability,
    SystemFailure,
    ConvergenceStalled,
}

impl RecoveryPlanReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderRetryAfter => "provider_retry_after",
            Self::ConfiguredBackoff => "configured_backoff",
            Self::NonRetryableClassification => "non_retryable_classification",
            Self::BudgetExhausted => "budget_exhausted",
            Self::PermanentExternal => "permanent_external",
            Self::ModelCapability => "model_capability",
            Self::SystemFailure => "system_failure",
            Self::ConvergenceStalled => "convergence_stalled",
        }
    }
}

/// Decisión observable de recovery (señal → estrategia → espera).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryDecision {
    pub strategy: RecoveryStrategy,
    pub wait: Duration,
    pub reason: RecoveryPlanReason,
    pub signal: StructuredRecoverySignal,
}

/// Presupuesto acotado de recovery transitorio (además del retry del ModelClient).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryBudget {
    pub max_attempts: u32,
    pub backoff: Duration,
    pub attempts_used: u32,
}

impl RecoveryBudget {
    pub fn new(max_attempts: u32, backoff: Duration) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            backoff,
            attempts_used: 0,
        }
    }

    pub fn remaining(&self) -> bool {
        self.attempts_used < self.max_attempts
    }

    pub fn consume(&mut self) -> bool {
        if !self.remaining() {
            return false;
        }
        self.attempts_used = self.attempts_used.saturating_add(1);
        true
    }
}

impl Default for RecoveryBudget {
    fn default() -> Self {
        Self::new(3, Duration::ZERO)
    }
}

/// Abstracción mínima de espera para tests sin sleeps reales largos.
pub trait RecoveryDelay: Send + Sync {
    fn delay(&self, duration: Duration);
}

/// Espera con `thread::sleep` (omite duración cero).
#[derive(Debug, Default, Clone, Copy)]
pub struct ThreadRecoveryDelay;

impl RecoveryDelay for ThreadRecoveryDelay {
    fn delay(&self, duration: Duration) {
        if !duration.is_zero() {
            thread::sleep(duration);
        }
    }
}

/// Delay que registra esperas solicitadas sin dormir (tests).
#[derive(Debug, Default)]
pub struct RecordingRecoveryDelay {
    waits: std::sync::Mutex<Vec<Duration>>,
}

impl RecordingRecoveryDelay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn waits(&self) -> Vec<Duration> {
        self.waits.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

impl RecoveryDelay for RecordingRecoveryDelay {
    fn delay(&self, duration: Duration) {
        if let Ok(mut guard) = self.waits.lock() {
            guard.push(duration);
        }
    }
}

pub type SharedRecoveryDelay = Arc<dyn RecoveryDelay>;

pub fn default_recovery_delay() -> SharedRecoveryDelay {
    Arc::new(ThreadRecoveryDelay)
}

/// Informe terminal reutilizable por [`crate::harness::agent_loop::LoopResult`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureReport {
    pub classification: FailureClass,
    pub retryable: bool,
    pub strategy: RecoveryStrategy,
    pub recovery_attempts: u32,
    pub recovery_restored_progress: bool,
    pub source: FailureSource,
    pub category: String,
    pub detail: String,
    pub http_status: Option<u16>,
    pub goal_satisfied: bool,
    pub meaningful_progress_observed: bool,
    pub signal: StructuredRecoverySignal,
    pub plan_reason: RecoveryPlanReason,
    pub wait: Duration,
}

impl FailureReport {
    pub fn terminal_explanation(&self) -> String {
        format!(
            "goal_satisfied={} progress_observed={} classification={} retryable={} strategy={} reason={} wait_ms={} recovery_attempts={} recovery_restored={} source={} category={} detail={} signal={}",
            self.goal_satisfied,
            self.meaningful_progress_observed,
            self.classification.as_str(),
            self.retryable,
            self.strategy.as_str(),
            self.plan_reason.as_str(),
            self.wait.as_millis(),
            self.recovery_attempts,
            self.recovery_restored_progress,
            self.source.as_str(),
            self.category,
            self.detail,
            self.signal.summary()
        )
    }
}

/// Clasifica un [`ModelError`] estructurado (sin string-matching de proveedor).
pub fn classify_model_error(error: &ModelError) -> FailureEvidence {
    let detail = redact_secrets(&error.to_string());
    let signal = StructuredRecoverySignal::from_model_error(error);
    match error {
        ModelError::RateLimited { category, .. } => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: true,
            http_status: Some(429),
            category: category.clone(),
            detail,
            failed_action: None,
            signal,
        },
        ModelError::Timeout => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: true,
            http_status: None,
            category: "timeout".to_string(),
            detail,
            failed_action: None,
            signal,
        },
        ModelError::Http {
            status, category, ..
        } if (500..600).contains(status) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: true,
            http_status: Some(*status),
            category: redact_secrets(category),
            detail,
            failed_action: None,
            signal,
        },
        ModelError::Transport { message, .. } => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: error.is_retryable(),
            http_status: None,
            category: "transport".to_string(),
            detail: redact_secrets(message),
            failed_action: None,
            signal,
        },
        ModelError::Authentication(category) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalPermanent,
            retryable: false,
            http_status: None,
            category: category.clone(),
            detail,
            failed_action: None,
            signal,
        },
        ModelError::Configuration(message) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalPermanent,
            retryable: false,
            http_status: None,
            category: "configuration".to_string(),
            detail: redact_secrets(message),
            failed_action: None,
            signal,
        },
        ModelError::InvalidResponse(message) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ModelCapability,
            retryable: false,
            http_status: None,
            category: "invalid_response".to_string(),
            detail: redact_secrets(message),
            failed_action: None,
            signal,
        },
        ModelError::Http {
            status, category, ..
        } => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalPermanent,
            retryable: false,
            http_status: Some(*status),
            category: redact_secrets(category),
            detail,
            failed_action: None,
            signal,
        },
    }
}

/// Errores de parseo / validación de decisión del modelo.
pub fn classify_response_error(error: &ModelResponseError) -> FailureEvidence {
    let detail = redact_secrets(&error.to_string());
    let signal = StructuredRecoverySignal::default();
    match error {
        ModelResponseError::ContextSerializationError(_) => FailureEvidence {
            source: FailureSource::InternalExecution,
            class: FailureClass::SystemFailure,
            retryable: false,
            http_status: None,
            category: "context_serialization".to_string(),
            detail,
            failed_action: None,
            signal,
        },
        ModelResponseError::InvalidModelResponse(_)
        | ModelResponseError::UnsupportedAction(_)
        | ModelResponseError::InvalidCorrection(_)
        | ModelResponseError::InvalidFileOperation(_) => FailureEvidence {
            source: FailureSource::ModelResponse,
            class: FailureClass::ModelCapability,
            retryable: false,
            http_status: None,
            category: "unusable_model_decision".to_string(),
            detail,
            failed_action: None,
            signal,
        },
    }
}

/// Clasifica estancamiento de progreso cuando no hay error de API demostrable.
pub fn classify_progress_stall(
    assessment: &ProgressAssessment,
    tool_executed_recently: bool,
) -> FailureEvidence {
    let last_action = assessment.snapshot.last_action.as_deref();
    let model_had_opportunity = tool_executed_recently
        || last_action.is_some_and(|action| !matches!(action, "noop" | "finish"));

    let (class, category) = if model_had_opportunity {
        (
            FailureClass::ModelCapability,
            "repeated_decisions_no_progress",
        )
    } else {
        (FailureClass::ConvergenceStalled, "unknown_stall")
    };

    FailureEvidence {
        source: FailureSource::ProgressStall,
        class,
        retryable: false,
        http_status: None,
        category: category.to_string(),
        detail: redact_secrets(&assessment.reason),
        failed_action: assessment.snapshot.last_action.clone(),
        signal: StructuredRecoverySignal::default(),
    }
}

/// Evidencia de fallo interno de ejecución (tool desconocida, wiring, etc.).
pub fn classify_system_failure(
    detail: impl Into<String>,
    failed_action: Option<String>,
) -> FailureEvidence {
    FailureEvidence {
        source: FailureSource::InternalExecution,
        class: FailureClass::SystemFailure,
        retryable: false,
        http_status: None,
        category: "internal_execution".to_string(),
        detail: redact_secrets(&detail.into()),
        failed_action,
        signal: StructuredRecoverySignal::default(),
    }
}

/// Planifica recovery: ProviderHint (`Retry-After`) tiene prioridad sobre backoff genérico.
pub fn plan_recovery(evidence: &FailureEvidence, budget: &RecoveryBudget) -> RecoveryDecision {
    let signal = evidence.signal.clone();
    match evidence.class {
        FailureClass::ExternalTransient if evidence.retryable && budget.remaining() => {
            if let Some(wait) = signal.retry_after.filter(|d| !d.is_zero()) {
                RecoveryDecision {
                    strategy: RecoveryStrategy::WaitThenRetry,
                    wait,
                    reason: RecoveryPlanReason::ProviderRetryAfter,
                    signal,
                }
            } else {
                RecoveryDecision {
                    strategy: RecoveryStrategy::RetryWithBackoff,
                    wait: budget.backoff,
                    reason: RecoveryPlanReason::ConfiguredBackoff,
                    signal,
                }
            }
        }
        FailureClass::ExternalTransient => RecoveryDecision {
            strategy: RecoveryStrategy::StopExternalBlocked,
            wait: Duration::ZERO,
            reason: if evidence.retryable {
                RecoveryPlanReason::BudgetExhausted
            } else {
                RecoveryPlanReason::NonRetryableClassification
            },
            signal,
        },
        FailureClass::ExternalPermanent => RecoveryDecision {
            strategy: RecoveryStrategy::StopConfigurationBlocked,
            wait: Duration::ZERO,
            reason: RecoveryPlanReason::PermanentExternal,
            signal,
        },
        FailureClass::ModelCapability => RecoveryDecision {
            strategy: RecoveryStrategy::StopModelCapability,
            wait: Duration::ZERO,
            reason: RecoveryPlanReason::ModelCapability,
            signal,
        },
        FailureClass::SystemFailure => RecoveryDecision {
            strategy: RecoveryStrategy::StopSystemFailure,
            wait: Duration::ZERO,
            reason: RecoveryPlanReason::SystemFailure,
            signal,
        },
        FailureClass::ConvergenceStalled => RecoveryDecision {
            strategy: RecoveryStrategy::StopConvergenceStalled,
            wait: Duration::ZERO,
            reason: RecoveryPlanReason::ConvergenceStalled,
            signal,
        },
    }
}

/// Selección determinista de estrategia según clasificación, señal y presupuesto.
pub fn select_recovery_strategy(
    evidence: &FailureEvidence,
    budget: &RecoveryBudget,
) -> RecoveryStrategy {
    plan_recovery(evidence, budget).strategy
}

pub fn build_failure_report(
    evidence: &FailureEvidence,
    decision: &RecoveryDecision,
    recovery_attempts: u32,
    recovery_restored_progress: bool,
    meaningful_progress_observed: bool,
) -> FailureReport {
    FailureReport {
        classification: evidence.class,
        retryable: evidence.retryable,
        strategy: decision.strategy,
        recovery_attempts,
        recovery_restored_progress,
        source: evidence.source,
        category: evidence.category.clone(),
        detail: evidence.detail.clone(),
        http_status: evidence.http_status,
        goal_satisfied: false,
        meaningful_progress_observed,
        signal: decision.signal.clone(),
        plan_reason: decision.reason,
        wait: decision.wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_is_external_transient_retryable() {
        let evidence = classify_model_error(&ModelError::rate_limited("rate_limited"));
        assert_eq!(evidence.class, FailureClass::ExternalTransient);
        assert!(evidence.retryable);
        assert_eq!(evidence.http_status, Some(429));
        assert_eq!(evidence.signal.http_status, Some(429));
        assert!(
            !evidence
                .detail
                .to_ascii_lowercase()
                .contains("authorization")
        );
    }

    #[test]
    fn authentication_is_permanent_non_retryable() {
        let evidence = classify_model_error(&ModelError::Authentication("forbidden".into()));
        assert_eq!(evidence.class, FailureClass::ExternalPermanent);
        assert!(!evidence.retryable);
        let strategy = select_recovery_strategy(&evidence, &RecoveryBudget::default());
        assert_eq!(strategy, RecoveryStrategy::StopConfigurationBlocked);
    }

    #[test]
    fn transient_recovery_exhausts_to_external_blocked() {
        let evidence = classify_model_error(&ModelError::Timeout);
        let mut budget = RecoveryBudget::new(2, Duration::ZERO);
        assert_eq!(
            select_recovery_strategy(&evidence, &budget),
            RecoveryStrategy::RetryWithBackoff
        );
        assert!(budget.consume());
        assert!(budget.consume());
        assert_eq!(
            select_recovery_strategy(&evidence, &budget),
            RecoveryStrategy::StopExternalBlocked
        );
    }

    #[test]
    fn secrets_redacted_in_evidence_detail() {
        let evidence = classify_model_error(&ModelError::transport(
            "authorization: Bearer secret-token-value",
            TransportFailureKind::Other,
        ));
        assert!(evidence.detail.contains("[REDACTED]"));
        assert!(!evidence.detail.contains("secret-token-value"));
    }

    #[test]
    fn retry_after_plans_wait_then_retry() {
        let error = ModelError::rate_limited_with_retry_after(
            "rate_limited",
            Some(Duration::from_secs(12)),
        );
        let evidence = classify_model_error(&error);
        let decision = plan_recovery(&evidence, &RecoveryBudget::new(3, Duration::from_secs(50)));
        assert_eq!(decision.strategy, RecoveryStrategy::WaitThenRetry);
        assert_eq!(decision.reason, RecoveryPlanReason::ProviderRetryAfter);
        assert_eq!(decision.wait, Duration::from_secs(12));
    }

    #[test]
    fn absent_retry_after_uses_configured_backoff() {
        let evidence = classify_model_error(&ModelError::rate_limited("rate_limited"));
        let decision = plan_recovery(
            &evidence,
            &RecoveryBudget::new(3, Duration::from_millis(75)),
        );
        assert_eq!(decision.strategy, RecoveryStrategy::RetryWithBackoff);
        assert_eq!(decision.reason, RecoveryPlanReason::ConfiguredBackoff);
        assert_eq!(decision.wait, Duration::from_millis(75));
    }
}
