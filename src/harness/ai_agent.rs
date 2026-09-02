use crate::harness::action::AgentAction;
use crate::harness::agent::Agent;
use crate::harness::artifact_file_operation::ArtifactFileOperation;
use crate::harness::context::AgentContext;
use crate::harness::correction::Correction;
use crate::harness::failure_classification::{
    FailureEvidence, classify_model_error, classify_response_error,
};
use crate::harness::feature_flags::ai_agent_gap_guidance_enabled;
use crate::harness::model::{
    AiSessionConfig, ModelClient, ModelDecision, ModelError, ModelInteractionTrace,
    ModelResponseError, model_request_from_context, parse_model_response, structured_to_correction,
    structured_to_file_operation, validate_apply_correction,
    validate_model_decision_against_recommendation,
};
use crate::harness::model_routing::{
    EscalationBudget, ModelCandidate, RoutingDecision, RoutingPlanInput, apply_routing_decision,
    plan_routing,
};

/// Estado de routing multi-modelo opcional (catálogo + presupuesto + historial).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoutingState {
    pub candidates: Vec<ModelCandidate>,
    pub active_index: usize,
    pub budget: EscalationBudget,
    pub decisions: Vec<RoutingDecision>,
}

/// Primer Agent basado en IA: serializa contexto, consulta [`ModelClient`]
/// y convierte la respuesta validada en [`AgentAction`].
///
/// No ejecuta Tools ni conoce Harness directamente.
pub struct AiAgent {
    clients: Vec<Box<dyn ModelClient>>,
    session: AiSessionConfig,
    routing: Option<ModelRoutingState>,
    pub trace: ModelInteractionTrace,
    pub last_model_error: Option<ModelError>,
    pub last_response_error: Option<ModelResponseError>,
}

impl AiAgent {
    pub fn new(client: Box<dyn ModelClient>, session: AiSessionConfig) -> Self {
        Self {
            clients: vec![client],
            session,
            routing: None,
            trace: ModelInteractionTrace::default(),
            last_model_error: None,
            last_response_error: None,
        }
    }

    /// Construye un agente con catálogo de candidatos y presupuesto de escalación.
    ///
    /// El primer elemento es el modelo activo inicial. Los ids/providers vienen de
    /// configuración inyectada (tests/orquestador), no de política hardcodeada.
    pub fn with_model_routing(
        catalog: Vec<(ModelCandidate, Box<dyn ModelClient>)>,
        session: AiSessionConfig,
        budget: EscalationBudget,
    ) -> Self {
        assert!(
            !catalog.is_empty(),
            "with_model_routing requiere al menos un candidato"
        );
        let mut candidates = Vec::with_capacity(catalog.len());
        let mut clients = Vec::with_capacity(catalog.len());
        for (candidate, client) in catalog {
            candidates.push(candidate);
            clients.push(client);
        }
        let mut budget = budget;
        budget.mark_visited(candidates[0].identity());
        Self {
            clients,
            session,
            routing: Some(ModelRoutingState {
                candidates,
                active_index: 0,
                budget,
                decisions: Vec::new(),
            }),
            trace: ModelInteractionTrace::default(),
            last_model_error: None,
            last_response_error: None,
        }
    }

    pub fn routing_state(&self) -> Option<&ModelRoutingState> {
        self.routing.as_ref()
    }

    pub fn active_model_candidate(&self) -> Option<&ModelCandidate> {
        let state = self.routing.as_ref()?;
        state.candidates.get(state.active_index)
    }

    fn active_client(&self) -> &dyn ModelClient {
        let index = self
            .routing
            .as_ref()
            .map(|state| state.active_index)
            .unwrap_or(0);
        self.clients[index].as_ref()
    }

    fn action_label(action: &AgentAction) -> String {
        match action {
            AgentAction::Validate { .. } => "validate".to_string(),
            AgentAction::RepairDiagnostic { .. } => "repair_diagnostic".to_string(),
            AgentAction::ApplyCorrection { .. } => "apply_correction".to_string(),
            AgentAction::ApplyFileOperations { .. } => "apply_file_operations".to_string(),
            AgentAction::Compile { .. } => "compile".to_string(),
            AgentAction::Finish { .. } => "finish".to_string(),
            AgentAction::RunTests { .. } => "run_tests".to_string(),
            AgentAction::RunClippy => "run_clippy".to_string(),
            AgentAction::CheckFormat => "check_format".to_string(),
            AgentAction::InvokeTool { tool_name, .. } => format!("invoke:{tool_name}"),
            AgentAction::NoOp => "noop".to_string(),
        }
    }

    fn decision_to_action(
        decision: ModelDecision,
        ctx: &AgentContext,
    ) -> Result<AgentAction, ModelResponseError> {
        match decision {
            ModelDecision::Validate {
                request,
                code,
                plan_kind,
            } => {
                let resolved_code = code.or_else(|| ctx.working_code().map(str::to_string));
                Ok(AgentAction::Validate {
                    request,
                    code: resolved_code,
                    plan_kind,
                })
            }
            ModelDecision::RepairDiagnostic { errors } => {
                Ok(AgentAction::RepairDiagnostic { errors })
            }
            ModelDecision::ApplyCorrection { corrections } => {
                validate_apply_correction(&corrections, ctx.working_artifact.as_ref())?;
                let mapped = corrections
                    .iter()
                    .map(structured_to_correction)
                    .collect::<Result<Vec<Correction>, ModelResponseError>>()?;
                Ok(AgentAction::ApplyCorrection {
                    corrections: mapped,
                })
            }
            ModelDecision::ApplyFileOperations { operations } => {
                let mapped = operations
                    .iter()
                    .map(structured_to_file_operation)
                    .collect::<Result<Vec<ArtifactFileOperation>, ModelResponseError>>()?;
                Ok(AgentAction::ApplyFileOperations { operations: mapped })
            }
            ModelDecision::Compile { code } => Ok(AgentAction::Compile { code }),
            ModelDecision::RunTests { filter } => Ok(AgentAction::RunTests { filter }),
            ModelDecision::RunClippy => Ok(AgentAction::RunClippy),
            ModelDecision::CheckFormat => Ok(AgentAction::CheckFormat),
            ModelDecision::Finish { summary } => Ok(AgentAction::Finish { summary }),
        }
    }

    fn finish_with_error(message: impl Into<String>) -> AgentAction {
        AgentAction::Finish {
            summary: format!("ai error: {}", message.into()),
        }
    }
}

impl Agent for AiAgent {
    fn last_failure_evidence(&self) -> Option<FailureEvidence> {
        if let Some(error) = &self.last_model_error {
            return Some(classify_model_error(error));
        }
        self.last_response_error
            .as_ref()
            .map(classify_response_error)
    }

    fn plan_route_after_failure(
        &self,
        evidence: &FailureEvidence,
        recent_progress_observed: bool,
    ) -> Option<RoutingDecision> {
        let state = self.routing.as_ref()?;
        let active = state.candidates.get(state.active_index)?.clone();
        Some(plan_routing(
            evidence,
            RoutingPlanInput {
                active: &active,
                candidates: &state.candidates,
                budget: &state.budget,
                meaningful_progress_observed: recent_progress_observed,
            },
        ))
    }

    fn apply_route_after_failure(&mut self, planned: RoutingDecision) -> RoutingDecision {
        let Some(state) = self.routing.as_mut() else {
            return planned;
        };
        if planned.action.changes_model() {
            let applied = apply_routing_decision(
                &planned,
                &mut state.active_index,
                &state.candidates,
                &mut state.budget,
            );
            if !applied {
                let stopped = RoutingDecision {
                    action: crate::harness::model_routing::RoutingAction::Stop,
                    reason: crate::harness::model_routing::RoutingReason::NoRouteableCandidates,
                    from: planned.from.clone(),
                    to: planned.to.clone(),
                    failure_class: planned.failure_class,
                    escalation_used: state.budget.switches_used,
                    escalation_remaining: state.budget.remaining_count(),
                };
                state.decisions.push(stopped.clone());
                return stopped;
            }
        }
        // Refrescar contadores post-apply para observabilidad exacta.
        let mut recorded = planned;
        recorded.escalation_used = state.budget.switches_used;
        recorded.escalation_remaining = state.budget.remaining_count();
        state.decisions.push(recorded.clone());
        recorded
    }

    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        self.last_model_error = None;
        self.last_response_error = None;

        let request = match model_request_from_context(ctx, &self.session) {
            Ok(value) => value,
            Err(error) => {
                self.last_response_error = Some(error.clone());
                self.trace.record_action_label(Err(error.clone()));
                return Self::finish_with_error(error.to_string());
            }
        };
        self.trace.record_request(request.clone());

        let response = match self.active_client().complete(&request) {
            Ok(value) => value,
            Err(error) => {
                self.last_model_error = Some(error.clone());
                self.trace
                    .record_action_label(Err(ModelResponseError::InvalidModelResponse(
                        error.to_string(),
                    )));
                return Self::finish_with_error(error.to_string());
            }
        };
        self.trace.record_response(response.clone());

        let decision = match parse_model_response(&response.raw_text) {
            Ok(value) => value,
            Err(error) => {
                self.last_response_error = Some(error.clone());
                self.trace.record_decision(Err(error.clone()));
                self.trace.record_action_label(Err(error.clone()));
                return Self::finish_with_error(error.to_string());
            }
        };
        let decision = if ai_agent_gap_guidance_enabled(self.session.gap_guidance) {
            validate_model_decision_against_recommendation(decision, &request)
        } else {
            decision
        };
        self.trace.record_decision(Ok(decision.clone()));

        let action = match Self::decision_to_action(decision, ctx) {
            Ok(value) => value,
            Err(error) => {
                self.last_response_error = Some(error.clone());
                self.trace.record_action_label(Err(error.clone()));
                return Self::finish_with_error(error.to_string());
            }
        };

        self.trace
            .record_action_label(Ok(Self::action_label(&action)));
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::agent_loop::{AgentLoop, LoopStatus};
    use crate::harness::bridge::introduce_validation_defect;
    use crate::harness::model::{
        MockModelClient, ModelDecision, StructuredCorrection, serialize_decision,
    };
    use crate::harness::runtime::Harness;
    use crate::harness::tool_permission::ToolPermissionConstraint;
    use crate::harness::tools::{
        APPLY_CORRECTION, COMPILE, CompileTool, CorrectionTool, REPAIR_DIAGNOSTIC,
        RepairDiagnosticTool, VALIDATE, ValidationTool,
    };

    fn api_valid_code() -> String {
        r#"fn main() {
    crear_servidor();
    definir_endpoints();
    implementar_handlers();
}

fn crear_servidor() {
    println!("Servidor HTTP configurado");
}

fn definir_endpoints() {
    println!("Endpoints definidos");
}

fn implementar_handlers() {
    println!("Handlers implementados");
}
"#
        .to_string()
    }

    struct ScriptModelClient {
        responses: Vec<String>,
    }

    impl ScriptModelClient {
        fn new(responses: Vec<String>) -> Self {
            Self { responses }
        }
    }

    impl ModelClient for ScriptModelClient {
        fn complete(
            &self,
            request: &crate::harness::model::ModelRequest,
        ) -> Result<crate::harness::model::ModelResponse, ModelError> {
            let index = request.step.saturating_sub(1) as usize;
            let raw_text =
                self.responses.get(index).cloned().unwrap_or_else(|| {
                    "{\"action\":\"finish\",\"summary\":\"exhausted\"}".to_string()
                });
            Ok(crate::harness::model::ModelResponse { raw_text })
        }
    }

    #[test]
    fn ai_agent_converts_valid_response_to_action() {
        let decision = ModelDecision::RepairDiagnostic {
            errors: vec!["error".to_string()],
        };
        let client = ScriptModelClient::new(vec![serialize_decision(&decision)]);
        let session = AiSessionConfig::new("r".to_string(), "Api".to_string());
        let mut agent = AiAgent::new(Box::new(client), session);
        let mut ctx = AgentContext::new("ai");
        ctx.step = 1;
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::RepairDiagnostic { .. }));
    }

    #[test]
    fn ai_agent_rejects_invalid_response_without_tool_execution() {
        let client = MockModelClient::invalid();
        let session = AiSessionConfig::new("r".to_string(), "Api".to_string());
        let mut agent = AiAgent::new(Box::new(client), session);
        let action = agent.propose(&AgentContext::new("ai"));
        assert!(matches!(action, AgentAction::Finish { .. }));
        assert!(agent.last_response_error.is_some());
        assert_eq!(agent.trace.responses.len(), 1);
    }

    #[test]
    fn ai_agent_apply_correction_is_structured_not_full_code() {
        let decision = ModelDecision::ApplyCorrection {
            corrections: vec![StructuredCorrection::ReplaceText {
                path: None,
                search: "NET".to_string(),
                replacement: "HTTP".to_string(),
            }],
        };
        let client = ScriptModelClient::new(vec![serialize_decision(&decision)]);
        let session = AiSessionConfig::new("r".to_string(), "Api".to_string());
        let mut agent = AiAgent::new(Box::new(client), session);
        let mut ctx = AgentContext::new("ai").with_working_code("Servidor NET");
        ctx.step = 1;
        let action = agent.propose(&ctx);
        match action {
            AgentAction::ApplyCorrection { corrections } => {
                assert_eq!(corrections.len(), 1);
                assert!(matches!(
                    corrections[0].operation,
                    crate::harness::CorrectionOperation::ReplaceText { .. }
                ));
            }
            other => panic!("expected ApplyCorrection, got {other:?}"),
        }
    }

    #[test]
    fn ai_agent_e2e_loop_with_mock_model_client() {
        let invalid = introduce_validation_defect(&api_valid_code());
        let mut harness = Harness::new(12);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(crate::harness::tools::CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());
        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let ctx = AgentContext::new("ai-e2e").with_working_code(invalid);

        let result = AgentLoop::new(10).run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(result.tools_executed().iter().any(|t| t == VALIDATE));
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == REPAIR_DIAGNOSTIC)
        );
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == APPLY_CORRECTION)
        );
        assert!(result.tools_executed().iter().any(|t| t == COMPILE));
        assert!(agent.trace.requests.len() >= 5);
        assert!(agent.trace.parsed_decisions.iter().all(|item| item.is_ok()));
    }

    #[test]
    fn ai_agent_invalid_response_does_not_execute_tools_in_loop() {
        let mut harness = Harness::new(5);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());
        let mut agent = AiAgent::new(Box::new(MockModelClient::invalid()), session);
        let result = AgentLoop::new(3).run(
            &harness,
            &mut agent,
            AgentContext::new("invalid-ai").with_working_code("fn main() {}"),
        );

        assert!(result.tools_executed().is_empty());
        assert!(matches!(
            result.history.proposed_actions[0],
            AgentAction::Finish { .. }
        ));
    }

    #[test]
    fn ai_agent_populates_goal_context_from_specification() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-ai-gap", "compilar")
            .with_requirements(vec![Requirement::new("req", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req")]),
            ]);
        let client = MockModelClient::new();
        let session = AiSessionConfig::new("compilar".to_string(), "Generic".to_string());
        let mut agent = AiAgent::new(Box::new(client), session);
        let ctx = AgentContext::new("ai-gap")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);
        let _ = agent.propose(&ctx);
        let request = agent.trace.requests.last().expect("request");
        assert!(request.goal_evaluation.is_some());
        assert!(request.goal_gap.is_some());
        assert_eq!(
            agent
                .trace
                .parsed_decisions
                .last()
                .unwrap()
                .as_ref()
                .unwrap(),
            &ModelDecision::Compile {
                code: "fn main() {}".to_string(),
            }
        );
    }

    struct AlwaysValidateModelClient;

    impl ModelClient for AlwaysValidateModelClient {
        fn complete(
            &self,
            _request: &crate::harness::model::ModelRequest,
        ) -> Result<crate::harness::model::ModelResponse, ModelError> {
            Ok(crate::harness::model::ModelResponse {
                raw_text: serialize_decision(&ModelDecision::Validate {
                    request: "compilar".to_string(),
                    code: Some("fn main() {}".to_string()),
                    plan_kind: "Generic".to_string(),
                }),
            })
        }
    }

    #[test]
    fn ai_agent_recommended_action_redirects_incompatible_validate_to_compile() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-rec-guidance", "compilar")
            .with_requirements(vec![Requirement::new("req", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req")]),
            ]);
        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(AlwaysValidateModelClient), session);
        let ctx = AgentContext::new("rec-guidance")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::Compile { .. }));
        assert!(matches!(
            agent
                .trace
                .parsed_decisions
                .last()
                .unwrap()
                .as_ref()
                .unwrap(),
            ModelDecision::Compile { .. }
        ));
        let request = agent.trace.requests.last().expect("request");
        assert_eq!(
            request
                .recommended_action
                .as_ref()
                .map(|rec| rec.kind.as_str()),
            Some("InvokeTool")
        );
    }

    struct AlwaysFinishModelClient;

    impl ModelClient for AlwaysFinishModelClient {
        fn complete(
            &self,
            _request: &crate::harness::model::ModelRequest,
        ) -> Result<crate::harness::model::ModelResponse, ModelError> {
            Ok(crate::harness::model::ModelResponse {
                raw_text: serialize_decision(&ModelDecision::Finish {
                    summary: "premature from model".to_string(),
                }),
            })
        }
    }

    #[test]
    fn ai_agent_gap_guidance_redirects_premature_finish_from_model() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-gap-guidance", "compilar")
            .with_requirements(vec![Requirement::new("req", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req")]),
            ]);
        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(AlwaysFinishModelClient), session);
        let ctx = AgentContext::new("gap-guidance-on")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::Compile { .. }));
        assert!(matches!(
            agent
                .trace
                .parsed_decisions
                .last()
                .unwrap()
                .as_ref()
                .unwrap(),
            ModelDecision::Compile { .. }
        ));
    }

    #[test]
    fn ai_agent_gap_guidance_disabled_preserves_finish_proposal() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-gap-off", "compilar")
            .with_requirements(vec![Requirement::new("req", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req")]),
            ]);
        let session = AiSessionConfig::new("compilar", "Generic");
        let mut agent = AiAgent::new(Box::new(AlwaysFinishModelClient), session);
        let ctx = AgentContext::new("gap-guidance-off")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::Finish { .. }));
    }

    #[test]
    fn ai_agent_gap_guidance_does_not_spin_on_finish_in_loop() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-gap-loop", "compilar")
            .with_requirements(vec![Requirement::new("req", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req")]),
            ]);
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));
        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(AlwaysFinishModelClient), session);
        let ctx = AgentContext::new("gap-loop")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);
        let result = AgentLoop::new(4).run(&harness, &mut agent, ctx);
        assert!(result.tools_executed().contains(&COMPILE.to_string()));
        assert!(
            result
                .history
                .proposed_actions
                .iter()
                .filter(|action| matches!(action, AgentAction::Finish { .. }))
                .count()
                < 4,
            "gap guidance no debe re-proponer Finish en cada iteración"
        );
    }

    /// E2E controlado: Goal insatisfecha → Finish prematuro del modelo →
    /// `apply_gap_guidance` redirige → Compile genera evidencia → Goal satisfecha → Finish permitido.
    ///
    /// Ejercita el path de producción real (`AiAgent::propose` + `AgentLoop`) sin APIs externas.
    #[test]
    fn gap_guidance_e2e_premature_finish_redirects_to_goal_satisfied() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::goal_driven::{
            Goal, GoalEvaluator, GoalStatus, collect_evidence_from_context,
        };
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-gap-e2e", "El código debe compilar")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);
        let goal = Goal::from_specification(spec.clone());
        let working_code = "fn main() { println!(\"ok\"); }\n";

        let ctx = AgentContext::new("gap-e2e")
            .with_working_code(working_code)
            .with_evaluation_specification(spec.clone());
        let initial_eval =
            GoalEvaluator::new().evaluate(&goal, &collect_evidence_from_context(&ctx));
        assert_ne!(
            initial_eval.status,
            GoalStatus::Satisfied,
            "artifacto inicial debe fallar Goal (sin evidencia de compile)"
        );
        assert!(
            !initial_eval.gap.is_empty(),
            "debe existir GoalGap accionable antes del loop"
        );

        let mut harness = Harness::new(6);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));
        let session = AiSessionConfig::new("compilar", "Generic").with_gap_guidance(true);
        let mut agent = AiAgent::new(Box::new(AlwaysFinishModelClient), session);
        let result = AgentLoop::new(6).run(&harness, &mut agent, ctx);

        let first_request = agent.trace.requests.first().expect("request inicial");
        assert!(first_request.goal_evaluation.is_some());
        assert!(first_request.goal_gap.is_some());
        assert!(
            matches!(
                agent
                    .trace
                    .parsed_decisions
                    .first()
                    .unwrap()
                    .as_ref()
                    .unwrap(),
                ModelDecision::Compile { .. }
            ),
            "apply_gap_guidance debe redirigir Finish prematuro a Compile"
        );
        assert!(
            matches!(
                result.history.proposed_actions.first(),
                Some(AgentAction::Compile { .. })
            ),
            "primera acción ejecutable debe ser Compile, no Finish"
        );
        assert!(
            result.tools_executed().contains(&COMPILE.to_string()),
            "el agente debe ejecutar Compile para cerrar el gap"
        );

        let final_eval = GoalEvaluator::new()
            .evaluate(&goal, &collect_evidence_from_context(&result.final_context));
        assert_eq!(
            final_eval.status,
            GoalStatus::Satisfied,
            "Goal debe alcanzarse tras generar evidencia de compile"
        );
        assert_eq!(result.status, LoopStatus::Completed);
    }
}
