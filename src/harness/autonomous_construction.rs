//! Flujo experimental: Specification → Plan → Artifact → AgentLoop → ConstructionResult.
//!
//! No reemplaza [`crate::main::run_constructor`]. Reutiliza AgentLoop, ActionPolicy,
//! EvaluationEngine y el Harness de sesión (Validate / Repair / Correct / Compile).
//!
//! Initial Artifact: por defecto desde [`crate::builder::initial_artifact_definition_for_kind`]
//! (PlanKind → archivos deterministas con primary). El caller puede inyectar un source explícito
//! para tests (p. ej. defectos controlados).
//!
//! Observabilidad: [`ConstructionObservability`] es un resumen **derivado** de
//! LoopResult / Evidence / EvaluationEngine — no es una segunda fuente de verdad.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::builder;
use crate::harness::action_policy::ActionPolicy;
use crate::harness::agent::Agent;
use crate::harness::agent_loop::{AgentLoop, LoopResult, LoopStatus};
use crate::harness::ai_agent::AiAgent;
use crate::harness::artifact::{ArtifactId, RustArtifact};
use crate::harness::artifact_path::ArtifactPath;
use crate::harness::constraint::Constraint;
use crate::harness::context::AgentContext;
use crate::harness::criterion::CriterionKind;
use crate::harness::evaluation::EvaluationVerdict;
use crate::harness::evaluation_engine::{
    EvaluationEngine, SpecificationEvaluation, SpecificationEvaluationStatus,
};
use crate::harness::goal_driven::{Goal, GoalDrivenLoop, GoalDrivenResult, GoalDrivenStatus};
use crate::harness::live_session::build_validate_compile_harness_with_policy;
use crate::harness::model::{AiSessionConfig, ModelClient};
use crate::harness::observation::AgentObservation;
use crate::harness::specification::{Specification, SpecificationId, SpecificationValidationError};
use crate::harness::specification_planner::{
    SpecificationBuildPlan, SpecificationPlannerError, plan_specification,
};
use crate::planner::{BuildPlan, PlanKind};

/// Estado terminal de una construcción autónoma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructionStatus {
    Completed,
    Failed,
    MaxIterations,
    InvalidSpecification,
}

/// Resumen agregado de ejecuciones de una Tool (derivado de LoopHistory.steps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionSummary {
    pub tool_name: String,
    pub executions: u32,
    pub successes: u32,
    pub failures: u32,
}

/// Evaluación de un criterio en el timeline (derivada de CriterionEvaluated / EvaluationEngine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionObservabilityEntry {
    pub criterion_id: String,
    pub kind: CriterionKind,
    pub verdict: EvaluationVerdict,
}

/// Resumen observable de una construcción. Derivado; no muta el flujo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructionObservability {
    /// Duración total de la sesión (ms). Presente siempre; no afirmar valores exactos en tests.
    pub duration_ms: u64,
    /// Iteraciones reales de AgentLoop (`0` si no hubo loop).
    pub iteration_count: u32,
    /// Secuencia ordenada de Tools que se ejecutaron.
    pub tools_executed_sequence: Vec<String>,
    /// Conteos por Tool.
    pub tool_summaries: Vec<ToolExecutionSummary>,
    /// Timeline de CriterionEvaluated observados durante el loop.
    pub criterion_timeline: Vec<CriterionObservabilityEntry>,
    /// Verdict final por criterio (SpecificationEvaluation agregada).
    pub final_criteria: Vec<CriterionObservabilityEntry>,
    pub final_status: ConstructionStatus,
    pub termination_reason: String,
    /// Retries de transporte/modelo observados en la sesión, si hay fuente causal.
    ///
    /// - `Some(n)`: se inyectó [`crate::harness::ModelRetryObservability`] (n puede ser 0).
    /// - `None`: no hay fuente causal (p. ej. `run_with_model_client` sin handle).
    ///
    /// Ortogonal a [`Self::iteration_count`]. Re-ejecutar una Tool tras FAIL **no** es un retry.
    pub model_retry_count: Option<u32>,
}

impl ConstructionObservability {
    pub fn tool_execution_count(&self, tool_name: &str) -> u32 {
        self.tool_summaries
            .iter()
            .find(|item| item.tool_name == tool_name)
            .map(|item| item.executions)
            .unwrap_or(0)
    }

    pub fn criterion_verdicts(&self, criterion_id: &str) -> Vec<EvaluationVerdict> {
        self.criterion_timeline
            .iter()
            .filter(|item| item.criterion_id == criterion_id)
            .map(|item| item.verdict)
            .collect()
    }
}

/// Configuración de una sesión de construcción desde Specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomousConstructionConfig {
    pub specification: Specification,
    /// Override opcional del source inicial (tests / inyección).
    /// Si es `None`, el source se obtiene de [`builder::initial_source_for_kind`].
    pub initial_source: Option<String>,
    pub max_iterations: u32,
    /// Nombre del archivo del Artifact.
    pub artifact_name: String,
}

impl AutonomousConstructionConfig {
    /// Configuración estándar: el Initial Artifact lo produce el Builder desde el Plan.
    pub fn new(specification: Specification, max_iterations: u32) -> Self {
        Self {
            specification,
            initial_source: None,
            max_iterations,
            artifact_name: "main.rs".to_string(),
        }
    }

    /// Inyecta un source explícito (p. ej. código defectuoso en tests).
    pub fn with_initial_source(mut self, source: impl Into<String>) -> Self {
        self.initial_source = Some(source.into());
        self
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
    /// Resumen derivado de LoopResult / Evaluation (no altera la semántica).
    pub observability: ConstructionObservability,
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

/// Materializa un [`RustArtifact`] inicial a partir del plan (Builder determinista).
pub fn initial_artifact_from_plan(
    specification_id: SpecificationId,
    plan: &BuildPlan,
    artifact_name: impl Into<String>,
) -> RustArtifact {
    let definition = builder::initial_artifact_definition_for_kind(plan.kind);
    let artifact_id = ArtifactId::new(format!("artifact:{}", specification_id.as_str()));
    let artifact_name = artifact_name.into();

    let artifact = if definition.file_count() == 1 {
        RustArtifact::with_id(
            artifact_id,
            artifact_name,
            builder::initial_source_for_kind(plan.kind),
        )
    } else {
        let primary = ArtifactPath::parse(definition.primary_path)
            .expect("primary path del Builder debe ser válido");
        let files: Vec<(ArtifactPath, String)> = definition
            .files()
            .map(|(path, source)| {
                ArtifactPath::parse(path)
                    .map(|parsed| (parsed, source.to_string()))
                    .expect("path del Builder debe ser válido")
            })
            .collect();
        RustArtifact::try_from_files(artifact_id, artifact_name, primary, files)
            .expect("definición inicial del Builder debe ser válida")
    };

    artifact.with_specification_id(specification_id)
}

/// Resultado de construcción con capa goal-driven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalDrivenConstructionResult {
    pub construction: ConstructionResult,
    pub goal_result: Option<GoalDrivenResult>,
}

impl GoalDrivenConstructionResult {
    pub fn is_goal_satisfied(&self) -> bool {
        self.goal_result
            .as_ref()
            .map(GoalDrivenResult::is_goal_satisfied)
            .unwrap_or(false)
    }
}

fn invalid_specification_result(
    specification_id: SpecificationId,
    error: SpecificationValidationError,
    policy_name: String,
    started: Instant,
) -> ConstructionResult {
    let termination_reason = format!("specification inválida: {error}");
    let status = ConstructionStatus::InvalidSpecification;
    ConstructionResult {
        status,
        specification_id,
        artifact_id: None,
        final_artifact: None,
        build_plan: None,
        loop_result: None,
        specification_evaluation: None,
        termination_reason: termination_reason.clone(),
        validation_error: Some(error),
        action_policy: policy_name,
        observability: build_observability(
            status,
            started.elapsed().as_millis() as u64,
            None,
            None,
            &termination_reason,
            None,
        ),
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
        let policy_name = policy.name().to_string();
        let harness = build_validate_compile_harness_with_policy(policy);
        Self::run_with_harness(config, agent, policy_name, harness)
    }

    /// Misma semántica que [`Self::run_with_policy`], con Harness inyectado (tests / stubs).
    ///
    /// No introduce un segundo runtime: usa el mismo [`AgentLoop`] + [`crate::harness::Harness`].
    pub fn run_with_harness(
        config: AutonomousConstructionConfig,
        agent: &mut dyn Agent,
        policy_name: impl Into<String>,
        harness: crate::harness::runtime::Harness,
    ) -> ConstructionResult {
        Self::run_with_harness_and_retry_observability(config, agent, policy_name, harness, None)
    }

    /// Como [`Self::run_with_harness`], con inyección causal opcional de retries de modelo.
    pub fn run_with_harness_and_retry_observability(
        config: AutonomousConstructionConfig,
        agent: &mut dyn Agent,
        policy_name: impl Into<String>,
        harness: crate::harness::runtime::Harness,
        retry_observability: Option<crate::harness::ModelRetryObservability>,
    ) -> ConstructionResult {
        let started = Instant::now();
        let specification_id = config.specification.id.clone();
        let policy_name = policy_name.into();
        // Con handle: Some(total) incluso si es 0 ("fuente causal, sin retries").
        // Sin handle: None ("sin fuente causal").
        let retry_count_from_handle = || retry_observability.as_ref().map(|obs| obs.total());

        if let Err(error) = config.specification.validate() {
            let termination_reason = format!("specification inválida: {error}");
            let status = ConstructionStatus::InvalidSpecification;
            return ConstructionResult {
                status,
                specification_id,
                artifact_id: None,
                final_artifact: None,
                build_plan: None,
                loop_result: None,
                specification_evaluation: None,
                termination_reason: termination_reason.clone(),
                validation_error: Some(error),
                action_policy: policy_name,
                observability: build_observability(
                    status,
                    started.elapsed().as_millis() as u64,
                    None,
                    None,
                    &termination_reason,
                    retry_count_from_handle(),
                ),
            };
        }

        let planned = match plan_specification(&config.specification) {
            Ok(plan) => plan,
            Err(SpecificationPlannerError::InvalidSpecification(error)) => {
                let termination_reason = format!("planificación rechazada: {error}");
                let status = ConstructionStatus::InvalidSpecification;
                return ConstructionResult {
                    status,
                    specification_id,
                    artifact_id: None,
                    final_artifact: None,
                    build_plan: None,
                    loop_result: None,
                    specification_evaluation: None,
                    termination_reason: termination_reason.clone(),
                    validation_error: Some(error),
                    action_policy: policy_name,
                    observability: build_observability(
                        status,
                        started.elapsed().as_millis() as u64,
                        None,
                        None,
                        &termination_reason,
                        retry_count_from_handle(),
                    ),
                };
            }
        };

        let initial_artifact = match &config.initial_source {
            Some(source) => RustArtifact::with_id(
                ArtifactId::new(format!("artifact:{}", specification_id.as_str())),
                config.artifact_name.clone(),
                source.clone(),
            )
            .with_specification_id(specification_id.clone()),
            None => initial_artifact_from_plan(
                specification_id.clone(),
                &planned.plan,
                config.artifact_name.clone(),
            ),
        };
        let artifact_id = initial_artifact.id().clone();

        let goal = format!(
            "autonomous:{}:{}",
            specification_id.as_str(),
            config.specification.goal
        );
        let ctx = AgentContext::new(goal)
            .with_working_artifact(initial_artifact)
            .with_evaluation_specification(config.specification.clone());

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

        let observability = build_observability(
            status,
            started.elapsed().as_millis() as u64,
            Some(&loop_result),
            Some(&specification_evaluation),
            &termination_reason,
            retry_count_from_handle(),
        );

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
            observability,
        }
    }

    /// Ejecuta con ciclo goal-driven (evaluación → gap → acción → evidencia → re-evaluación).
    pub fn run_goal_driven(
        config: AutonomousConstructionConfig,
        agent: &mut dyn Agent,
    ) -> GoalDrivenConstructionResult {
        Self::run_goal_driven_with_policy(config, agent, ActionPolicy::default_session_policy())
    }

    /// Como [`Self::run_goal_driven`], con ActionPolicy inyectada.
    pub fn run_goal_driven_with_policy(
        config: AutonomousConstructionConfig,
        agent: &mut dyn Agent,
        policy: ActionPolicy,
    ) -> GoalDrivenConstructionResult {
        let policy_name = policy.name().to_string();
        let harness = build_validate_compile_harness_with_policy(policy);
        Self::run_goal_driven_with_harness(config, agent, policy_name, harness)
    }

    /// Goal-driven con Harness inyectado.
    pub fn run_goal_driven_with_harness(
        config: AutonomousConstructionConfig,
        agent: &mut dyn Agent,
        policy_name: impl Into<String>,
        harness: crate::harness::runtime::Harness,
    ) -> GoalDrivenConstructionResult {
        let started = Instant::now();
        let specification_id = config.specification.id.clone();
        let policy_name = policy_name.into();
        let goal = Goal::from_specification(config.specification.clone());

        if let Err(error) = config.specification.validate() {
            return GoalDrivenConstructionResult {
                construction: invalid_specification_result(
                    specification_id,
                    error,
                    policy_name,
                    started,
                ),
                goal_result: None,
            };
        }

        let planned = match plan_specification(&config.specification) {
            Ok(plan) => plan,
            Err(SpecificationPlannerError::InvalidSpecification(error)) => {
                return GoalDrivenConstructionResult {
                    construction: invalid_specification_result(
                        specification_id,
                        error,
                        policy_name,
                        started,
                    ),
                    goal_result: None,
                };
            }
        };

        let initial_artifact = match &config.initial_source {
            Some(source) => RustArtifact::with_id(
                ArtifactId::new(format!("artifact:{}", specification_id.as_str())),
                config.artifact_name.clone(),
                source.clone(),
            )
            .with_specification_id(specification_id.clone()),
            None => initial_artifact_from_plan(
                specification_id.clone(),
                &planned.plan,
                config.artifact_name.clone(),
            ),
        };

        let ctx = AgentContext::new(format!(
            "goal-driven:{}:{}",
            specification_id.as_str(),
            goal.description()
        ))
        .with_working_artifact(initial_artifact)
        .with_evaluation_specification(config.specification.clone());

        let max_iterations = config.max_iterations.max(1);
        let mut goal_loop = GoalDrivenLoop::with_defaults(max_iterations);
        let goal_result = goal_loop.run(&harness, agent, &goal, ctx);

        let evidence = &goal_result.loop_result.history.evidence;
        let specification_evaluation =
            EvaluationEngine::new().evaluate_specification(&config.specification, evidence);

        let status = match goal_result.status {
            GoalDrivenStatus::GoalSatisfied => ConstructionStatus::Completed,
            GoalDrivenStatus::MaxIterations => ConstructionStatus::MaxIterations,
            GoalDrivenStatus::Escalated
            | GoalDrivenStatus::NonProgress
            | GoalDrivenStatus::Failed => ConstructionStatus::Failed,
        };

        let termination_reason = goal_result.termination_reason.clone();
        let observability = build_observability(
            status,
            started.elapsed().as_millis() as u64,
            Some(&goal_result.loop_result),
            Some(&specification_evaluation),
            &termination_reason,
            None,
        );

        GoalDrivenConstructionResult {
            construction: ConstructionResult {
                status,
                specification_id,
                artifact_id: goal_result
                    .loop_result
                    .final_context
                    .working_artifact
                    .as_ref()
                    .map(|a| a.id().clone()),
                final_artifact: goal_result
                    .loop_result
                    .final_context
                    .working_artifact
                    .clone(),
                build_plan: Some(planned),
                loop_result: Some(goal_result.loop_result.clone()),
                specification_evaluation: Some(specification_evaluation),
                termination_reason,
                validation_error: None,
                action_policy: policy_name,
                observability,
            },
            goal_result: Some(goal_result),
        }
    }

    /// Atajo: AiAgent + ModelClient + policy por defecto (sin retry observability → `None`).
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
        Self::run_with_model_client_policy_and_retry_observability(config, client, policy, None)
    }

    /// Como [`Self::run_with_model_client`], con handle causal de retries.
    pub fn run_with_model_client_and_retry_observability(
        config: AutonomousConstructionConfig,
        client: Box<dyn ModelClient>,
        retry_observability: crate::harness::ModelRetryObservability,
    ) -> ConstructionResult {
        Self::run_with_model_client_policy_and_retry_observability(
            config,
            client,
            ActionPolicy::default_session_policy(),
            Some(retry_observability),
        )
    }

    /// AiAgent + policy + inyección causal opcional de retries de modelo.
    pub fn run_with_model_client_policy_and_retry_observability(
        config: AutonomousConstructionConfig,
        client: Box<dyn ModelClient>,
        policy: ActionPolicy,
        retry_observability: Option<crate::harness::ModelRetryObservability>,
    ) -> ConstructionResult {
        let plan_kind = match plan_specification(&config.specification) {
            Ok(planned) => plan_kind_label(planned.plan.kind),
            Err(_) => "Generic".to_string(),
        };
        let session = AiSessionConfig::new(config.specification.goal.clone(), plan_kind);
        let mut agent = AiAgent::new(client, session);
        let policy_name = policy.name().to_string();
        let harness = build_validate_compile_harness_with_policy(policy);
        Self::run_with_harness_and_retry_observability(
            config,
            &mut agent,
            policy_name,
            harness,
            retry_observability,
        )
    }
}

fn build_observability(
    final_status: ConstructionStatus,
    duration_ms: u64,
    loop_result: Option<&LoopResult>,
    evaluation: Option<&SpecificationEvaluation>,
    termination_reason: &str,
    model_retry_count: Option<u32>,
) -> ConstructionObservability {
    let iteration_count = loop_result.map(|result| result.iterations).unwrap_or(0);

    let tools_executed_sequence = loop_result
        .map(LoopResult::tools_executed)
        .unwrap_or_default();

    let mut counts: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    if let Some(result) = loop_result {
        for step in &result.history.steps {
            if !step.tool_executed {
                continue;
            }
            let Some(name) = &step.tool_name else {
                continue;
            };
            let entry = counts.entry(name.clone()).or_insert((0, 0, 0));
            entry.0 += 1;
            let success = step
                .tool_result
                .as_ref()
                .map(|tool| tool.success)
                .unwrap_or(false);
            if success {
                entry.1 += 1;
            } else {
                entry.2 += 1;
            }
        }
    }
    let tool_summaries = counts
        .into_iter()
        .map(
            |(tool_name, (executions, successes, failures))| ToolExecutionSummary {
                tool_name,
                executions,
                successes,
                failures,
            },
        )
        .collect();

    let mut criterion_timeline = Vec::new();
    if let Some(result) = loop_result {
        for observation in &result.history.observations {
            if let AgentObservation::CriterionEvaluated {
                criterion_id,
                kind,
                verdict,
                ..
            } = observation
            {
                criterion_timeline.push(CriterionObservabilityEntry {
                    criterion_id: criterion_id.as_str().to_string(),
                    kind: *kind,
                    verdict: *verdict,
                });
            }
        }
    }

    let final_criteria = evaluation
        .map(|aggregated| {
            aggregated
                .criteria
                .iter()
                .map(|item| CriterionObservabilityEntry {
                    criterion_id: item.criterion_id.as_str().to_string(),
                    kind: item.kind,
                    verdict: item.verdict,
                })
                .collect()
        })
        .unwrap_or_default();

    ConstructionObservability {
        duration_ms,
        iteration_count,
        tools_executed_sequence,
        tool_summaries,
        criterion_timeline,
        final_criteria,
        final_status,
        termination_reason: termination_reason.to_string(),
        model_retry_count,
    }
}

pub(crate) fn plan_kind_label(kind: PlanKind) -> String {
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
        LoopStatus::Failed | LoopStatus::Running | LoopStatus::NonProgress => {
            ConstructionStatus::Failed
        }
        LoopStatus::Completed => match evaluation.status {
            SpecificationEvaluationStatus::Pass => ConstructionStatus::Completed,
            SpecificationEvaluationStatus::Fail => ConstructionStatus::Failed,
            SpecificationEvaluationStatus::InsufficientEvidence => ConstructionStatus::Failed,
        },
    }
}
