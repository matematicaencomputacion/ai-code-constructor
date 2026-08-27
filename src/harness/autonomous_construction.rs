//! Flujo experimental: Specification → Plan → Artifact → AgentLoop → ConstructionResult.
//!
//! No reemplaza [`crate::main::run_constructor`]. Reutiliza AgentLoop, ActionPolicy,
//! EvaluationEngine y el Harness de sesión (Validate / Repair / Correct / Compile).

use crate::harness::action_policy::ActionPolicy;
use crate::harness::agent::Agent;
use crate::harness::agent_loop::{AgentLoop, LoopResult, LoopStatus};
use crate::harness::ai_agent::AiAgent;
use crate::harness::artifact::{ArtifactId, RustArtifact};
use crate::harness::constraint::Constraint;
use crate::harness::context::AgentContext;
use crate::harness::evaluation_engine::{
    EvaluationEngine, SpecificationEvaluation, SpecificationEvaluationStatus,
};
use crate::harness::live_session::build_validate_compile_harness_with_policy;
use crate::harness::model::{AiSessionConfig, ModelClient};
use crate::harness::specification::{Specification, SpecificationId, SpecificationValidationError};
use crate::harness::specification_planner::{
    SpecificationBuildPlan, SpecificationPlannerError, plan_specification,
};
use crate::planner::PlanKind;

/// Estado terminal de una construcción autónoma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructionStatus {
    Completed,
    Failed,
    MaxIterations,
    InvalidSpecification,
}

/// Configuración de una sesión de construcción desde Specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomousConstructionConfig {
    pub specification: Specification,
    /// Source inicial del Artifact (típicamente inválido; el Agent debe corregirlo).
    pub initial_source: String,
    pub max_iterations: u32,
    /// Nombre del archivo del Artifact.
    pub artifact_name: String,
}

impl AutonomousConstructionConfig {
    pub fn new(
        specification: Specification,
        initial_source: impl Into<String>,
        max_iterations: u32,
    ) -> Self {
        Self {
            specification,
            initial_source: initial_source.into(),
            max_iterations,
            artifact_name: "main.rs".to_string(),
        }
    }

    pub fn with_artifact_name(mut self, name: impl Into<String>) -> Self {
        self.artifact_name = name.into();
        self
    }
}

/// Resultado estructurado y trazable de la construcción.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructionResult {
    pub status: ConstructionStatus,
    pub specification_id: SpecificationId,
    pub artifact_id: Option<ArtifactId>,
    pub final_artifact: Option<RustArtifact>,
    pub build_plan: Option<SpecificationBuildPlan>,
    pub loop_result: Option<LoopResult>,
    pub specification_evaluation: Option<SpecificationEvaluation>,
    pub termination_reason: String,
    pub validation_error: Option<SpecificationValidationError>,
    pub action_policy: String,
}

impl ConstructionResult {
    pub fn is_completed(&self) -> bool {
        self.status == ConstructionStatus::Completed
    }

    pub fn tools_executed(&self) -> Vec<String> {
        self.loop_result
            .as_ref()
            .map(LoopResult::tools_executed)
            .unwrap_or_default()
    }

    pub fn iterations(&self) -> u32 {
        self.loop_result
            .as_ref()
            .map(|result| result.iterations)
            .unwrap_or(0)
    }
}

/// Orquestador: Specification → Plan → Artifact → AgentLoop → Evaluation → Result.
///
/// No implementa otro loop; delega en [`AgentLoop`].
pub struct AutonomousConstructionSession;

impl AutonomousConstructionSession {
    /// Ejecuta con [`ActionPolicy::default_session_policy`].
    pub fn run(config: AutonomousConstructionConfig, agent: &mut dyn Agent) -> ConstructionResult {
        Self::run_with_policy(config, agent, ActionPolicy::default_session_policy())
    }

    /// Ejecuta con ActionPolicy inyectada.
    pub fn run_with_policy(
        config: AutonomousConstructionConfig,
        agent: &mut dyn Agent,
        policy: ActionPolicy,
    ) -> ConstructionResult {
        let specification_id = config.specification.id.clone();
        let policy_name = policy.name().to_string();

        if let Err(error) = config.specification.validate() {
            return ConstructionResult {
                status: ConstructionStatus::InvalidSpecification,
                specification_id,
                artifact_id: None,
                final_artifact: None,
                build_plan: None,
                loop_result: None,
                specification_evaluation: None,
                termination_reason: format!("specification inválida: {error}"),
                validation_error: Some(error),
                action_policy: policy_name,
            };
        }

        let planned = match plan_specification(&config.specification) {
            Ok(plan) => plan,
            Err(SpecificationPlannerError::InvalidSpecification(error)) => {
                return ConstructionResult {
                    status: ConstructionStatus::InvalidSpecification,
                    specification_id,
                    artifact_id: None,
                    final_artifact: None,
                    build_plan: None,
                    loop_result: None,
                    specification_evaluation: None,
                    termination_reason: format!("planificación rechazada: {error}"),
                    validation_error: Some(error),
                    action_policy: policy_name,
                };
            }
        };

        let initial_artifact = RustArtifact::with_id(
            ArtifactId::new(format!("artifact:{}", specification_id.as_str())),
            config.artifact_name.clone(),
            config.initial_source.clone(),
        )
        .with_specification_id(specification_id.clone());
        let artifact_id = initial_artifact.id().clone();

        let goal = format!(
            "autonomous:{}:{}",
            specification_id.as_str(),
            config.specification.goal
        );
        let ctx = AgentContext::new(goal)
            .with_working_artifact(initial_artifact)
            .with_evaluation_specification(config.specification.clone());

        let harness = build_validate_compile_harness_with_policy(policy);
        let max_iterations = config.max_iterations.max(1);
        let loop_result = AgentLoop::new(max_iterations).run(&harness, agent, ctx);

        let evidence = &loop_result.history.evidence;
        let specification_evaluation =
            EvaluationEngine::new().evaluate_specification(&config.specification, evidence);

        let final_artifact = loop_result.final_context.working_artifact.clone();
        let status = resolve_status(&loop_result, &specification_evaluation);
        let termination_reason = match status {
            ConstructionStatus::Completed => format!(
                "specification {} satisfecha: {}",
                specification_id.as_str(),
                specification_evaluation.message
            ),
            ConstructionStatus::MaxIterations => loop_result.termination_reason.clone(),
            ConstructionStatus::Failed => format!(
                "construcción fallida: {} ({})",
                loop_result.termination_reason, specification_evaluation.message
            ),
            ConstructionStatus::InvalidSpecification => unreachable!("ya filtrado"),
        };

        ConstructionResult {
            status,
            specification_id,
            artifact_id: Some(artifact_id),
            final_artifact,
            build_plan: Some(planned),
            loop_result: Some(loop_result),
            specification_evaluation: Some(specification_evaluation),
            termination_reason,
            validation_error: None,
            action_policy: policy_name,
        }
    }

    /// Atajo: AiAgent + ModelClient + policy por defecto.
    pub fn run_with_model_client(
        config: AutonomousConstructionConfig,
        client: Box<dyn ModelClient>,
    ) -> ConstructionResult {
        Self::run_with_model_client_and_policy(
            config,
            client,
            ActionPolicy::default_session_policy(),
        )
    }

    pub fn run_with_model_client_and_policy(
        config: AutonomousConstructionConfig,
        client: Box<dyn ModelClient>,
        policy: ActionPolicy,
    ) -> ConstructionResult {
        let plan_kind = match plan_specification(&config.specification) {
            Ok(planned) => plan_kind_label(planned.plan.kind),
            Err(_) => "Generic".to_string(),
        };
        let session = AiSessionConfig {
            user_request: config.specification.goal.clone(),
            plan_kind,
        };
        let mut agent = AiAgent::new(client, session);
        Self::run_with_policy(config, &mut agent, policy)
    }
}

fn plan_kind_label(kind: PlanKind) -> String {
    match kind {
        PlanKind::Api => "Api".to_string(),
        PlanKind::Calculator => "Calculator".to_string(),
        PlanKind::Authentication => "Authentication".to_string(),
        PlanKind::Generic => "Generic".to_string(),
    }
}

fn resolve_status(
    loop_result: &LoopResult,
    evaluation: &SpecificationEvaluation,
) -> ConstructionStatus {
    match loop_result.status {
        LoopStatus::MaxIterations => ConstructionStatus::MaxIterations,
        LoopStatus::Failed => ConstructionStatus::Failed,
        LoopStatus::Running => ConstructionStatus::Failed,
        LoopStatus::Completed => match evaluation.status {
            SpecificationEvaluationStatus::Pass => ConstructionStatus::Completed,
            SpecificationEvaluationStatus::Fail => ConstructionStatus::Failed,
            SpecificationEvaluationStatus::InsufficientEvidence => ConstructionStatus::Failed,
        },
    }
}
