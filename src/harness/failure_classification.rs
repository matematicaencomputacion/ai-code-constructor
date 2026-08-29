//! Clasificación de fallos autónomos y selección de estrategias de recovery acotado.
//!
//! Model-agnostic: opera sobre [`ModelError`] estructurado y señales de progreso,
//! sin hardcodear proveedores ni hacer matching de strings de rate-limit.

use std::time::Duration;

use crate::harness::goal_driven::ProgressAssessment;
use crate::harness::model::{ModelError, ModelResponseError, redact_secrets};

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
    RetryWithBackoff,
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
            Self::StopExternalBlocked => "stop_external_blocked",
            Self::StopConfigurationBlocked => "stop_configuration_blocked",
            Self::StopModelCapability => "stop_model_capability",
            Self::StopSystemFailure => "stop_system_failure",
            Self::StopConvergenceStalled => "stop_convergence_stalled",
        }
    }

    pub fn is_recover(self) -> bool {
        matches!(self, Self::RetryWithBackoff)
    }
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
}

impl FailureReport {
    pub fn terminal_explanation(&self) -> String {
        format!(
            "goal_satisfied={} progress_observed={} classification={} retryable={} strategy={} recovery_attempts={} recovery_restored={} source={} category={} detail={}",
            self.goal_satisfied,
            self.meaningful_progress_observed,
            self.classification.as_str(),
            self.retryable,
            self.strategy.as_str(),
            self.recovery_attempts,
            self.recovery_restored_progress,
            self.source.as_str(),
            self.category,
            self.detail
        )
    }
}

/// Clasifica un [`ModelError`] estructurado (sin string-matching de proveedor).
pub fn classify_model_error(error: &ModelError) -> FailureEvidence {
    let detail = redact_secrets(&error.to_string());
    match error {
        ModelError::RateLimited(category) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: true,
            http_status: Some(429),
            category: category.clone(),
            detail,
            failed_action: None,
        },
        ModelError::Timeout => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: true,
            http_status: None,
            category: "timeout".to_string(),
            detail,
            failed_action: None,
        },
        ModelError::Http { status, category } if (500..600).contains(status) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: true,
            http_status: Some(*status),
            category: redact_secrets(category),
            detail,
            failed_action: None,
        },
        ModelError::Transport(message) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalTransient,
            retryable: error.is_retryable(),
            http_status: None,
            category: "transport".to_string(),
            detail: redact_secrets(message),
            failed_action: None,
        },
        ModelError::Authentication(category) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalPermanent,
            retryable: false,
            http_status: None,
            category: category.clone(),
            detail,
            failed_action: None,
        },
        ModelError::Configuration(message) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalPermanent,
            retryable: false,
            http_status: None,
            category: "configuration".to_string(),
            detail: redact_secrets(message),
            failed_action: None,
        },
        ModelError::InvalidResponse(message) => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ModelCapability,
            retryable: false,
            http_status: None,
            category: "invalid_response".to_string(),
            detail: redact_secrets(message),
            failed_action: None,
        },
        ModelError::Http { status, category } => FailureEvidence {
            source: FailureSource::ModelApi,
            class: FailureClass::ExternalPermanent,
            retryable: false,
            http_status: Some(*status),
            category: redact_secrets(category),
            detail,
            failed_action: None,
        },
    }
}

/// Errores de parseo / validación de decisión del modelo.
pub fn classify_response_error(error: &ModelResponseError) -> FailureEvidence {
    let detail = redact_secrets(&error.to_string());
    match error {
        ModelResponseError::ContextSerializationError(_) => FailureEvidence {
            source: FailureSource::InternalExecution,
            class: FailureClass::SystemFailure,
            retryable: false,
            http_status: None,
            category: "context_serialization".to_string(),
            detail,
            failed_action: None,
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
    }
}

/// Selección determinista de estrategia según clasificación y presupuesto.
pub fn select_recovery_strategy(
    evidence: &FailureEvidence,
    budget: &RecoveryBudget,
) -> RecoveryStrategy {
    match evidence.class {
        FailureClass::ExternalTransient if evidence.retryable && budget.remaining() => {
            RecoveryStrategy::RetryWithBackoff
        }
        FailureClass::ExternalTransient => RecoveryStrategy::StopExternalBlocked,
        FailureClass::ExternalPermanent => RecoveryStrategy::StopConfigurationBlocked,
        FailureClass::ModelCapability => RecoveryStrategy::StopModelCapability,
        FailureClass::SystemFailure => RecoveryStrategy::StopSystemFailure,
        FailureClass::ConvergenceStalled => RecoveryStrategy::StopConvergenceStalled,
    }
}

pub fn build_failure_report(
    evidence: &FailureEvidence,
    strategy: RecoveryStrategy,
    recovery_attempts: u32,
    recovery_restored_progress: bool,
    meaningful_progress_observed: bool,
) -> FailureReport {
    FailureReport {
        classification: evidence.class,
        retryable: evidence.retryable,
        strategy,
        recovery_attempts,
        recovery_restored_progress,
        source: evidence.source,
        category: evidence.category.clone(),
        detail: evidence.detail.clone(),
        http_status: evidence.http_status,
        goal_satisfied: false,
        meaningful_progress_observed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_is_external_transient_retryable() {
        let evidence = classify_model_error(&ModelError::RateLimited("rate_limited".into()));
        assert_eq!(evidence.class, FailureClass::ExternalTransient);
        assert!(evidence.retryable);
        assert_eq!(evidence.http_status, Some(429));
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
        let evidence = classify_model_error(&ModelError::Transport(
            "authorization: Bearer secret-token-value".into(),
        ));
        assert!(evidence.detail.contains("[REDACTED]"));
        assert!(!evidence.detail.contains("secret-token-value"));
    }
}
