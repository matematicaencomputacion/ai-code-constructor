use crate::harness::action::AgentAction;
use crate::harness::constraint::{Constraint, ConstraintDecision};
use crate::harness::context::AgentContext;
use std::collections::BTreeSet;

/// Restricción que solo permite herramientas de una lista explícita.
pub struct ToolPermissionConstraint {
    allowed_tools: BTreeSet<String>,
}

impl ToolPermissionConstraint {
    pub fn new(allowed_tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed_tools: allowed_tools.into_iter().map(Into::into).collect(),
        }
    }

    pub fn default_constructor_tools() -> Self {
        Self::new([
            crate::harness::tools::COMPILE,
            crate::harness::tools::VALIDATE,
            crate::harness::tools::REPAIR_DIAGNOSTIC,
            crate::harness::tools::APPLY_CORRECTION,
            crate::harness::tools::RUN_TESTS,
            crate::harness::tools::RUN_CLIPPY,
            crate::harness::tools::CHECK_FORMAT,
        ])
    }
}

impl Constraint for ToolPermissionConstraint {
    fn name(&self) -> &str {
        "tool_permission"
    }

    fn check(&self, action: &AgentAction, _ctx: &AgentContext) -> ConstraintDecision {
        match action.tool_name() {
            None => ConstraintDecision::Allow,
            Some(tool_name) if self.allowed_tools.contains(tool_name) => ConstraintDecision::Allow,
            Some(tool_name) => ConstraintDecision::Reject {
                reason: format!("herramienta no autorizada: {tool_name}"),
            },
        }
    }
}
