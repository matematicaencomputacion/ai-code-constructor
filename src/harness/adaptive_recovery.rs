//! Coordinación determinista entre recovery transitorio y routing multi-modelo.
//!
//! La clasificación, el plan de recovery y el plan de routing siguen siendo capas
//! independientes. Este módulo compone sus decisiones bajo un único presupuesto
//! de sesión para impedir que retries, esperas y cambios de modelo se autoricen
//! de forma aislada.

use std::time::Duration;

use crate::harness::failure_classification::{RecoveryBudget, RecoveryDecision};
use crate::harness::model_routing::RoutingDecision;

/// Presupuesto común de las estrategias adaptativas de recuperación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveRecoveryBudget {
    pub recovery: RecoveryBudget,
    pub max_model_switches: u32,
    pub model_switches_used: u32,
    pub max_cumulative_wait: Duration,
    pub cumulative_wait: Duration,
}

impl AdaptiveRecoveryBudget {
    pub fn new(
        recovery: RecoveryBudget,
        max_model_switches: u32,
        max_cumulative_wait: Duration,
    ) -> Self {
        Self {
            recovery,
            max_model_switches,
            model_switches_used: 0,
            max_cumulative_wait,
            cumulative_wait: Duration::ZERO,
        }
    }

    pub fn can_recover(&self, wait: Duration) -> bool {
        self.recovery.remaining() && self.can_wait(wait)
    }

    pub fn can_wait(&self, wait: Duration) -> bool {
        wait.is_zero()
            || self
                .cumulative_wait
                .checked_add(wait)
                .is_some_and(|total| total <= self.max_cumulative_wait)
    }

    pub fn consume_recovery(&mut self, wait: Duration) -> bool {
        if !self.can_recover(wait) || !self.recovery.consume() {
            return false;
        }
        self.cumulative_wait = self.cumulative_wait.saturating_add(wait);
        true
    }

    pub fn can_switch_model(&self) -> bool {
        self.model_switches_used < self.max_model_switches
    }

    pub fn consume_model_switch(&mut self) -> bool {
        if !self.can_switch_model() {
            return false;
        }
        self.model_switches_used = self.model_switches_used.saturating_add(1);
        true
    }

    pub fn snapshot(&self) -> AdaptiveRecoveryBudgetSnapshot {
        AdaptiveRecoveryBudgetSnapshot {
            recovery_attempts_used: self.recovery.attempts_used,
            recovery_attempts_max: self.recovery.max_attempts,
            model_switches_used: self.model_switches_used,
            model_switches_max: self.max_model_switches,
            cumulative_wait: self.cumulative_wait,
            max_cumulative_wait: self.max_cumulative_wait,
        }
    }
}

impl Default for AdaptiveRecoveryBudget {
    fn default() -> Self {
        Self::new(RecoveryBudget::default(), 2, Duration::from_secs(300))
    }
}

/// Foto inmutable del presupuesto en el momento de decidir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveRecoveryBudgetSnapshot {
    pub recovery_attempts_used: u32,
    pub recovery_attempts_max: u32,
    pub model_switches_used: u32,
    pub model_switches_max: u32,
    pub cumulative_wait: Duration,
    pub max_cumulative_wait: Duration,
}

/// Acción final del coordinador adaptativo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveRecoveryAction {
    RetrySameModel,
    RouteModel,
    Stop,
}

impl AdaptiveRecoveryAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetrySameModel => "retry_same_model",
            Self::RouteModel => "route_model",
            Self::Stop => "stop",
        }
    }
}

/// Motivo explicable de la decisión compuesta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveRecoveryReason {
    RecoveryAuthorized,
    ModelRouteAuthorized,
    RecoveryAttemptsExhausted,
    CumulativeWaitExhausted,
    ModelSwitchBudgetExhausted,
    RoutingDidNotChangeModel,
    NoRoutingAvailable,
}

impl AdaptiveRecoveryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecoveryAuthorized => "recovery_authorized",
            Self::ModelRouteAuthorized => "model_route_authorized",
            Self::RecoveryAttemptsExhausted => "recovery_attempts_exhausted",
            Self::CumulativeWaitExhausted => "cumulative_wait_exhausted",
            Self::ModelSwitchBudgetExhausted => "model_switch_budget_exhausted",
            Self::RoutingDidNotChangeModel => "routing_did_not_change_model",
            Self::NoRoutingAvailable => "no_routing_available",
        }
    }
}

/// Decisión final, con los planes de origen y el presupuesto que la justificó.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveRecoveryDecision {
    pub action: AdaptiveRecoveryAction,
    pub reason: AdaptiveRecoveryReason,
    pub recovery: RecoveryDecision,
    pub routing: Option<RoutingDecision>,
    pub budget: AdaptiveRecoveryBudgetSnapshot,
}

impl AdaptiveRecoveryDecision {
    pub fn summary(&self) -> String {
        format!(
            "action={} reason={} recovery={} route={} attempts={}/{} switches={}/{} wait_ms={}/{}",
            self.action.as_str(),
            self.reason.as_str(),
            self.recovery.strategy.as_str(),
            self.routing
                .as_ref()
                .map(|decision| decision.action.as_str())
                .unwrap_or("none"),
            self.budget.recovery_attempts_used,
            self.budget.recovery_attempts_max,
            self.budget.model_switches_used,
            self.budget.model_switches_max,
            self.budget.cumulative_wait.as_millis(),
            self.budget.max_cumulative_wait.as_millis(),
        )
    }
}

/// Compone recovery y routing sin mutar Agent, presupuestos ni clientes.
pub fn plan_adaptive_recovery(
    recovery: RecoveryDecision,
    routing: Option<RoutingDecision>,
    budget: &AdaptiveRecoveryBudget,
) -> AdaptiveRecoveryDecision {
    let (action, reason) = if recovery.strategy.is_recover() {
        if !budget.recovery.remaining() {
            (
                AdaptiveRecoveryAction::Stop,
                AdaptiveRecoveryReason::RecoveryAttemptsExhausted,
            )
        } else if !budget.can_wait(recovery.wait) {
            (
                AdaptiveRecoveryAction::Stop,
                AdaptiveRecoveryReason::CumulativeWaitExhausted,
            )
        } else {
            (
                AdaptiveRecoveryAction::RetrySameModel,
                AdaptiveRecoveryReason::RecoveryAuthorized,
            )
        }
    } else if let Some(route) = routing.as_ref() {
        if route.action.changes_model() {
            if budget.can_switch_model() {
                (
                    AdaptiveRecoveryAction::RouteModel,
                    AdaptiveRecoveryReason::ModelRouteAuthorized,
                )
            } else {
                (
                    AdaptiveRecoveryAction::Stop,
                    AdaptiveRecoveryReason::ModelSwitchBudgetExhausted,
                )
            }
        } else {
            (
                AdaptiveRecoveryAction::Stop,
                AdaptiveRecoveryReason::RoutingDidNotChangeModel,
            )
        }
    } else {
        (
            AdaptiveRecoveryAction::Stop,
            AdaptiveRecoveryReason::NoRoutingAvailable,
        )
    };

    AdaptiveRecoveryDecision {
        action,
        reason,
        recovery,
        routing,
        budget: budget.snapshot(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::failure_classification::{
        FailureClass, FailureSource, StructuredRecoverySignal, classify_model_error, plan_recovery,
    };
    use crate::harness::model::ModelError;
    use crate::harness::model_routing::{
        ModelIdentity, RoutingAction, RoutingDecision, RoutingReason,
    };

    fn route(action: RoutingAction) -> RoutingDecision {
        RoutingDecision {
            action,
            reason: RoutingReason::ModelCapabilityEscalate,
            from: ModelIdentity::new("provider", "low"),
            to: action
                .changes_model()
                .then(|| ModelIdentity::new("provider", "high")),
            failure_class: FailureClass::ModelCapability,
            escalation_used: 0,
            escalation_remaining: 1,
        }
    }

    fn capability_evidence() -> crate::harness::failure_classification::FailureEvidence {
        crate::harness::failure_classification::FailureEvidence {
            source: FailureSource::ProgressStall,
            class: FailureClass::ModelCapability,
            retryable: false,
            http_status: None,
            category: "stalled".into(),
            detail: "stalled".into(),
            failed_action: None,
            signal: StructuredRecoverySignal::default(),
        }
    }

    #[test]
    fn retry_is_authorized_by_attempt_and_wait_budget() {
        let evidence = classify_model_error(&ModelError::rate_limited_with_retry_after(
            "rate_limited",
            Some(Duration::from_secs(2)),
        ));
        let budget = AdaptiveRecoveryBudget::new(
            RecoveryBudget::new(2, Duration::ZERO),
            1,
            Duration::from_secs(5),
        );
        let decision =
            plan_adaptive_recovery(plan_recovery(&evidence, &budget.recovery), None, &budget);
        assert_eq!(decision.action, AdaptiveRecoveryAction::RetrySameModel);
        assert_eq!(decision.reason, AdaptiveRecoveryReason::RecoveryAuthorized);
    }

    #[test]
    fn cumulative_wait_is_a_hard_stop() {
        let evidence = classify_model_error(&ModelError::rate_limited_with_retry_after(
            "rate_limited",
            Some(Duration::from_secs(6)),
        ));
        let budget = AdaptiveRecoveryBudget::new(
            RecoveryBudget::new(2, Duration::ZERO),
            1,
            Duration::from_secs(5),
        );
        let decision =
            plan_adaptive_recovery(plan_recovery(&evidence, &budget.recovery), None, &budget);
        assert_eq!(decision.action, AdaptiveRecoveryAction::Stop);
        assert_eq!(
            decision.reason,
            AdaptiveRecoveryReason::CumulativeWaitExhausted
        );
    }

    #[test]
    fn terminal_recovery_can_be_replaced_by_model_route() {
        let evidence = capability_evidence();
        let budget = AdaptiveRecoveryBudget::default();
        let decision = plan_adaptive_recovery(
            plan_recovery(&evidence, &budget.recovery),
            Some(route(RoutingAction::EscalateCapability)),
            &budget,
        );
        assert_eq!(decision.action, AdaptiveRecoveryAction::RouteModel);
        assert_eq!(
            decision.reason,
            AdaptiveRecoveryReason::ModelRouteAuthorized
        );
    }

    #[test]
    fn common_switch_budget_can_veto_specialized_routing() {
        let evidence = capability_evidence();
        let budget = AdaptiveRecoveryBudget::new(RecoveryBudget::default(), 0, Duration::ZERO);
        let decision = plan_adaptive_recovery(
            plan_recovery(&evidence, &budget.recovery),
            Some(route(RoutingAction::EscalateCapability)),
            &budget,
        );
        assert_eq!(decision.action, AdaptiveRecoveryAction::Stop);
        assert_eq!(
            decision.reason,
            AdaptiveRecoveryReason::ModelSwitchBudgetExhausted
        );
    }

    #[test]
    fn zero_wait_retry_is_allowed_when_wait_budget_is_zero() {
        let evidence = classify_model_error(&ModelError::Timeout);
        let budget =
            AdaptiveRecoveryBudget::new(RecoveryBudget::new(1, Duration::ZERO), 0, Duration::ZERO);
        let decision =
            plan_adaptive_recovery(plan_recovery(&evidence, &budget.recovery), None, &budget);
        assert_eq!(decision.action, AdaptiveRecoveryAction::RetrySameModel);
    }
}
