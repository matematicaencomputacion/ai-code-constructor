//! Políticas de validez de [`AgentAction`] (distintas de permisos de Tool).
//!
//! - [`ToolPermissionConstraint`]: ¿la Tool está autorizada?
//! - Action constraints: ¿la acción es válida en el estado actual?
//! - [`ActionPolicy`]: composición con short-circuit.
//!
//! No ejecutan Tools, no mutan Context, no llaman LLM ni filesystem.

use crate::harness::action::AgentAction;
use crate::harness::constraint::{Constraint, ConstraintDecision};
use crate::harness::context::AgentContext;
use crate::harness::correction::{CorrectionOperation, CorrectionTarget};
use crate::harness::evaluation::EvaluationVerdict;
use crate::harness::observation::AgentObservation;
use crate::harness::tool_permission::ToolPermissionConstraint;

/// Contrato explícito de validez/permiso de acción (alias del trait [`Constraint`]).
///
/// Recibe [`AgentAction`] + [`AgentContext`] → Allow | Reject(reason).
pub trait ActionConstraint: Constraint {}

impl<T: Constraint> ActionConstraint for T {}

/// Requiere Artifact canónico con source no vacío para acciones que mutan/verifican código.
pub struct ArtifactStateConstraint;

impl Constraint for ArtifactStateConstraint {
    fn name(&self) -> &str {
        "artifact_state"
    }

    fn check(&self, action: &AgentAction, ctx: &AgentContext) -> ConstraintDecision {
        let needs_artifact = matches!(
            action,
            AgentAction::Compile { .. }
                | AgentAction::Validate { .. }
                | AgentAction::ApplyCorrection { .. }
        );
        if !needs_artifact {
            return ConstraintDecision::Allow;
        }
        match ctx.working_artifact.as_ref() {
            Some(artifact) if !artifact.source().trim().is_empty() => ConstraintDecision::Allow,
            Some(_) => ConstraintDecision::Reject {
                reason: "working_artifact sin source".to_string(),
            },
            None => ConstraintDecision::Reject {
                reason: "working_artifact ausente".to_string(),
            },
        }
    }
}

/// RepairDiagnostic solo si hay errores diagnósticos concretos.
pub struct RepairDiagnosticConstraint;

impl Constraint for RepairDiagnosticConstraint {
    fn name(&self) -> &str {
        "repair_diagnostic"
    }

    fn check(&self, action: &AgentAction, _ctx: &AgentContext) -> ConstraintDecision {
        match action {
            AgentAction::RepairDiagnostic { errors } => {
                let usable: Vec<&str> = errors
                    .iter()
                    .map(|item| item.trim())
                    .filter(|item| !item.is_empty())
                    .collect();
                if usable.is_empty() {
                    ConstraintDecision::Reject {
                        reason: "RepairDiagnostic sin errores relevantes".to_string(),
                    }
                } else {
                    ConstraintDecision::Allow
                }
            }
            _ => ConstraintDecision::Allow,
        }
    }
}

/// Validez estructural de ApplyCorrection (además de Artifact).
pub struct ApplyCorrectionConstraint;

impl Constraint for ApplyCorrectionConstraint {
    fn name(&self) -> &str {
        "apply_correction"
    }

    fn check(&self, action: &AgentAction, ctx: &AgentContext) -> ConstraintDecision {
        let AgentAction::ApplyCorrection { corrections } = action else {
            return ConstraintDecision::Allow;
        };

        if corrections.is_empty() {
            return ConstraintDecision::Reject {
                reason: "ApplyCorrection sin operaciones".to_string(),
            };
        }

        let Some(artifact) = ctx.working_artifact.as_ref() else {
            return ConstraintDecision::Reject {
                reason: "working_artifact ausente".to_string(),
            };
        };

        for correction in corrections {
            if correction.target != CorrectionTarget::SessionCode {
                return ConstraintDecision::Reject {
                    reason: "target de corrección no autorizado".to_string(),
                };
            }

            let path = correction.resolved_path(artifact);
            let Some(code) = artifact.file(path) else {
                return ConstraintDecision::Reject {
                    reason: format!("archivo de corrección inexistente: {}", path.as_str()),
                };
            };

            match &correction.operation {
                CorrectionOperation::ReplaceText { search, .. } if search.is_empty() => {
                    return ConstraintDecision::Reject {
                        reason: "ReplaceText con search vacío".to_string(),
                    };
                }
                CorrectionOperation::InsertText { text, .. } if text.is_empty() => {
                    return ConstraintDecision::Reject {
                        reason: "InsertText con text vacío".to_string(),
                    };
                }
                CorrectionOperation::RemoveText { start, end } if start >= end => {
                    return ConstraintDecision::Reject {
                        reason: "RemoveText con rango inválido".to_string(),
                    };
                }
                CorrectionOperation::RemoveText { end, .. } => {
                    if *end > code.len() {
                        return ConstraintDecision::Reject {
                            reason: format!("RemoveText fuera de rango en {}", path.as_str()),
                        };
                    }
                }
                CorrectionOperation::InsertText { position, .. } => {
                    if *position > code.len() {
                        return ConstraintDecision::Reject {
                            reason: format!(
                                "InsertText position fuera de rango en {}",
                                path.as_str()
                            ),
                        };
                    }
                }
                CorrectionOperation::ReplaceText { search, .. } => {
                    if !code.contains(search.as_str()) {
                        return ConstraintDecision::Reject {
                            reason: format!(
                                "ReplaceText search no encontrado en {}",
                                path.as_str()
                            ),
                        };
                    }
                }
            }
        }

        ConstraintDecision::Allow
    }
}

/// Finish solo cuando la Specification (si existe) tiene criterios en PASS.
///
/// Sin `evaluation_specification`, Allow (política de finish no configurada).
/// `InsufficientEvidence` y `Fail` nunca autorizan Finish.
pub struct FinishConstraint;

impl Constraint for FinishConstraint {
    fn name(&self) -> &str {
        "finish"
    }

    fn check(&self, action: &AgentAction, ctx: &AgentContext) -> ConstraintDecision {
        let AgentAction::Finish { .. } = action else {
            return ConstraintDecision::Allow;
        };

        let Some(specification) = ctx.evaluation_specification.as_ref() else {
            return ConstraintDecision::Allow;
        };

        if specification.acceptance_criteria.is_empty() {
            return ConstraintDecision::Reject {
                reason: "Finish sin AcceptanceCriteria configurados".to_string(),
            };
        }

        for criterion in &specification.acceptance_criteria {
            let verdict = ctx
                .observation_history
                .iter()
                .rev()
                .find_map(|obs| match obs {
                    AgentObservation::CriterionEvaluated {
                        criterion_id,
                        verdict,
                        ..
                    } if criterion_id == &criterion.id => Some(*verdict),
                    _ => None,
                });

            match verdict {
                Some(EvaluationVerdict::Pass) => {}
                Some(EvaluationVerdict::Fail) => {
                    return ConstraintDecision::Reject {
                        reason: format!(
                            "Finish bloqueado: criterio `{}` en FAIL",
                            criterion.id.as_str()
                        ),
                    };
                }
                Some(EvaluationVerdict::InsufficientEvidence) => {
                    return ConstraintDecision::Reject {
                        reason: format!(
                            "Finish bloqueado: criterio `{}` con InsufficientEvidence (no es PASS)",
                            criterion.id.as_str()
                        ),
                    };
                }
                None => {
                    return ConstraintDecision::Reject {
                        reason: format!(
                            "Finish bloqueado: evidencia insuficiente para criterio `{}`",
                            criterion.id.as_str()
                        ),
                    };
                }
            }
        }

        ConstraintDecision::Allow
    }
}

/// Composición de constraints con short-circuit (la primera Reject detiene el resto).
pub struct ActionPolicy {
    constraints: Vec<Box<dyn Constraint>>,
}

impl ActionPolicy {
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
        }
    }

    pub fn with_constraint(mut self, constraint: Box<dyn Constraint>) -> Self {
        self.constraints.push(constraint);
        self
    }

    /// Política de sesión: permission + validez de Artifact/Repair/Correction/Finish.
    pub fn default_session_policy() -> Self {
        Self::new()
            .with_constraint(Box::new(
                ToolPermissionConstraint::default_constructor_tools(),
            ))
            .with_constraint(Box::new(ArtifactStateConstraint))
            .with_constraint(Box::new(RepairDiagnosticConstraint))
            .with_constraint(Box::new(ApplyCorrectionConstraint))
            .with_constraint(Box::new(FinishConstraint))
    }

    pub fn constraints(&self) -> &[Box<dyn Constraint>] {
        &self.constraints
    }

    /// Evalúa constraints en orden; short-circuit en el primer Reject.
    pub fn decide(&self, action: &AgentAction, ctx: &AgentContext) -> PolicyVerdict {
        for constraint in &self.constraints {
            match constraint.check(action, ctx) {
                ConstraintDecision::Allow => {}
                ConstraintDecision::Reject { reason } => {
                    return PolicyVerdict::Reject {
                        constraint: constraint.name().to_string(),
                        reason,
                    };
                }
            }
        }
        PolicyVerdict::Allow
    }
}

impl Default for ActionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl Constraint for ActionPolicy {
    fn name(&self) -> &str {
        "action_policy"
    }

    fn check(&self, action: &AgentAction, ctx: &AgentContext) -> ConstraintDecision {
        match self.decide(action, ctx) {
            PolicyVerdict::Allow => ConstraintDecision::Allow,
            PolicyVerdict::Reject { constraint, reason } => ConstraintDecision::Reject {
                reason: format!("{constraint}: {reason}"),
            },
        }
    }
}

/// Resultado de evaluar la política con identidad de la constraint que rechazó.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    Allow,
    Reject { constraint: String, reason: String },
}
