use crate::harness::action::AgentAction;
use crate::harness::context::AgentContext;

/// Decisión de una restricción sobre una acción propuesta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintDecision {
    Allow,
    Reject { reason: String },
}

/// Restricción que puede permitir o rechazar una [`AgentAction`].
pub trait Constraint: Send + Sync {
    fn name(&self) -> &str;

    fn check(&self, action: &AgentAction, ctx: &AgentContext) -> ConstraintDecision;
}
