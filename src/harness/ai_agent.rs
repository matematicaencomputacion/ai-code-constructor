use crate::harness::action::AgentAction;
use crate::harness::agent::Agent;
use crate::harness::context::AgentContext;
use crate::harness::correction::Correction;
use crate::harness::model::{
    AiSessionConfig, ModelClient, ModelDecision, ModelError, ModelInteractionTrace,
    ModelResponseError, model_request_from_context, parse_model_response, structured_to_correction,
    validate_apply_correction,
};

/// Primer Agent basado en IA: serializa contexto, consulta [`ModelClient`]
/// y convierte la respuesta validada en [`AgentAction`].
///
/// No ejecuta Tools ni conoce Harness directamente.
pub struct AiAgent {
    client: Box<dyn ModelClient>,
    session: AiSessionConfig,
    pub trace: ModelInteractionTrace,
    pub last_model_error: Option<ModelError>,
    pub last_response_error: Option<ModelResponseError>,
}

impl AiAgent {
    pub fn new(client: Box<dyn ModelClient>, session: AiSessionConfig) -> Self {
        Self {
            client,
            session,
            trace: ModelInteractionTrace::default(),
            last_model_error: None,
            last_response_error: None,
        }
    }

    fn action_label(action: &AgentAction) -> String {
        match action {
            AgentAction::Validate { .. } => "validate".to_string(),
            AgentAction::RepairDiagnostic { .. } => "repair_diagnostic".to_string(),
            AgentAction::ApplyCorrection { .. } => "apply_correction".to_string(),
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
                validate_apply_correction(&corrections, ctx.working_code())?;
                let mapped = corrections
                    .iter()
                    .map(structured_to_correction)
                    .collect::<Vec<Correction>>();
                Ok(AgentAction::ApplyCorrection {
                    corrections: mapped,
                })
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

        let response = match self.client.complete(&request) {
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
        APPLY_CORRECTION, COMPILE, CorrectionTool, REPAIR_DIAGNOSTIC, RepairDiagnosticTool,
        VALIDATE, ValidationTool,
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
        let session = AiSessionConfig {
            user_request: "r".to_string(),
            plan_kind: "Api".to_string(),
        };
        let mut agent = AiAgent::new(Box::new(client), session);
        let mut ctx = AgentContext::new("ai");
        ctx.step = 1;
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::RepairDiagnostic { .. }));
    }

    #[test]
    fn ai_agent_rejects_invalid_response_without_tool_execution() {
        let client = MockModelClient::invalid();
        let session = AiSessionConfig {
            user_request: "r".to_string(),
            plan_kind: "Api".to_string(),
        };
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
                search: "NET".to_string(),
                replacement: "HTTP".to_string(),
            }],
        };
        let client = ScriptModelClient::new(vec![serialize_decision(&decision)]);
        let session = AiSessionConfig {
            user_request: "r".to_string(),
            plan_kind: "Api".to_string(),
        };
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

        let session = AiSessionConfig {
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        };
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

        let session = AiSessionConfig {
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        };
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
    fn ai_agent_trace_records_request_and_response() {
        let client = MockModelClient::new();
        let session = AiSessionConfig {
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        };
        let mut agent = AiAgent::new(Box::new(client), session);
        let _ = agent.propose(&AgentContext::new("trace").with_working_code("NET"));
        assert_eq!(agent.trace.requests.len(), 1);
        assert_eq!(agent.trace.responses.len(), 1);
        assert_eq!(agent.trace.parsed_decisions.len(), 1);
    }
}
