use crate::harness::action::AgentAction;
use crate::harness::context::AgentContext;
use crate::harness::correction::Correction;
use crate::harness::correction_policy::{
    CorrectionPolicy, CorrectionPolicyInput, DeterministicCorrectionPolicy,
};
use crate::harness::failure_classification::FailureEvidence;
use crate::harness::model_routing::RoutingDecision;
use crate::harness::observation::AgentObservation;

/// Productor de acciones. Permite reemplazar mocks por un AiAgent
/// sin modificar el [`crate::harness::Harness`] ni el [`crate::harness::AgentLoop`].
pub trait Agent: Send + Sync {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction;

    /// Evidencia estructurada del último fallo de servicio/modelo, si existe.
    ///
    /// Default: `None`. [`crate::harness::AiAgent`] expone [`ModelError`] /
    /// errores de respuesta sin acoplar el loop al proveedor.
    fn last_failure_evidence(&self) -> Option<FailureEvidence> {
        None
    }

    /// Planifica (y aplica si corresponde) routing multi-modelo tras un fallo clasificado.
    ///
    /// Default: `None` (agentes sin catálogo de candidatos). Un `Some` con
    /// `action.changes_model()` indica que el loop puede continuar con el nuevo modelo.
    fn try_route_after_failure(
        &mut self,
        evidence: &FailureEvidence,
        meaningful_progress_observed: bool,
    ) -> Option<RoutingDecision> {
        let _ = (evidence, meaningful_progress_observed);
        None
    }
}

/// Agente determinista con secuencia prefijada (útil para smoke tests).
///
/// Para causalidad observación→decisión usar los agentes basados en observación.
pub struct MockAgent {
    scripted: Vec<AgentAction>,
    index: usize,
}

impl MockAgent {
    pub fn new(scripted: Vec<AgentAction>) -> Self {
        Self { scripted, index: 0 }
    }
}

impl Agent for MockAgent {
    fn propose(&mut self, _ctx: &AgentContext) -> AgentAction {
        if self.index < self.scripted.len() {
            let action = self.scripted[self.index].clone();
            self.index += 1;
            action
        } else {
            AgentAction::Finish {
                summary: "mock agent exhausted script".to_string(),
            }
        }
    }
}

/// Acción permitida y luego Finish al observar éxito.
pub struct PermittedThenFinishAgent {
    code: String,
}

impl PermittedThenFinishAgent {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl Agent for PermittedThenFinishAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            None => AgentAction::Compile {
                code: self.code.clone(),
            },
            Some(AgentObservation::ToolOutcome { success: true, .. }) => AgentAction::Finish {
                summary: "compile ok".to_string(),
            },
            Some(other) => AgentAction::Finish {
                summary: format!("terminando tras observación: {}", other.summary()),
            },
        }
    }
}

/// Solicita una acción rechazable; al observar rechazo, hace Finish.
pub struct RejectedThenFinishAgent {
    unauthorized_tool: String,
}

impl RejectedThenFinishAgent {
    pub fn new(unauthorized_tool: impl Into<String>) -> Self {
        Self {
            unauthorized_tool: unauthorized_tool.into(),
        }
    }
}

impl Agent for RejectedThenFinishAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            None => AgentAction::InvokeTool {
                tool_name: self.unauthorized_tool.clone(),
                input: "blocked".to_string(),
            },
            Some(AgentObservation::ActionRejected { .. }) => AgentAction::Finish {
                summary: "continué tras rechazo".to_string(),
            },
            Some(other) => AgentAction::Finish {
                summary: format!("fin inesperado: {}", other.summary()),
            },
        }
    }
}

/// Observa FAIL de ValidationTool, solicita RepairDiagnostic, delega correcciones
/// en una [`CorrectionPolicy`] y revalida.
pub struct ValidateThenRepairAgent {
    request: String,
    plan_kind: String,
    invalid_code: String,
    policy: Box<dyn CorrectionPolicy>,
    pub proposed_corrections: Option<Vec<Correction>>,
}

impl ValidateThenRepairAgent {
    pub fn new(
        request: impl Into<String>,
        plan_kind: impl Into<String>,
        invalid_code: impl Into<String>,
    ) -> Self {
        Self::with_policy(
            request,
            plan_kind,
            invalid_code,
            Box::new(DeterministicCorrectionPolicy::new()),
        )
    }

    pub fn with_policy(
        request: impl Into<String>,
        plan_kind: impl Into<String>,
        invalid_code: impl Into<String>,
        policy: Box<dyn CorrectionPolicy>,
    ) -> Self {
        Self {
            request: request.into(),
            plan_kind: plan_kind.into(),
            invalid_code: invalid_code.into(),
            policy,
            proposed_corrections: None,
        }
    }
}

impl Agent for ValidateThenRepairAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            None => AgentAction::Validate {
                request: self.request.clone(),
                code: Some(self.invalid_code.clone()),
                plan_kind: self.plan_kind.clone(),
            },
            Some(
                obs @ AgentObservation::ToolOutcome {
                    tool_name,
                    success: false,
                    ..
                },
            ) if tool_name == crate::harness::tools::VALIDATE => {
                let errors = obs.validator_errors();
                if errors.is_empty() {
                    return AgentAction::Finish {
                        summary: "fail: validation without errors".to_string(),
                    };
                }
                if !obs.repairer_feedback().is_empty() {
                    return AgentAction::Finish {
                        summary: "fail: validation must not include repair feedback".to_string(),
                    };
                }
                AgentAction::RepairDiagnostic {
                    errors: errors.iter().map(|e| (*e).to_string()).collect(),
                }
            }
            Some(
                obs @ AgentObservation::ToolOutcome {
                    tool_name,
                    success: true,
                    ..
                },
            ) if tool_name == crate::harness::tools::REPAIR_DIAGNOSTIC => {
                let feedback = obs.repairer_feedback();
                if feedback.is_empty() {
                    return AgentAction::Finish {
                        summary: "fail: repair diagnostic without feedback".to_string(),
                    };
                }
                let input = CorrectionPolicyInput::new(obs, ctx);
                let corrections = match self.policy.propose_corrections(&input) {
                    Ok(items) => items,
                    Err(error) => {
                        return AgentAction::Finish {
                            summary: format!("policy error: {error}"),
                        };
                    }
                };
                self.proposed_corrections = Some(corrections.clone());
                AgentAction::ApplyCorrection { corrections }
            }
            Some(AgentObservation::ToolOutcome {
                tool_name,
                success: true,
                ..
            }) if tool_name == crate::harness::tools::APPLY_CORRECTION => {
                let corrected = ctx
                    .working_code()
                    .map(str::to_string)
                    .or_else(|| {
                        ctx.last_observation
                            .as_ref()
                            .and_then(|o| o.corrected_code().map(str::to_string))
                    })
                    .unwrap_or_default();
                AgentAction::Validate {
                    request: self.request.clone(),
                    code: Some(corrected),
                    plan_kind: self.plan_kind.clone(),
                }
            }
            Some(AgentObservation::ToolOutcome {
                tool_name,
                success: true,
                ..
            }) if tool_name == crate::harness::tools::VALIDATE => AgentAction::Finish {
                summary: "validación reparada".to_string(),
            },
            Some(other) => AgentAction::Finish {
                summary: format!("fin tras: {}", other.summary()),
            },
        }
    }
}

/// Nunca termina: demuestra corte por MAX_ITERATIONS.
pub struct NeverFinishAgent;

impl Agent for NeverFinishAgent {
    fn propose(&mut self, _ctx: &AgentContext) -> AgentAction {
        AgentAction::NoOp
    }
}

/// Compila código inválido y, al observar el fallo, termina con Failure.
pub struct FailThenStopAgent {
    code: String,
}

impl FailThenStopAgent {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl Agent for FailThenStopAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            None => AgentAction::Compile {
                code: self.code.clone(),
            },
            Some(AgentObservation::ToolOutcome { success: false, .. }) => AgentAction::Finish {
                summary: "fail: compile error".to_string(),
            },
            Some(other) => AgentAction::Finish {
                summary: format!("fail: unexpected {}", other.summary()),
            },
        }
    }
}

/// Consume la observación de echo para decidir el siguiente input (causalidad).
pub struct ObservationDrivenEchoAgent {
    pub saw_first_observation: bool,
    pub second_input: Option<String>,
}

impl ObservationDrivenEchoAgent {
    pub fn new() -> Self {
        Self {
            saw_first_observation: false,
            second_input: None,
        }
    }
}

impl Default for ObservationDrivenEchoAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent for ObservationDrivenEchoAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            None => AgentAction::InvokeTool {
                tool_name: "echo".to_string(),
                input: "first".to_string(),
            },
            Some(AgentObservation::ToolOutcome {
                success: true,
                output,
                ..
            }) if output == "first" => {
                self.saw_first_observation = true;
                let next = format!("after:{output}");
                self.second_input = Some(next.clone());
                AgentAction::InvokeTool {
                    tool_name: "echo".to_string(),
                    input: next,
                }
            }
            Some(AgentObservation::ToolOutcome {
                success: true,
                output,
                ..
            }) if self.second_input.as_deref() == Some(output.as_str()) => AgentAction::Finish {
                summary: "segunda acción basada en observación".to_string(),
            },
            Some(_) => AgentAction::Finish {
                summary: "cerrar".to_string(),
            },
        }
    }
}

/// Propone primero una acción inválida; tras [`AgentObservation::ActionRejected`] propone Compile.
///
/// La segunda decisión depende de la Observation de rechazo, no de `step`/`iteration`.
pub struct FirstActionAgent {
    pub code: String,
}

impl FirstActionAgent {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl Agent for FirstActionAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            Some(AgentObservation::ActionRejected { .. }) => AgentAction::Compile {
                code: self.code.clone(),
            },
            Some(AgentObservation::CriterionEvaluated {
                verdict: crate::harness::evaluation::EvaluationVerdict::Pass,
                ..
            }) => AgentAction::Finish {
                summary: "finish after evaluation pass".to_string(),
            },
            Some(AgentObservation::ToolOutcome {
                tool_name,
                success: true,
                ..
            }) if tool_name == crate::harness::tools::COMPILE => AgentAction::Finish {
                summary: "finish after compile tool".to_string(),
            },
            None => AgentAction::Finish {
                summary: "premature finish".to_string(),
            },
            Some(other) => AgentAction::Finish {
                summary: format!("stop after {}", other.summary()),
            },
        }
    }
}
