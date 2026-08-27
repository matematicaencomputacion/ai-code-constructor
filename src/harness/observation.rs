use crate::harness::action::AgentAction;
use crate::harness::criterion::CriterionKind;
use crate::harness::evaluation::{EvaluationVerdict, Evidence};
use crate::harness::evaluation_engine::{CriterionEvaluation, SpecificationEvaluationStatus};
use crate::harness::specification::{AcceptanceCriterionId, SpecificationId};
use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC, VALIDATE};

/// Observación estructurada entregada al Agent tras un paso del Harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentObservation {
    /// Resultado de una Tool ejecutada (éxito o fallo).
    ToolOutcome {
        tool_name: String,
        success: bool,
        output: String,
        evidence: Vec<Evidence>,
        verdict: EvaluationVerdict,
    },
    /// Resultado de evaluar un AcceptanceCriterion (VERIFY → CONTEXT).
    CriterionEvaluated {
        specification_id: SpecificationId,
        criterion_id: AcceptanceCriterionId,
        kind: CriterionKind,
        verdict: EvaluationVerdict,
        message: String,
        evidence: Vec<Evidence>,
    },
    /// Resultado agregado de evaluar una Specification completa.
    SpecificationEvaluated {
        specification_id: SpecificationId,
        status: SpecificationEvaluationStatus,
        message: String,
        criteria: Vec<CriterionEvaluation>,
    },
    /// La acción fue rechazada por una Constraint (la Tool no se ejecutó).
    ActionRejected {
        action: AgentAction,
        reason: String,
        /// Nombre de la constraint que rechazó (p. ej. `artifact_state`, `finish`).
        constraint: String,
    },
    /// Se ejecutó un NoOp.
    NoOpDone,
    /// El Agent solicitó Finish.
    Finished { summary: String },
    /// Se pidió una herramienta no registrada.
    UnknownTool { tool_name: String },
}

impl AgentObservation {
    pub fn is_success(&self) -> bool {
        match self {
            AgentObservation::ToolOutcome { success, .. } => *success,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Pass,
                ..
            } => true,
            AgentObservation::SpecificationEvaluated {
                status: SpecificationEvaluationStatus::Pass,
                ..
            } => true,
            AgentObservation::Finished { .. } | AgentObservation::NoOpDone => true,
            AgentObservation::CriterionEvaluated { .. }
            | AgentObservation::SpecificationEvaluated { .. }
            | AgentObservation::ActionRejected { .. }
            | AgentObservation::UnknownTool { .. } => false,
        }
    }

    pub fn is_failure(&self) -> bool {
        !self.is_success()
    }

    pub fn is_evaluation_pass(&self) -> bool {
        matches!(
            self,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Pass,
                ..
            } | AgentObservation::SpecificationEvaluated {
                status: SpecificationEvaluationStatus::Pass,
                ..
            }
        )
    }

    pub fn is_evaluation_fail(&self) -> bool {
        matches!(
            self,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Fail,
                ..
            } | AgentObservation::SpecificationEvaluated {
                status: SpecificationEvaluationStatus::Fail,
                ..
            }
        )
    }

    pub fn is_insufficient_evidence(&self) -> bool {
        matches!(
            self,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::InsufficientEvidence,
                ..
            } | AgentObservation::SpecificationEvaluated {
                status: SpecificationEvaluationStatus::InsufficientEvidence,
                ..
            }
        )
    }

    pub fn evaluation_verdict(&self) -> Option<EvaluationVerdict> {
        match self {
            AgentObservation::CriterionEvaluated { verdict, .. } => Some(*verdict),
            AgentObservation::SpecificationEvaluated { status, .. } => Some(match status {
                SpecificationEvaluationStatus::Pass => EvaluationVerdict::Pass,
                SpecificationEvaluationStatus::Fail => EvaluationVerdict::Fail,
                SpecificationEvaluationStatus::InsufficientEvidence => {
                    EvaluationVerdict::InsufficientEvidence
                }
            }),
            _ => None,
        }
    }

    pub fn evaluation_kind(&self) -> Option<CriterionKind> {
        match self {
            AgentObservation::CriterionEvaluated { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    pub fn specification_id(&self) -> Option<&SpecificationId> {
        match self {
            AgentObservation::CriterionEvaluated {
                specification_id, ..
            }
            | AgentObservation::SpecificationEvaluated {
                specification_id, ..
            } => Some(specification_id),
            _ => None,
        }
    }

    pub fn tool_name(&self) -> Option<&str> {
        match self {
            AgentObservation::ToolOutcome { tool_name, .. } => Some(tool_name.as_str()),
            _ => None,
        }
    }

    pub fn is_validation_outcome(&self) -> bool {
        self.tool_name() == Some(VALIDATE)
    }

    pub fn is_repair_diagnostic_outcome(&self) -> bool {
        self.tool_name() == Some(REPAIR_DIAGNOSTIC)
    }

    pub fn is_correction_outcome(&self) -> bool {
        self.tool_name() == Some(APPLY_CORRECTION)
    }

    pub fn is_compile_outcome(&self) -> bool {
        self.tool_name() == Some(COMPILE)
    }

    /// Errores del Validator (`validator_error_*`).
    pub fn validator_errors(&self) -> Vec<&str> {
        match self {
            AgentObservation::ToolOutcome { evidence, .. }
            | AgentObservation::CriterionEvaluated { evidence, .. } => evidence
                .iter()
                .filter(|item| item.label.starts_with("validator_error_"))
                .map(|item| item.detail.as_str())
                .collect(),
            AgentObservation::SpecificationEvaluated { criteria, .. } => criteria
                .iter()
                .flat_map(|item| item.evidence_used.iter())
                .filter(|item| item.label.starts_with("validator_error_"))
                .map(|item| item.detail.as_str())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Feedback diagnóstico del Repairer (`repairer_feedback_*`).
    pub fn repairer_feedback(&self) -> Vec<&str> {
        match self {
            AgentObservation::ToolOutcome { evidence, .. } => evidence
                .iter()
                .filter(|item| item.label.starts_with("repairer_feedback_"))
                .map(|item| item.detail.as_str())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Código corregido por CorrectionTool (`corrected_code`).
    pub fn corrected_code(&self) -> Option<&str> {
        match self {
            AgentObservation::ToolOutcome { evidence, .. } => evidence
                .iter()
                .find(|item| item.label == "corrected_code")
                .map(|item| item.detail.as_str()),
            _ => None,
        }
    }

    /// Evidencia de compilación (`compiler_stderr`, `compile_status`).
    pub fn compile_evidence(&self) -> Vec<&Evidence> {
        match self {
            AgentObservation::ToolOutcome { evidence, .. }
            | AgentObservation::CriterionEvaluated { evidence, .. } => evidence
                .iter()
                .filter(|item| {
                    item.label == "compile_status"
                        || item.label == "compiler_stderr"
                        || item.label == "code_bytes"
                })
                .collect(),
            AgentObservation::SpecificationEvaluated { criteria, .. } => criteria
                .iter()
                .flat_map(|item| item.evidence_used.iter())
                .filter(|item| {
                    item.label == "compile_status"
                        || item.label == "compiler_stderr"
                        || item.label == "code_bytes"
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn summary(&self) -> String {
        match self {
            AgentObservation::ToolOutcome {
                tool_name,
                success,
                output,
                ..
            } => format!(
                "tool:{tool_name}:{}:{output}",
                if *success { "ok" } else { "err" }
            ),
            AgentObservation::CriterionEvaluated {
                specification_id,
                criterion_id,
                kind,
                verdict,
                message,
                ..
            } => format!(
                "criterion_eval:spec={}:criterion={}:kind={kind}:verdict={verdict:?}:{message}",
                specification_id.as_str(),
                criterion_id.as_str(),
            ),
            AgentObservation::SpecificationEvaluated {
                specification_id,
                status,
                message,
                criteria,
                ..
            } => format!(
                "spec_eval:spec={}:status={status:?}:criteria={}:{message}",
                specification_id.as_str(),
                criteria.len(),
            ),
            AgentObservation::ActionRejected {
                reason, constraint, ..
            } => {
                format!("rejected:{constraint}:{reason}")
            }
            AgentObservation::NoOpDone => "noop".to_string(),
            AgentObservation::Finished { summary } => format!("finished:{summary}"),
            AgentObservation::UnknownTool { tool_name } => {
                format!("unknown_tool:{tool_name}")
            }
        }
    }
}
