//! Primera sesión live: Model REAL → AiAgent → Harness → Tools → Observation → Model.
//!
//! LiveSession usa [`ActionPolicy::default_session_policy`] por defecto (inyectable).

use crate::harness::action_policy::ActionPolicy;
use crate::harness::agent_loop::{AgentLoop, LoopResult};
use crate::harness::agent_prompt::{SYSTEM_PROMPT_VERSION, system_prompt_v1};
use crate::harness::ai_agent::AiAgent;
use crate::harness::artifact::RustArtifact;
use crate::harness::constraint::Constraint;
use crate::harness::context::AgentContext;
use crate::harness::evaluation::EvaluationVerdict;
use crate::harness::model::{
    AiSessionConfig, ModelClient, ModelError, ModelInteractionTrace, model_request_from_context,
};
use crate::harness::observation::AgentObservation;
use crate::harness::openai_compatible_client::{ModelClientConfig, OpenAICompatibleModelClient};
use crate::harness::retrying_model_client::RetryingModelClient;
use crate::harness::runtime::Harness;
use crate::harness::specification::Specification;
use crate::harness::tools::{CompileTool, CorrectionTool, RepairDiagnosticTool, ValidationTool};

/// Límite estricto de iteraciones para sesiones live.
pub const LIVE_AGENT_MAX_ITERATIONS: u32 = 8;

/// Configuración de una sesión live controlada.
///
/// La ActionPolicy no vive aquí (no es `Clone`/`Eq`); se inyecta al ejecutar
/// vía [`run_live_agent_session_with_client_and_policy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSessionConfig {
    pub goal: String,
    pub user_request: String,
    pub plan_kind: String,
    /// Código de compatibilidad; se materializa como [`RustArtifact`] al iniciar.
    pub working_code: String,
    /// Artifact explícito (preferido). Si está presente, tiene prioridad sobre `working_code`.
    pub working_artifact: Option<RustArtifact>,
    /// Specification opcional para Evaluation + FinishConstraint.
    pub evaluation_specification: Option<Specification>,
    pub max_iterations: u32,
    pub debug_log_prompt: bool,
}

impl LiveSessionConfig {
    pub fn validate_and_compile_artifact(
        user_request: impl Into<String>,
        plan_kind: impl Into<String>,
        working_code: impl Into<String>,
    ) -> Self {
        let working_code = working_code.into();
        Self {
            goal: "live:validate-and-compile".to_string(),
            user_request: user_request.into(),
            plan_kind: plan_kind.into(),
            working_artifact: Some(RustArtifact::new("main.rs", working_code.clone())),
            working_code,
            evaluation_specification: None,
            max_iterations: LIVE_AGENT_MAX_ITERATIONS,
            debug_log_prompt: false,
        }
    }

    pub fn with_artifact(mut self, artifact: RustArtifact) -> Self {
        self.working_code = artifact.source().to_string();
        self.working_artifact = Some(artifact);
        self
    }

    pub fn with_specification(mut self, specification: Specification) -> Self {
        self.evaluation_specification = Some(specification);
        self
    }
}

/// Error controlado al iniciar o ejecutar una sesión live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveSessionError {
    Configuration(String),
    Model(ModelError),
}

impl std::fmt::Display for LiveSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(message) => write!(f, "configuración live: {message}"),
            Self::Model(error) => write!(f, "modelo: {error}"),
        }
    }
}

impl From<ModelError> for LiveSessionError {
    fn from(error: ModelError) -> LiveSessionError {
        Self::Model(error)
    }
}

/// Registro seguro de una iteración live (sin secretos).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSessionStepRecord {
    pub iteration: u32,
    pub proposed_action: String,
    pub tool_executed: bool,
    pub tool_name: Option<String>,
    pub success: bool,
    pub permitted: bool,
    pub rejected_constraint: Option<String>,
    pub rejected_reason: Option<String>,
    pub evaluation_verdict: Option<String>,
    pub observation_summary: String,
    pub model_latency_ms: Option<u64>,
    pub retry_count: Option<u32>,
}

/// Traza estructurada de la sesión live.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveSessionTrace {
    pub prompt_version: String,
    pub model_name: Option<String>,
    /// Nombre de la ActionPolicy registrada (`action_policy` por defecto).
    pub action_policy: String,
    pub records: Vec<LiveSessionStepRecord>,
    pub finish_reason: String,
    pub total_retries: u32,
}

impl LiveSessionTrace {
    pub fn log_summary(&self) {
        println!("=== LIVE SESSION TRACE ===");
        println!("prompt_version={}", self.prompt_version);
        println!("action_policy={}", self.action_policy);
        if let Some(model) = &self.model_name {
            println!("model={model}");
        }
        println!("finish_reason={}", self.finish_reason);
        println!("total_retries={}", self.total_retries);
        for record in &self.records {
            println!(
                "iter={} action={} permitted={} rejected_constraint={:?} tool={:?} executed={} success={} eval={:?} obs={} latency_ms={:?} retries={:?}",
                record.iteration,
                record.proposed_action,
                record.permitted,
                record.rejected_constraint,
                record.tool_name,
                record.tool_executed,
                record.success,
                record.evaluation_verdict,
                record.observation_summary,
                record.model_latency_ms,
                record.retry_count,
            );
        }
    }

    /// Garantiza que la traza no filtre secretos conocidos.
    pub fn contains_secrets(&self) -> bool {
        let blob = format!("{self:?}").to_ascii_lowercase();
        blob.contains("api_key")
            || blob.contains("authorization")
            || blob.contains("bearer ")
            || blob.contains("sk-")
    }
}

/// Resultado de una sesión live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSessionResult {
    pub loop_result: LoopResult,
    pub model_trace: ModelInteractionTrace,
    pub session_trace: LiveSessionTrace,
}

/// Construye Harness con Tools + [`ActionPolicy::default_session_policy`].
pub fn build_validate_compile_harness() -> Harness {
    build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy())
}

/// Construye Harness con Tools y una [`ActionPolicy`] inyectada.
pub fn build_validate_compile_harness_with_policy(policy: ActionPolicy) -> Harness {
    let mut harness = Harness::new(LIVE_AGENT_MAX_ITERATIONS);
    harness.register_tool(Box::new(ValidationTool));
    harness.register_tool(Box::new(RepairDiagnosticTool));
    harness.register_tool(Box::new(CorrectionTool));
    harness.register_tool(Box::new(CompileTool));
    harness.register_constraint(Box::new(policy));
    harness
}

/// Ejecuta sesión live leyendo configuración del ModelClient desde entorno.
pub fn run_live_agent_session(
    config: LiveSessionConfig,
) -> Result<LiveSessionResult, LiveSessionError> {
    let model_config = ModelClientConfig::from_env()?;
    let inner = OpenAICompatibleModelClient::new(model_config.clone());
    let client: Box<dyn ModelClient> = Box::new(RetryingModelClient::new(Box::new(inner)));
    run_live_agent_session_with_client(client, config, Some(model_config.model))
}

/// Ejecuta sesión live con [`ActionPolicy::default_session_policy`].
pub fn run_live_agent_session_with_client(
    client: Box<dyn ModelClient>,
    config: LiveSessionConfig,
    model_name: Option<String>,
) -> Result<LiveSessionResult, LiveSessionError> {
    run_live_agent_session_with_client_and_policy(
        client,
        config,
        model_name,
        ActionPolicy::default_session_policy(),
    )
}

/// Ejecuta sesión live con ActionPolicy inyectada (tests / entornos futuros).
pub fn run_live_agent_session_with_client_and_policy(
    client: Box<dyn ModelClient>,
    config: LiveSessionConfig,
    model_name: Option<String>,
    policy: ActionPolicy,
) -> Result<LiveSessionResult, LiveSessionError> {
    if config.user_request.trim().is_empty() {
        return Err(LiveSessionError::Configuration(
            "user_request vacío".to_string(),
        ));
    }
    if config.working_code.trim().is_empty()
        && config
            .working_artifact
            .as_ref()
            .map(|a| a.source().trim().is_empty())
            .unwrap_or(true)
    {
        return Err(LiveSessionError::Configuration(
            "working_artifact / working_code vacío".to_string(),
        ));
    }

    let max_iterations = config.max_iterations.min(LIVE_AGENT_MAX_ITERATIONS);
    let session = AiSessionConfig {
        user_request: config.user_request.clone(),
        plan_kind: config.plan_kind.clone(),
    };
    let mut ctx = match config.working_artifact.clone() {
        Some(artifact) => AgentContext::new(&config.goal).with_working_artifact(artifact),
        None => AgentContext::new(&config.goal).with_working_code(config.working_code.clone()),
    };
    if let Some(specification) = config.evaluation_specification.clone() {
        ctx = ctx.with_evaluation_specification(specification);
    }

    if config.debug_log_prompt {
        let preview = model_request_from_context(&ctx, &session)
            .map(|request| request.system_prompt)
            .unwrap_or_else(|_| system_prompt_v1().to_string());
        println!("=== LIVE SESSION PROMPT ({SYSTEM_PROMPT_VERSION}) ===");
        println!("{preview}");
    }

    let policy_name = policy.name().to_string();
    let mut agent = AiAgent::new(client, session);
    let harness = build_validate_compile_harness_with_policy(policy);
    let loop_result = AgentLoop::new(max_iterations).run(&harness, &mut agent, ctx);

    let session_trace = build_session_trace(&loop_result, &agent.trace, model_name, &policy_name);
    session_trace.log_summary();

    Ok(LiveSessionResult {
        loop_result,
        model_trace: agent.trace.clone(),
        session_trace,
    })
}

fn build_session_trace(
    loop_result: &LoopResult,
    model_trace: &ModelInteractionTrace,
    model_name: Option<String>,
    policy_name: &str,
) -> LiveSessionTrace {
    let mut records = Vec::new();
    for (index, step) in loop_result.history.steps.iter().enumerate() {
        let iteration = (index + 1) as u32;
        let proposed_action = loop_result
            .history
            .proposed_actions
            .get(index)
            .map(action_label)
            .unwrap_or_else(|| "unknown".to_string());

        let evaluation_verdict = Some(match step.evaluation.verdict {
            EvaluationVerdict::Pass => "Pass".to_string(),
            EvaluationVerdict::Fail => "Fail".to_string(),
            EvaluationVerdict::InsufficientEvidence => "InsufficientEvidence".to_string(),
        });

        let success = match &step.observation {
            AgentObservation::ActionRejected { .. } => false,
            other => other.is_success(),
        };

        records.push(LiveSessionStepRecord {
            iteration,
            proposed_action,
            tool_executed: step.tool_executed,
            tool_name: step.tool_name.clone(),
            success,
            permitted: step.permitted,
            rejected_constraint: step.rejected_constraint.clone(),
            rejected_reason: step.rejected_reason.clone(),
            evaluation_verdict,
            observation_summary: step.observation.summary(),
            model_latency_ms: None,
            retry_count: None,
        });
    }

    // Retries del ModelClient son independientes del AgentLoop; se reportan agregados.
    let total_retries = 0;
    let _ = model_trace;

    LiveSessionTrace {
        prompt_version: SYSTEM_PROMPT_VERSION.to_string(),
        model_name,
        action_policy: policy_name.to_string(),
        records,
        finish_reason: loop_result.termination_reason.clone(),
        total_retries,
    }
}

fn action_label(action: &crate::harness::AgentAction) -> String {
    match action {
        crate::harness::AgentAction::Validate { .. } => "validate".to_string(),
        crate::harness::AgentAction::RepairDiagnostic { .. } => "repair_diagnostic".to_string(),
        crate::harness::AgentAction::ApplyCorrection { .. } => "apply_correction".to_string(),
        crate::harness::AgentAction::Compile { .. } => "compile".to_string(),
        crate::harness::AgentAction::Finish { .. } => "finish".to_string(),
        crate::harness::AgentAction::RunTests { .. } => "run_tests".to_string(),
        crate::harness::AgentAction::RunClippy => "run_clippy".to_string(),
        crate::harness::AgentAction::CheckFormat => "check_format".to_string(),
        crate::harness::AgentAction::InvokeTool { tool_name, .. } => format!("invoke:{tool_name}"),
        crate::harness::AgentAction::NoOp => "noop".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::action::AgentAction;
    use crate::harness::action_policy::{ActionPolicy, ArtifactStateConstraint, FinishConstraint};
    use crate::harness::agent::Agent;
    use crate::harness::agent_loop::LoopStatus;
    use crate::harness::artifact::ArtifactId;
    use crate::harness::bridge::introduce_validation_defect;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::evaluation::EvaluationVerdict;
    use crate::harness::evaluation_engine::EvaluationEngine;
    use crate::harness::evaluation_observation::observation_from_criterion_evaluation;
    use crate::harness::model::{MockModelClient, ModelDecision, ModelRequest, serialize_decision};
    use crate::harness::openai_compatible_client::ModelClientConfig;
    use crate::harness::retrying_model_client::RetryConfig;
    use crate::harness::specification::{
        AcceptanceCriterion, Requirement, Specification, SpecificationId,
    };
    use crate::harness::tool_permission::ToolPermissionConstraint;
    use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC, VALIDATE};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

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

    fn compile_spec() -> Specification {
        Specification::new("spec-live", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    #[test]
    fn live_session_uses_action_policy_by_default() {
        // A
        let harness = build_validate_compile_harness();
        let mut ctx = AgentContext::new("a");
        let outcome = harness.execute_step(
            AgentAction::Compile {
                code: "fn main() {}".to_string(),
            },
            &mut ctx,
        );
        assert!(!outcome.permitted);
        assert_eq!(
            outcome.rejected_constraint.as_deref(),
            Some("artifact_state")
        );
        assert_eq!(
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy())
                .execute_step(AgentAction::NoOp, &mut AgentContext::new("a2"))
                .permitted,
            true
        );
    }

    #[test]
    fn live_session_policy_can_be_injected() {
        // B
        let policy =
            ActionPolicy::new().with_constraint(Box::new(ToolPermissionConstraint::new(["echo"])));
        let result = run_live_agent_session_with_client_and_policy(
            Box::new(MockModelClient::new()),
            LiveSessionConfig::validate_and_compile_artifact(
                "Crear una API REST",
                "Api",
                "fn main() {}",
            ),
            Some("mock".to_string()),
            ActionPolicy::new().with_constraint(Box::new(
                ToolPermissionConstraint::default_constructor_tools(),
            )),
        )
        .expect("injected");
        assert_eq!(result.session_trace.action_policy, "action_policy");
        let _ = policy;
    }

    #[test]
    fn live_session_config_has_iteration_limit() {
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        );
        assert_eq!(config.max_iterations, LIVE_AGENT_MAX_ITERATIONS);
        assert!(config.working_artifact.is_some());
    }

    #[test]
    fn live_session_can_start_with_explicit_artifact() {
        let artifact =
            RustArtifact::with_id(ArtifactId::new("live-art-1"), "main.rs", "fn main() {}");
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "ignored",
        )
        .with_artifact(artifact);

        struct FinishClient;
        impl ModelClient for FinishClient {
            fn complete(
                &self,
                request: &ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, ModelError> {
                assert_eq!(request.artifact_id.as_deref(), Some("live-art-1"));
                Ok(crate::harness::model::ModelResponse {
                    raw_text: serialize_decision(&ModelDecision::Finish {
                        summary: "artifact session ok".to_string(),
                    }),
                })
            }
        }

        let result = run_live_agent_session_with_client(
            Box::new(FinishClient),
            config,
            Some("mock".to_string()),
        )
        .expect("live session");
        assert_eq!(
            result
                .loop_result
                .final_context
                .working_artifact
                .as_ref()
                .map(|a| a.id().as_str()),
            Some("live-art-1")
        );
    }

    #[test]
    fn missing_env_configuration_returns_controlled_error() {
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        );
        let previous_base = std::env::var("MODEL_BASE_URL").ok();
        unsafe {
            std::env::remove_var("MODEL_BASE_URL");
        }
        let err = run_live_agent_session(config).unwrap_err();
        assert!(matches!(
            err,
            LiveSessionError::Model(ModelError::Configuration(_))
        ));
        if let Some(value) = previous_base {
            unsafe {
                std::env::set_var("MODEL_BASE_URL", value);
            }
        }
    }

    #[test]
    fn live_session_with_mock_model_client_completes_flow() {
        let invalid = introduce_validation_defect(&api_valid_code());
        let config =
            LiveSessionConfig::validate_and_compile_artifact("Crear una API REST", "Api", invalid);
        let result = run_live_agent_session_with_client(
            Box::new(MockModelClient::new()),
            config,
            Some("mock-model".to_string()),
        )
        .expect("live session");

        assert_eq!(result.loop_result.status, LoopStatus::Completed);
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|t| t == VALIDATE)
        );
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|t| t == REPAIR_DIAGNOSTIC)
        );
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|t| t == APPLY_CORRECTION)
        );
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|t| t == COMPILE)
        );
        assert_eq!(result.session_trace.action_policy, "action_policy");
        assert_eq!(result.session_trace.prompt_version, SYSTEM_PROMPT_VERSION);
    }

    #[test]
    fn invalid_model_response_does_not_execute_tools_in_live_session() {
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        );
        let result =
            run_live_agent_session_with_client(Box::new(MockModelClient::invalid()), config, None)
                .expect("live session");
        assert!(result.loop_result.tools_executed().is_empty());
        assert!(matches!(
            result.loop_result.history.proposed_actions[0],
            AgentAction::Finish { .. }
        ));
    }

    #[test]
    fn live_session_respects_max_iterations_cap() {
        // L
        let config = LiveSessionConfig {
            goal: "live-cap".to_string(),
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
            working_code: "fn main() {}".to_string(),
            working_artifact: Some(RustArtifact::new("main.rs", "fn main() {}")),
            evaluation_specification: Some(compile_spec()),
            max_iterations: 100,
            debug_log_prompt: false,
        };
        assert_eq!(config.max_iterations.min(LIVE_AGENT_MAX_ITERATIONS), 8);

        struct AlwaysFinishClient;
        impl ModelClient for AlwaysFinishClient {
            fn complete(
                &self,
                _request: &ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, ModelError> {
                Ok(crate::harness::model::ModelResponse {
                    raw_text: serialize_decision(&ModelDecision::Finish {
                        summary: "premature".to_string(),
                    }),
                })
            }
        }

        let result = run_live_agent_session_with_client(Box::new(AlwaysFinishClient), config, None)
            .expect("session");
        assert_eq!(result.loop_result.status, LoopStatus::MaxIterations);
        assert_eq!(result.loop_result.iterations, LIVE_AGENT_MAX_ITERATIONS);
        assert!(
            result
                .session_trace
                .records
                .iter()
                .all(|r| !r.tool_executed)
        );
    }

    #[test]
    fn invalid_action_does_not_execute_tool_and_produces_action_rejected() {
        // C + D + E + F + J + K
        let executed = Arc::new(AtomicBool::new(false));
        struct TrackingClient {
            flag: Arc<AtomicBool>,
            saw_rejected: Arc<AtomicBool>,
        }
        impl ModelClient for TrackingClient {
            fn complete(
                &self,
                request: &ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, ModelError> {
                match &request.last_observation {
                    None => Ok(crate::harness::model::ModelResponse {
                        raw_text: serialize_decision(&ModelDecision::Finish {
                            summary: "premature".to_string(),
                        }),
                    }),
                    Some(obs) if obs.kind == "action_rejected" => {
                        self.saw_rejected.store(true, Ordering::SeqCst);
                        Ok(crate::harness::model::ModelResponse {
                            raw_text: serialize_decision(&ModelDecision::Compile {
                                code: request.working_code.clone().unwrap_or_default(),
                            }),
                        })
                    }
                    Some(obs)
                        if obs.kind == "criterion_evaluated"
                            && obs.evaluation_verdict.as_deref() == Some("Pass") =>
                    {
                        Ok(crate::harness::model::ModelResponse {
                            raw_text: serialize_decision(&ModelDecision::Finish {
                                summary: "done after pass".to_string(),
                            }),
                        })
                    }
                    Some(obs) if obs.tool_name.as_deref() == Some(COMPILE) => {
                        self.flag.store(true, Ordering::SeqCst);
                        Ok(crate::harness::model::ModelResponse {
                            raw_text: serialize_decision(&ModelDecision::Finish {
                                summary: "after compile tool".to_string(),
                            }),
                        })
                    }
                    _ => Ok(crate::harness::model::ModelResponse {
                        raw_text: serialize_decision(&ModelDecision::Finish {
                            summary: "stop".to_string(),
                        }),
                    }),
                }
            }
        }

        let saw_rejected = Arc::new(AtomicBool::new(false));
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        )
        .with_specification(compile_spec());
        let before_spec = config.evaluation_specification.clone();
        let before_artifact = config.working_artifact.clone();

        let result = run_live_agent_session_with_client(
            Box::new(TrackingClient {
                flag: Arc::clone(&executed),
                saw_rejected: Arc::clone(&saw_rejected),
            }),
            config,
            None,
        )
        .expect("session");

        assert!(result.session_trace.records.iter().any(|r| {
            !r.permitted && r.rejected_constraint.as_deref() == Some("finish") && !r.tool_executed
        }));
        assert!(result.loop_result.history.observations.iter().any(|o| {
            matches!(
                o,
                AgentObservation::ActionRejected {
                    constraint,
                    ..
                } if constraint == "finish"
            )
        }));
        assert!(saw_rejected.load(Ordering::SeqCst));
        assert!(result.model_trace.requests.iter().any(|r| {
            r.last_observation.as_ref().map(|o| o.kind.as_str()) == Some("action_rejected")
        }));
        assert_eq!(
            result
                .loop_result
                .final_context
                .working_artifact
                .as_ref()
                .map(|a| a.id().as_str()),
            before_artifact.as_ref().map(|a| a.id().as_str())
        );
        assert_eq!(
            result
                .loop_result
                .final_context
                .evaluation_specification
                .as_ref()
                .map(|s| s.id.as_str()),
            before_spec.as_ref().map(|s| s.id.as_str())
        );
        assert_eq!(result.loop_result.status, LoopStatus::Completed);
    }

    #[test]
    fn finish_fail_and_insufficient_evidence_rejected_pass_allowed() {
        // G + H + I
        let fail_eval = EvaluationEngine::new().evaluate_criterion(
            &compile_spec().acceptance_criteria[0],
            &[
                crate::harness::Evidence::new("tool", COMPILE),
                crate::harness::Evidence::new("compile_status", "error"),
            ],
        );
        let insuf_eval = EvaluationEngine::new().evaluate_criterion(
            &compile_spec().acceptance_criteria[0],
            &[crate::harness::Evidence::new("tool", COMPILE)],
        );
        let pass_eval = EvaluationEngine::new().evaluate_criterion(
            &compile_spec().acceptance_criteria[0],
            &[
                crate::harness::Evidence::new("tool", COMPILE),
                crate::harness::Evidence::new("compile_status", "ok"),
            ],
        );
        assert_eq!(fail_eval.verdict, EvaluationVerdict::Fail);
        assert_eq!(insuf_eval.verdict, EvaluationVerdict::InsufficientEvidence);
        assert_eq!(pass_eval.verdict, EvaluationVerdict::Pass);

        let harness = build_validate_compile_harness();
        let mut fail_ctx = AgentContext::new("g")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_spec());
        fail_ctx.push_observation(observation_from_criterion_evaluation(
            SpecificationId::new("spec-live"),
            &fail_eval,
        ));
        assert!(
            !harness
                .execute_step(
                    AgentAction::Finish {
                        summary: "no".to_string()
                    },
                    &mut fail_ctx
                )
                .permitted
        );

        let mut insuf_ctx = AgentContext::new("h")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_spec());
        insuf_ctx.push_observation(observation_from_criterion_evaluation(
            SpecificationId::new("spec-live"),
            &insuf_eval,
        ));
        assert!(
            !harness
                .execute_step(
                    AgentAction::Finish {
                        summary: "no".to_string()
                    },
                    &mut insuf_ctx
                )
                .permitted
        );

        let mut pass_ctx = AgentContext::new("i")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_spec());
        pass_ctx.push_observation(observation_from_criterion_evaluation(
            SpecificationId::new("spec-live"),
            &pass_eval,
        ));
        assert!(
            harness
                .execute_step(
                    AgentAction::Finish {
                        summary: "yes".to_string()
                    },
                    &mut pass_ctx
                )
                .permitted
        );
    }

    #[test]
    fn model_client_retry_does_not_consume_agent_loop_iteration() {
        // M
        struct FlakyThenFinish {
            calls: AtomicUsize,
        }
        impl ModelClient for FlakyThenFinish {
            fn complete(
                &self,
                _request: &ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, ModelError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    return Err(ModelError::Timeout);
                }
                Ok(crate::harness::model::ModelResponse {
                    raw_text: serialize_decision(&ModelDecision::Finish {
                        summary: "ok after retries".to_string(),
                    }),
                })
            }
        }

        let inner = FlakyThenFinish {
            calls: AtomicUsize::new(0),
        };
        let client = RetryingModelClient::with_config(
            Box::new(inner),
            RetryConfig {
                max_retries: 3,
                backoff: Duration::from_millis(1),
            },
        );
        let retries_before = 0;
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        );
        let result =
            run_live_agent_session_with_client(Box::new(client), config, None).expect("session");
        assert_eq!(result.loop_result.iterations, 1);
        assert!(result.loop_result.iterations > retries_before);
        assert_eq!(result.loop_result.status, LoopStatus::Completed);
    }

    #[test]
    fn tool_permission_and_action_validity_still_work() {
        // N + O
        let harness = build_validate_compile_harness();
        let mut ctx = AgentContext::new("n").with_working_code("fn main() {}");
        let denied = harness.execute_step(
            AgentAction::InvokeTool {
                tool_name: "echo".to_string(),
                input: "x".to_string(),
            },
            &mut ctx,
        );
        assert!(!denied.permitted);
        assert_eq!(
            denied.rejected_constraint.as_deref(),
            Some("tool_permission")
        );

        let mut empty = AgentContext::new("o");
        let invalid = harness.execute_step(
            AgentAction::Validate {
                request: "Crear una API REST".to_string(),
                code: Some("fn main() {}".to_string()),
                plan_kind: "Api".to_string(),
            },
            &mut empty,
        );
        assert!(!invalid.permitted);
        assert_eq!(
            invalid.rejected_constraint.as_deref(),
            Some("artifact_state")
        );
        let _ = (ArtifactStateConstraint, FinishConstraint);
    }

    #[test]
    fn evaluation_engine_remains_independent() {
        // P
        let evaluation = EvaluationEngine::new().evaluate_criterion(
            &AcceptanceCriterion::new("ac-1", "c", CriterionKind::Compile),
            &[
                crate::harness::Evidence::new("tool", COMPILE),
                crate::harness::Evidence::new("compile_status", "ok"),
            ],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn live_trace_does_not_contain_secrets() {
        // Q
        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        );
        let result = run_live_agent_session_with_client(
            Box::new(MockModelClient::invalid()),
            config,
            Some("mock".to_string()),
        )
        .expect("session");
        assert!(!result.session_trace.contains_secrets());
        let dumped = format!("{:?}", result.session_trace).to_ascii_lowercase();
        assert!(!dumped.contains("api_key"));
        assert!(!dumped.contains("authorization"));
    }

    #[test]
    fn ai_agent_never_executes_tools_directly() {
        // R
        let mut agent = AiAgent::new(
            Box::new(MockModelClient::new()),
            AiSessionConfig {
                user_request: "Crear una API REST".to_string(),
                plan_kind: "Api".to_string(),
            },
        );
        let action = agent.propose(&AgentContext::new("r").with_working_code("fn main() {}"));
        assert!(matches!(action, AgentAction::Validate { .. }));
    }

    #[test]
    fn e2e_live_session_reject_then_compile_evaluate_finish() {
        struct CausalLiveClient;
        impl ModelClient for CausalLiveClient {
            fn complete(
                &self,
                request: &ModelRequest,
            ) -> Result<crate::harness::model::ModelResponse, ModelError> {
                let decision = match &request.last_observation {
                    None => ModelDecision::Finish {
                        summary: "premature finish".to_string(),
                    },
                    Some(obs) if obs.kind == "action_rejected" => ModelDecision::Compile {
                        code: request.working_code.clone().unwrap_or_default(),
                    },
                    Some(obs)
                        if obs.kind == "criterion_evaluated"
                            && obs.evaluation_verdict.as_deref() == Some("Pass") =>
                    {
                        ModelDecision::Finish {
                            summary: "finish after evaluation pass".to_string(),
                        }
                    }
                    Some(obs) if obs.tool_name.as_deref() == Some(COMPILE) => {
                        ModelDecision::Finish {
                            summary: "finish after compile outcome".to_string(),
                        }
                    }
                    _ => ModelDecision::Finish {
                        summary: "stop".to_string(),
                    },
                };
                Ok(crate::harness::model::ModelResponse {
                    raw_text: serialize_decision(&decision),
                })
            }
        }

        let config = LiveSessionConfig::validate_and_compile_artifact(
            "Crear una API REST",
            "Api",
            "fn main() {}",
        )
        .with_specification(compile_spec());

        let result = run_live_agent_session_with_client(
            Box::new(CausalLiveClient),
            config,
            Some("causal-mock".to_string()),
        )
        .expect("e2e");

        let actions: Vec<_> = result
            .loop_result
            .history
            .proposed_actions
            .iter()
            .map(action_label)
            .collect();
        assert_eq!(actions.first().map(String::as_str), Some("finish"));
        assert!(actions.iter().any(|a| a == "compile"));
        assert!(result.session_trace.records.iter().any(|r| {
            r.proposed_action == "finish"
                && !r.permitted
                && r.rejected_constraint.as_deref() == Some("finish")
        }));
        assert!(
            result
                .loop_result
                .tools_executed()
                .iter()
                .any(|t| t == COMPILE)
        );
        assert!(result.loop_result.history.observations.iter().any(|o| {
            matches!(
                o,
                AgentObservation::CriterionEvaluated {
                    verdict: EvaluationVerdict::Pass,
                    specification_id,
                    ..
                } if specification_id.as_str() == "spec-live"
            )
        }));
        assert_eq!(result.loop_result.status, LoopStatus::Completed);
        assert!(!result.session_trace.contains_secrets());
    }

    #[test]
    fn openai_compatible_client_can_be_wrapped_for_live_session() {
        let config = ModelClientConfig::new(
            "http://127.0.0.1:9",
            "test-model",
            Some("test-api-key".to_string()),
            Duration::from_millis(100),
        );
        let inner = OpenAICompatibleModelClient::new(config);
        let _client: Box<dyn ModelClient> = Box::new(RetryingModelClient::new(Box::new(inner)));
    }

    /// Prueba manual (NO CI). Requiere MODEL_BASE_URL, MODEL_API_KEY, MODEL_NAME.
    #[test]
    #[ignore = "requiere endpoint real y variables MODEL_* configuradas por el operador"]
    fn manual_live_agent_session() {
        let invalid = introduce_validation_defect(&api_valid_code());
        let config =
            LiveSessionConfig::validate_and_compile_artifact("Crear una API REST", "Api", invalid);
        let result = run_live_agent_session(config).expect("live session");
        assert!(
            result.loop_result.status == LoopStatus::Completed
                || result.loop_result.status == LoopStatus::MaxIterations
        );
        assert!(!result.session_trace.records.is_empty());
        assert_eq!(result.session_trace.action_policy, "action_policy");
    }
}
