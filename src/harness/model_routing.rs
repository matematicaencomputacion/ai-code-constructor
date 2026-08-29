//! Routing multi-modelo y escalación de capacidad acotada.
//!
//! Capa encima de la taxonomía de fallos (#36) y recovery signal-aware (#37):
//! decide Stay / Wait / Switch / Escalate / Stop sin hardcodear nombres de
//! modelos de producción ni hacer fallback ciego.

use crate::harness::failure_classification::{FailureClass, FailureEvidence};

/// Nivel relativo de capacidad o costo (no es catálogo comercial).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelativeTier {
    Low = 0,
    Medium = 1,
    High = 2,
}

impl RelativeTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Identidad observable de un modelo (provider + id configurado).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelIdentity {
    pub provider: String,
    pub model_id: String,
}

impl ModelIdentity {
    pub fn new(provider: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
        }
    }

    pub fn summary(&self) -> String {
        format!("{}:{}", self.provider, self.model_id)
    }
}

/// Candidato de routing: representación estructurada, no política de precios.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCandidate {
    pub provider: String,
    pub model_id: String,
    pub capability_tier: RelativeTier,
    pub cost_tier: RelativeTier,
    pub available: bool,
}

impl ModelCandidate {
    pub fn new(
        provider: impl Into<String>,
        model_id: impl Into<String>,
        capability_tier: RelativeTier,
        cost_tier: RelativeTier,
    ) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
            capability_tier,
            cost_tier,
            available: true,
        }
    }

    pub fn with_availability(mut self, available: bool) -> Self {
        self.available = available;
        self
    }

    pub fn identity(&self) -> ModelIdentity {
        ModelIdentity::new(self.provider.clone(), self.model_id.clone())
    }
}

/// Acción de routing determinista.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingAction {
    Stay,
    WaitSameModel,
    SwitchAlternative,
    EscalateCapability,
    Stop,
}

impl RoutingAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stay => "stay",
            Self::WaitSameModel => "wait_same_model",
            Self::SwitchAlternative => "switch_alternative",
            Self::EscalateCapability => "escalate_capability",
            Self::Stop => "stop",
        }
    }

    pub fn changes_model(self) -> bool {
        matches!(self, Self::SwitchAlternative | Self::EscalateCapability)
    }
}

/// Motivo explicable de la decisión de routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingReason {
    ProgressNoEscalation,
    ExternalTransientWait,
    ExternalTransientBackoff,
    ExternalPermanentSwitch,
    ExternalPermanentNoAlternative,
    ModelCapabilityEscalate,
    ModelCapabilityNoHigherTier,
    ConvergenceEscalate,
    ConvergenceStop,
    SystemFailureNoEscalation,
    EscalationBudgetExhausted,
    NoRouteableCandidates,
}

impl RoutingReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProgressNoEscalation => "progress_no_escalation",
            Self::ExternalTransientWait => "external_transient_wait",
            Self::ExternalTransientBackoff => "external_transient_backoff",
            Self::ExternalPermanentSwitch => "external_permanent_switch",
            Self::ExternalPermanentNoAlternative => "external_permanent_no_alternative",
            Self::ModelCapabilityEscalate => "model_capability_escalate",
            Self::ModelCapabilityNoHigherTier => "model_capability_no_higher_tiers",
            Self::ConvergenceEscalate => "convergence_escalate",
            Self::ConvergenceStop => "convergence_stop",
            Self::SystemFailureNoEscalation => "system_failure_no_escalation",
            Self::EscalationBudgetExhausted => "escalation_budget_exhausted",
            Self::NoRouteableCandidates => "no_routeable_candidates",
        }
    }
}

/// Decisión de routing estructurada (fuente de estado, no logs libres).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingDecision {
    pub action: RoutingAction,
    pub reason: RoutingReason,
    pub from: ModelIdentity,
    pub to: Option<ModelIdentity>,
    pub failure_class: FailureClass,
    pub escalation_used: u32,
    pub escalation_remaining: u32,
}

impl RoutingDecision {
    pub fn summary(&self) -> String {
        format!(
            "action={} reason={} from={} to={} class={} escalation={}/{}",
            self.action.as_str(),
            self.reason.as_str(),
            self.from.summary(),
            self.to
                .as_ref()
                .map(ModelIdentity::summary)
                .unwrap_or_else(|| "none".to_string()),
            self.failure_class.as_str(),
            self.escalation_used,
            self.escalation_used + self.escalation_remaining,
        )
    }
}

/// Presupuesto acotado de cambios de modelo/proveedor por Goal.
///
/// Impide oscilación LOW↔HIGH y cambios infinitos: solo se consume al aplicar
/// Switch/Escalate, y la selección nunca re-visita identidades ya usadas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationBudget {
    pub max_switches: u32,
    pub switches_used: u32,
    pub visited: Vec<ModelIdentity>,
}

impl EscalationBudget {
    pub fn new(max_switches: u32) -> Self {
        Self {
            max_switches: max_switches.max(1),
            switches_used: 0,
            visited: Vec::new(),
        }
    }

    pub fn remaining(&self) -> bool {
        self.switches_used < self.max_switches
    }

    pub fn remaining_count(&self) -> u32 {
        self.max_switches.saturating_sub(self.switches_used)
    }

    pub fn mark_visited(&mut self, identity: ModelIdentity) {
        if !self.visited.iter().any(|item| item == &identity) {
            self.visited.push(identity);
        }
    }

    pub fn consume(&mut self, identity: &ModelIdentity) -> bool {
        if !self.remaining() {
            return false;
        }
        self.switches_used = self.switches_used.saturating_add(1);
        self.mark_visited(identity.clone());
        true
    }

    pub fn has_visited(&self, identity: &ModelIdentity) -> bool {
        self.visited.iter().any(|item| item == identity)
    }
}

impl Default for EscalationBudget {
    fn default() -> Self {
        Self::new(2)
    }
}

/// Entradas inmutables para planificar routing.
#[derive(Debug, Clone, Copy)]
pub struct RoutingPlanInput<'a> {
    pub active: &'a ModelCandidate,
    pub candidates: &'a [ModelCandidate],
    pub budget: &'a EscalationBudget,
    pub meaningful_progress_observed: bool,
}

fn decision(
    action: RoutingAction,
    reason: RoutingReason,
    from: ModelIdentity,
    to: Option<ModelIdentity>,
    class: FailureClass,
    budget: &EscalationBudget,
) -> RoutingDecision {
    RoutingDecision {
        action,
        reason,
        from,
        to,
        failure_class: class,
        escalation_used: budget.switches_used,
        escalation_remaining: budget.remaining_count(),
    }
}

fn find_alternative(
    active: &ModelCandidate,
    candidates: &[ModelCandidate],
    budget: &EscalationBudget,
) -> Option<usize> {
    let active_id = active.identity();
    candidates.iter().position(|candidate| {
        candidate.available
            && candidate.identity() != active_id
            && !budget.has_visited(&candidate.identity())
    })
}

/// Escala solo hacia mayor `capability_tier` (nunca degrada → anti-oscilación).
fn find_capability_escalation(
    active: &ModelCandidate,
    candidates: &[ModelCandidate],
    budget: &EscalationBudget,
) -> Option<usize> {
    let active_id = active.identity();
    candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate.available
                && candidate.identity() != active_id
                && !budget.has_visited(&candidate.identity())
                && candidate.capability_tier > active.capability_tier
        })
        .min_by_key(|(_, candidate)| {
            (
                candidate.capability_tier,
                candidate.cost_tier,
                candidate.model_id.as_str(),
            )
        })
        .map(|(index, _)| index)
}

/// Planifica routing determinista a partir de clasificación + candidatos + presupuesto.
pub fn plan_routing(evidence: &FailureEvidence, input: RoutingPlanInput<'_>) -> RoutingDecision {
    let from = input.active.identity();
    let class = evidence.class;

    // PROGRESS → NO_ESCALATION_REQUIRED (salvo transients, que esperan el mismo modelo).
    if input.meaningful_progress_observed
        && !matches!(
            class,
            FailureClass::ExternalTransient | FailureClass::SystemFailure
        )
    {
        return decision(
            RoutingAction::Stay,
            RoutingReason::ProgressNoEscalation,
            from,
            None,
            class,
            input.budget,
        );
    }

    match class {
        FailureClass::SystemFailure => decision(
            RoutingAction::Stop,
            RoutingReason::SystemFailureNoEscalation,
            from,
            None,
            class,
            input.budget,
        ),
        FailureClass::ExternalTransient => {
            let reason = if evidence.signal.retry_after.is_some() {
                RoutingReason::ExternalTransientWait
            } else {
                RoutingReason::ExternalTransientBackoff
            };
            decision(
                RoutingAction::WaitSameModel,
                reason,
                from,
                None,
                class,
                input.budget,
            )
        }
        FailureClass::ExternalPermanent => {
            if !input.budget.remaining() {
                return decision(
                    RoutingAction::Stop,
                    RoutingReason::EscalationBudgetExhausted,
                    from,
                    None,
                    class,
                    input.budget,
                );
            }
            match find_alternative(input.active, input.candidates, input.budget) {
                Some(index) => decision(
                    RoutingAction::SwitchAlternative,
                    RoutingReason::ExternalPermanentSwitch,
                    from,
                    Some(input.candidates[index].identity()),
                    class,
                    input.budget,
                ),
                None => decision(
                    RoutingAction::Stop,
                    RoutingReason::ExternalPermanentNoAlternative,
                    from,
                    None,
                    class,
                    input.budget,
                ),
            }
        }
        FailureClass::ModelCapability => {
            if !input.budget.remaining() {
                return decision(
                    RoutingAction::Stop,
                    RoutingReason::EscalationBudgetExhausted,
                    from,
                    None,
                    class,
                    input.budget,
                );
            }
            match find_capability_escalation(input.active, input.candidates, input.budget) {
                Some(index) => decision(
                    RoutingAction::EscalateCapability,
                    RoutingReason::ModelCapabilityEscalate,
                    from,
                    Some(input.candidates[index].identity()),
                    class,
                    input.budget,
                ),
                None => decision(
                    RoutingAction::Stop,
                    if input.candidates.iter().any(|c| c.available) {
                        RoutingReason::ModelCapabilityNoHigherTier
                    } else {
                        RoutingReason::NoRouteableCandidates
                    },
                    from,
                    None,
                    class,
                    input.budget,
                ),
            }
        }
        FailureClass::ConvergenceStalled => {
            if !input.budget.remaining() {
                return decision(
                    RoutingAction::Stop,
                    RoutingReason::EscalationBudgetExhausted,
                    from,
                    None,
                    class,
                    input.budget,
                );
            }
            match find_capability_escalation(input.active, input.candidates, input.budget) {
                Some(index) => decision(
                    RoutingAction::EscalateCapability,
                    RoutingReason::ConvergenceEscalate,
                    from,
                    Some(input.candidates[index].identity()),
                    class,
                    input.budget,
                ),
                None => decision(
                    RoutingAction::Stop,
                    RoutingReason::ConvergenceStop,
                    from,
                    None,
                    class,
                    input.budget,
                ),
            }
        }
    }
}

/// Aplica un cambio de modelo si la decisión lo requiere; actualiza presupuesto/visitas.
pub fn apply_routing_decision(
    decision: &RoutingDecision,
    active_index: &mut usize,
    candidates: &[ModelCandidate],
    budget: &mut EscalationBudget,
) -> bool {
    if !decision.action.changes_model() {
        return false;
    }
    let Some(target) = &decision.to else {
        return false;
    };
    let Some(index) = candidates
        .iter()
        .position(|candidate| &candidate.identity() == target && candidate.available)
    else {
        return false;
    };
    if !budget.consume(target) {
        return false;
    }
    budget.mark_visited(decision.from.clone());
    *active_index = index;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::failure_classification::{
        FailureSource, StructuredRecoverySignal, classify_model_error, classify_system_failure,
    };
    use crate::harness::model::ModelError;
    use std::time::Duration;

    fn low(id: &str) -> ModelCandidate {
        ModelCandidate::new("provider-a", id, RelativeTier::Low, RelativeTier::Low)
    }

    fn medium(id: &str) -> ModelCandidate {
        ModelCandidate::new("provider-a", id, RelativeTier::Medium, RelativeTier::Medium)
    }

    fn high(id: &str) -> ModelCandidate {
        ModelCandidate::new("provider-b", id, RelativeTier::High, RelativeTier::High)
    }

    fn input<'a>(
        active: &'a ModelCandidate,
        candidates: &'a [ModelCandidate],
        budget: &'a EscalationBudget,
        progress: bool,
    ) -> RoutingPlanInput<'a> {
        RoutingPlanInput {
            active,
            candidates,
            budget,
            meaningful_progress_observed: progress,
        }
    }

    fn transient_with_retry_after() -> FailureEvidence {
        let error =
            ModelError::rate_limited_with_retry_after("rate_limited", Some(Duration::from_secs(9)));
        classify_model_error(&error)
    }

    fn transient_without_hint() -> FailureEvidence {
        classify_model_error(&ModelError::Timeout)
    }

    fn permanent() -> FailureEvidence {
        classify_model_error(&ModelError::Configuration("model not found".into()))
    }

    fn capability() -> FailureEvidence {
        FailureEvidence {
            source: FailureSource::ProgressStall,
            class: FailureClass::ModelCapability,
            retryable: false,
            http_status: None,
            category: "repeated_decisions_no_progress".into(),
            detail: "stalled".into(),
            failed_action: Some("apply_correction".into()),
            signal: StructuredRecoverySignal::default(),
        }
    }

    fn stalled() -> FailureEvidence {
        FailureEvidence {
            source: FailureSource::ProgressStall,
            class: FailureClass::ConvergenceStalled,
            retryable: false,
            http_status: None,
            category: "unknown_stall".into(),
            detail: "no progress".into(),
            failed_action: None,
            signal: StructuredRecoverySignal::default(),
        }
    }

    #[test]
    fn a_external_transient_retry_after_waits_same_model() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let budget = EscalationBudget::new(3);
        let decision = plan_routing(
            &transient_with_retry_after(),
            input(&active, &candidates, &budget, false),
        );
        assert_eq!(decision.action, RoutingAction::WaitSameModel);
        assert_eq!(decision.reason, RoutingReason::ExternalTransientWait);
        assert!(decision.to.is_none());
        assert_eq!(decision.from.model_id, "cheap");
    }

    #[test]
    fn b_transient_without_hint_uses_backoff_same_model() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let budget = EscalationBudget::new(3);
        let decision = plan_routing(
            &transient_without_hint(),
            input(&active, &candidates, &budget, false),
        );
        assert_eq!(decision.action, RoutingAction::WaitSameModel);
        assert_eq!(decision.reason, RoutingReason::ExternalTransientBackoff);
    }

    #[test]
    fn c_external_permanent_routes_to_alternative() {
        let active = low("missing");
        let alt = medium("other");
        let candidates = vec![active.clone(), alt.clone()];
        let budget = EscalationBudget::new(2);
        let decision = plan_routing(&permanent(), input(&active, &candidates, &budget, false));
        assert_eq!(decision.action, RoutingAction::SwitchAlternative);
        assert_eq!(decision.reason, RoutingReason::ExternalPermanentSwitch);
        assert_eq!(
            decision.to.as_ref().map(|t| t.model_id.as_str()),
            Some("other")
        );
    }

    #[test]
    fn d_model_capability_allows_escalation() {
        let active = low("cheap");
        let candidates = vec![active.clone(), medium("mid"), high("strong")];
        let budget = EscalationBudget::new(2);
        let decision = plan_routing(&capability(), input(&active, &candidates, &budget, false));
        assert_eq!(decision.action, RoutingAction::EscalateCapability);
        assert_eq!(decision.reason, RoutingReason::ModelCapabilityEscalate);
        // Escala al menor tier superior (medium), no salta a high de golpe.
        assert_eq!(
            decision.to.as_ref().map(|t| t.model_id.as_str()),
            Some("mid")
        );
    }

    #[test]
    fn e_system_failure_never_escalates() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let budget = EscalationBudget::new(5);
        let evidence = classify_system_failure("wiring broken", None);
        let decision = plan_routing(&evidence, input(&active, &candidates, &budget, false));
        assert_eq!(decision.action, RoutingAction::Stop);
        assert_eq!(decision.reason, RoutingReason::SystemFailureNoEscalation);
        assert!(decision.to.is_none());
    }

    #[test]
    fn f_progress_observed_blocks_escalation() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let budget = EscalationBudget::new(2);
        let decision = plan_routing(&capability(), input(&active, &candidates, &budget, true));
        assert_eq!(decision.action, RoutingAction::Stay);
        assert_eq!(decision.reason, RoutingReason::ProgressNoEscalation);
    }

    #[test]
    fn g_non_progress_convergence_evaluated_with_evidence() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let budget = EscalationBudget::new(1);
        let decision = plan_routing(&stalled(), input(&active, &candidates, &budget, false));
        assert_eq!(decision.action, RoutingAction::EscalateCapability);
        assert_eq!(decision.reason, RoutingReason::ConvergenceEscalate);
        assert_eq!(
            decision.to.as_ref().map(|t| t.model_id.as_str()),
            Some("strong")
        );
    }

    #[test]
    fn h_escalation_budget_exhausted_stops() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let mut budget = EscalationBudget::new(1);
        assert!(budget.consume(&high("strong").identity()));
        let decision = plan_routing(&capability(), input(&active, &candidates, &budget, false));
        assert_eq!(decision.action, RoutingAction::Stop);
        assert_eq!(decision.reason, RoutingReason::EscalationBudgetExhausted);
    }

    #[test]
    fn i_routing_decision_is_observable() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let budget = EscalationBudget::new(2);
        let decision = plan_routing(&capability(), input(&active, &candidates, &budget, false));
        let summary = decision.summary();
        assert!(summary.contains("escalate_capability"));
        assert!(summary.contains("cheap"));
        assert!(summary.contains("strong"));
        assert!(summary.contains("model_capability"));
    }

    #[test]
    fn j_model_change_exposes_previous_and_next_identity() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let mut budget = EscalationBudget::new(2);
        budget.mark_visited(active.identity());
        let planned = plan_routing(&capability(), input(&active, &candidates, &budget, false));
        let mut index = 0;
        assert!(apply_routing_decision(
            &planned,
            &mut index,
            &candidates,
            &mut budget
        ));
        assert_eq!(index, 1);
        assert_eq!(planned.from.model_id, "cheap");
        assert_eq!(planned.to.as_ref().unwrap().model_id, "strong");
        assert!(budget.has_visited(&ModelIdentity::new("provider-a", "cheap")));
        assert!(budget.has_visited(&ModelIdentity::new("provider-b", "strong")));
    }

    #[test]
    fn k_infinite_alternation_impossible_by_budget_and_visited() {
        let cheap = low("cheap");
        let strong = high("strong");
        let candidates = vec![cheap.clone(), strong.clone()];
        let mut budget = EscalationBudget::new(2);
        budget.mark_visited(cheap.identity());

        let first = plan_routing(&capability(), input(&cheap, &candidates, &budget, false));
        let mut index = 0usize;
        assert!(apply_routing_decision(
            &first,
            &mut index,
            &candidates,
            &mut budget
        ));
        assert_eq!(index, 1);

        // Tras visitar strong, no hay escalación hacia abajo ni de vuelta a cheap.
        let second = plan_routing(&capability(), input(&strong, &candidates, &budget, false));
        assert_eq!(second.action, RoutingAction::Stop);
        assert!(matches!(
            second.reason,
            RoutingReason::ModelCapabilityNoHigherTier | RoutingReason::EscalationBudgetExhausted
        ));

        // Intentar planificar desde cheap otra vez: visited bloquea volver a strong sin budget.
        let third = plan_routing(&capability(), input(&cheap, &candidates, &budget, false));
        assert_eq!(third.action, RoutingAction::Stop);
    }

    #[test]
    fn l_retry_after_compatible_never_escalates() {
        let active = low("cheap");
        let candidates = vec![active.clone(), high("strong")];
        let budget = EscalationBudget::new(10);
        let decision = plan_routing(
            &transient_with_retry_after(),
            input(&active, &candidates, &budget, false),
        );
        assert_eq!(decision.action, RoutingAction::WaitSameModel);
        assert!(!decision.action.changes_model());
    }
}
