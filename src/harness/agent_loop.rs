use std::sync::Arc;

use crate::harness::action::AgentAction;
use crate::harness::agent::Agent;
use crate::harness::context::AgentContext;
use crate::harness::evaluation::{Evaluation, Evidence};
use crate::harness::evaluation_engine::CriterionEvaluation;
use crate::harness::evaluation_observation::evaluate_tool_evidence;
use crate::harness::failure_classification::{
    FailureEvidence, FailureReport, RecoveryBudget, RecoveryDelay, RecoveryStrategy,
    SharedRecoveryDelay, build_failure_report, classify_progress_stall, classify_system_failure,
    default_recovery_delay, plan_recovery,
};
use crate::harness::goal_driven::{
    Goal, GoalEvaluator, GoalProgressTracker, GoalStatus, ProgressAssessment, ProgressSignal,
    collect_evidence_from_context,
};
use crate::harness::observation::AgentObservation;
use crate::harness::runtime::{Harness, StepOutcome};
use crate::harness::tool::ToolResult;

/// Estado de terminación del Agent Loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStatus {
    Running,
    Completed,
    Failed,
    MaxIterations,
    /// Ventana de no-progreso agotada sin causa más específica demostrable.
    NonProgress,
    /// Servicio externo bloqueó la ejecución (p. ej. rate limit agotado).
    ExternalServiceBlocked,
    /// Fallo permanente de configuración/credenciales del servicio modelo.
    ExternalConfigurationBlocked,
    /// Decisiones del modelo sin progreso medible (servicio OK).
    ModelCapabilityFailure,
    /// Fallo interno del sistema autónomo.
    SystemFailure,
}

/// Historial observable de una ejecución del loop.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoopHistory {
    pub proposed_actions: Vec<AgentAction>,
    pub executed_actions: Vec<AgentAction>,
    pub rejected_actions: Vec<(AgentAction, String)>,
    pub tool_results: Vec<ToolResult>,
    pub evidence: Vec<Evidence>,
    pub evaluations: Vec<Evaluation>,
    /// Evaluaciones de AcceptanceCriterion producidas tras Tools (VERIFY).
    pub criterion_evaluations: Vec<CriterionEvaluation>,
    pub observations: Vec<AgentObservation>,
    pub steps: Vec<StepOutcome>,
    /// Assessments de progreso goal-driven (vacío si no hay specification / stale tracking).
    pub progress_assessments: Vec<ProgressAssessment>,
    /// Informes de clasificación/recovery emitidos durante la corrida.
    pub failure_reports: Vec<FailureReport>,
    /// Decisiones de routing multi-modelo (Stay/Wait/Switch/Escalate/Stop).
    pub routing_decisions: Vec<crate::harness::model_routing::RoutingDecision>,
}

/// Resultado estructurado del Agent Loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopResult {
    pub status: LoopStatus,
    pub iterations: u32,
    pub history: LoopHistory,
    pub final_context: AgentContext,
    pub termination_reason: String,
    /// Último informe de fallo clasificado, si aplica.
    pub failure_report: Option<FailureReport>,
}

impl LoopResult {
    pub fn tools_executed(&self) -> Vec<String> {
        self.history
            .steps
            .iter()
            .filter(|step| step.tool_executed)
            .filter_map(|step| step.tool_name.clone())
            .collect()
    }
}

/// Loop genérico Observe → Decide → Harness → Observe [→ Evaluate → Observe].
///
/// No conoce Rust, Compiler, Validator ni proveedores de IA.
/// El límite de iteraciones es responsabilidad del loop, no del Agent.
/// EvaluationEngine se invoca solo como coordinación opcional tras Tools.
pub struct AgentLoop {
    max_iterations: u32,
    /// Si `Some`, evalúa progreso de Goal tras cada paso con specification.
    max_stale_iterations: Option<u32>,
    recovery_budget_template: RecoveryBudget,
    recovery_delay: SharedRecoveryDelay,
}

impl AgentLoop {
    pub fn new(max_iterations: u32) -> Self {
        assert!(
            max_iterations > 0,
            "max_iterations del AgentLoop debe ser >= 1"
        );
        Self {
            max_iterations,
            max_stale_iterations: None,
            recovery_budget_template: RecoveryBudget::default(),
            recovery_delay: default_recovery_delay(),
        }
    }

    /// Activa detección de no-progreso (default productivo: 3).
    pub fn with_max_stale_iterations(mut self, max_stale_iterations: u32) -> Self {
        self.max_stale_iterations = Some(max_stale_iterations.max(1));
        self
    }

    /// Configura el presupuesto de recovery transitorio (intentos + backoff).
    ///
    /// Tests: usar `Duration::ZERO` (default) para evitar sleeps reales.
    pub fn with_recovery_budget(mut self, budget: RecoveryBudget) -> Self {
        self.recovery_budget_template = budget;
        self
    }

    /// Inyecta abstracción de espera (p. ej. recording delay en tests).
    pub fn with_recovery_delay(mut self, delay: Arc<dyn RecoveryDelay>) -> Self {
        self.recovery_delay = delay;
        self
    }

    pub fn max_iterations(&self) -> u32 {
        self.max_iterations
    }

    pub fn run(
        &self,
        harness: &Harness,
        agent: &mut dyn Agent,
        mut ctx: AgentContext,
    ) -> LoopResult {
        let mut history = LoopHistory::default();
        let mut status = LoopStatus::Running;
        let mut iterations = 0;
        let mut termination_reason = String::new();
        let mut progress = GoalProgressTracker::new();
        let mut recovery_budget = self.recovery_budget_template;
        let mut failure_report: Option<FailureReport> = None;
        let mut meaningful_progress_observed = false;
        let mut recovered_after_failure = false;

        while iterations < self.max_iterations {
            iterations += 1;
            ctx.step = iterations;

            let action = agent.propose(&ctx);
            history.proposed_actions.push(action.clone());

            let outcome = harness.execute_step(action, &mut ctx);
            history.steps.push(outcome.clone());
            history.evaluations.push(outcome.evaluation.clone());
            history.evidence.extend(outcome.evidence.clone());
            history.observations.push(outcome.observation.clone());

            if let Some(reason) = &outcome.rejected_reason {
                history
                    .rejected_actions
                    .push((outcome.action.clone(), reason.clone()));
            } else {
                history.executed_actions.push(outcome.action.clone());
            }

            if let Some(tool_result) = outcome.tool_result.clone() {
                history.tool_results.push(tool_result);
            }

            // Fallo estructurado del Agent (ModelError / response) — antes de stale/Finish genérico.
            if let Some(mut evidence) = agent.last_failure_evidence() {
                evidence.failed_action =
                    outcome.action.tool_name().map(str::to_string).or_else(|| {
                        match &outcome.action {
                            AgentAction::Finish { .. } => Some("finish".to_string()),
                            AgentAction::NoOp => Some("noop".to_string()),
                            _ => None,
                        }
                    });
                match self.handle_classified_failure(
                    agent,
                    evidence,
                    &mut recovery_budget,
                    &mut history,
                    meaningful_progress_observed,
                    &mut recovered_after_failure,
                ) {
                    FailureHandle::Recover => continue,
                    FailureHandle::Stop {
                        loop_status,
                        reason,
                        report,
                    } => {
                        status = loop_status;
                        termination_reason = reason;
                        failure_report = Some(report);
                        break;
                    }
                }
            }

            if let AgentObservation::UnknownTool { tool_name } = &outcome.observation {
                let evidence = classify_system_failure(
                    format!("herramienta no registrada: {tool_name}"),
                    Some(tool_name.clone()),
                );
                match self.handle_classified_failure(
                    agent,
                    evidence,
                    &mut recovery_budget,
                    &mut history,
                    meaningful_progress_observed,
                    &mut recovered_after_failure,
                ) {
                    FailureHandle::Recover => continue,
                    FailureHandle::Stop {
                        loop_status,
                        reason,
                        report,
                    } => {
                        status = loop_status;
                        termination_reason = reason;
                        failure_report = Some(report);
                        break;
                    }
                }
            }

            if let AgentObservation::Finished { summary } = &outcome.observation {
                let failed = summary.to_ascii_lowercase().contains("fail")
                    || summary.to_ascii_lowercase().contains("error");
                if failed {
                    status = LoopStatus::Failed;
                    termination_reason = format!("finish con fallo: {summary}");
                } else {
                    status = LoopStatus::Completed;
                    termination_reason = format!("finish: {summary}");
                }
                break;
            }

            // Coordinación VERIFY: Tool Evidence → EvaluationEngine → Observation.
            if outcome.tool_executed
                && let (Some(tool_name), Some(specification)) = (
                    outcome.tool_name.as_deref(),
                    ctx.evaluation_specification.as_ref(),
                )
                && let Some(step) =
                    evaluate_tool_evidence(specification, tool_name, &outcome.evidence)
            {
                history.criterion_evaluations.push(step.evaluation.clone());
                history.observations.push(step.observation.clone());
                ctx.push_observation(step.observation);
            }

            if let Some(max_stale) = self.max_stale_iterations
                && let Some(specification) = ctx.evaluation_specification.as_ref()
            {
                let goal = Goal::from_specification(specification.clone());
                let evaluation =
                    GoalEvaluator::new().evaluate(&goal, &collect_evidence_from_context(&ctx));
                if evaluation.status == GoalStatus::Satisfied {
                    continue;
                }
                let action_label = outcome.action.tool_name().map(str::to_string).or_else(|| {
                    if matches!(outcome.action, AgentAction::Finish { .. }) {
                        Some("finish".to_string())
                    } else {
                        Some("noop".to_string())
                    }
                });
                let artifact_revision = ctx
                    .working_artifact
                    .as_ref()
                    .map(|artifact| artifact.revision());
                let assessment = progress.record_iteration(
                    &evaluation,
                    action_label.as_deref(),
                    artifact_revision,
                );
                history.progress_assessments.push(assessment.clone());
                if assessment.signal == ProgressSignal::Improved {
                    meaningful_progress_observed = true;
                    recovered_after_failure = false;
                }

                let stagnating = progress.stale_iterations() >= max_stale
                    || (assessment.repeated_action && progress.stale_iterations() >= max_stale);
                if stagnating {
                    let tool_executed_recently = history
                        .steps
                        .iter()
                        .rev()
                        .take(max_stale as usize)
                        .any(|step| step.tool_executed);
                    let evidence = classify_progress_stall(&assessment, tool_executed_recently);
                    match self.handle_classified_failure(
                        agent,
                        evidence,
                        &mut recovery_budget,
                        &mut history,
                        meaningful_progress_observed,
                        &mut recovered_after_failure,
                    ) {
                        FailureHandle::Recover => continue,
                        FailureHandle::Stop {
                            loop_status,
                            reason,
                            report,
                        } => {
                            status = loop_status;
                            termination_reason = reason;
                            failure_report = Some(report);
                            break;
                        }
                    }
                }
            }
        }

        if status == LoopStatus::Running {
            status = LoopStatus::MaxIterations;
            termination_reason =
                format!("máximo de iteraciones alcanzado ({})", self.max_iterations);
            history.evaluations.push(Evaluation::fail(
                termination_reason.clone(),
                vec![Evidence::new(
                    "max_iterations",
                    self.max_iterations.to_string(),
                )],
            ));
            history.evidence.push(Evidence::new(
                "max_iterations",
                self.max_iterations.to_string(),
            ));
        }

        LoopResult {
            status,
            iterations,
            history,
            final_context: ctx,
            termination_reason,
            failure_report,
        }
    }

    fn handle_classified_failure(
        &self,
        agent: &mut dyn Agent,
        evidence: FailureEvidence,
        budget: &mut RecoveryBudget,
        history: &mut LoopHistory,
        meaningful_progress_observed: bool,
        recovered_after_failure: &mut bool,
    ) -> FailureHandle {
        // Recovery externo no registra progreso de Goal: este path hace continue
        // antes de GoalProgressTracker::record_iteration (evita contaminar NonProgress).
        let decision = plan_recovery(&evidence, budget);
        if decision.strategy.is_recover() && budget.consume() {
            self.recovery_delay.delay(decision.wait);
            *recovered_after_failure = true;
            let report = build_failure_report(
                &evidence,
                &decision,
                budget.attempts_used,
                false,
                meaningful_progress_observed,
            );
            history.failure_reports.push(report);
            // Observabilidad de routing: ExternalTransient → WaitSameModel (mismo modelo).
            if let Some(route) =
                agent.try_route_after_failure(&evidence, meaningful_progress_observed)
            {
                history
                    .evidence
                    .push(Evidence::new("model_routing", route.summary()));
                history.routing_decisions.push(route);
            }
            history.evidence.push(Evidence::new(
                "failure_recovery",
                format!(
                    "class={} strategy={} reason={} wait_ms={} attempt={}/{} signal={}",
                    evidence.class.as_str(),
                    decision.strategy.as_str(),
                    decision.reason.as_str(),
                    decision.wait.as_millis(),
                    budget.attempts_used,
                    budget.max_attempts,
                    decision.signal.summary()
                ),
            ));
            return FailureHandle::Recover;
        }

        // Antes de terminal: intentar Switch/Escalate si el agente tiene catálogo.
        if let Some(route) = agent.try_route_after_failure(&evidence, meaningful_progress_observed)
        {
            history
                .evidence
                .push(Evidence::new("model_routing", route.summary()));
            let changed = route.action.changes_model();
            history.routing_decisions.push(route);
            if changed {
                *recovered_after_failure = true;
                history.evidence.push(Evidence::new(
                    "failure_recovery",
                    format!(
                        "class={} strategy=model_route reason=routing_applied signal={}",
                        evidence.class.as_str(),
                        evidence.signal.summary()
                    ),
                ));
                return FailureHandle::Recover;
            }
        }

        let terminal_decision = if decision.strategy.is_recover() {
            let mut blocked = decision.clone();
            blocked.strategy = RecoveryStrategy::StopExternalBlocked;
            blocked.wait = std::time::Duration::ZERO;
            blocked.reason =
                crate::harness::failure_classification::RecoveryPlanReason::BudgetExhausted;
            blocked
        } else {
            decision
        };
        let report = build_failure_report(
            &evidence,
            &terminal_decision,
            budget.attempts_used,
            *recovered_after_failure && meaningful_progress_observed,
            meaningful_progress_observed,
        );
        history.failure_reports.push(report.clone());
        history.evidence.push(Evidence::new(
            "failure_terminal",
            report.terminal_explanation(),
        ));
        FailureHandle::Stop {
            loop_status: loop_status_for_strategy(terminal_decision.strategy),
            reason: format!(
                "{}: {}",
                evidence.class.as_str(),
                report.terminal_explanation()
            ),
            report,
        }
    }
}

enum FailureHandle {
    Recover,
    Stop {
        loop_status: LoopStatus,
        reason: String,
        report: FailureReport,
    },
}

fn loop_status_for_strategy(strategy: RecoveryStrategy) -> LoopStatus {
    match strategy {
        RecoveryStrategy::StopExternalBlocked
        | RecoveryStrategy::RetryWithBackoff
        | RecoveryStrategy::WaitThenRetry => LoopStatus::ExternalServiceBlocked,
        RecoveryStrategy::StopConfigurationBlocked => LoopStatus::ExternalConfigurationBlocked,
        RecoveryStrategy::StopModelCapability => LoopStatus::ModelCapabilityFailure,
        RecoveryStrategy::StopSystemFailure => LoopStatus::SystemFailure,
        RecoveryStrategy::StopConvergenceStalled => LoopStatus::NonProgress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Evidence;
    use crate::harness::agent::{
        FailThenStopAgent, NeverFinishAgent, ObservationDrivenEchoAgent, PermittedThenFinishAgent,
        RejectedThenFinishAgent, ValidateThenRepairAgent,
    };
    use crate::harness::bridge::introduce_validation_defect;
    use crate::harness::tool::Tool;
    use crate::harness::tool_permission::ToolPermissionConstraint;
    use crate::harness::tools::{
        COMPILE, CompileTool, CorrectionTool, REPAIR_DIAGNOSTIC, RepairDiagnosticTool, VALIDATE,
        ValidationTool,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn execute(&self, input: &str, _ctx: &AgentContext) -> ToolResult {
            ToolResult::success(input.to_string(), vec![Evidence::new("echo_output", input)])
        }
    }

    struct TrackingTool {
        name: &'static str,
        executed: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }

    impl Tool for TrackingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn execute(&self, input: &str, _ctx: &AgentContext) -> ToolResult {
            self.executed.store(true, Ordering::SeqCst);
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success(
                input.to_string(),
                vec![Evidence::new("tracking", self.name)],
            )
        }
    }

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

    #[test]
    fn agent_loop_executes_permitted_action() {
        // A
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = PermittedThenFinishAgent::new("fn main() { println!(\"ok\"); }\n");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("A"));

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(
            result
                .history
                .executed_actions
                .iter()
                .any(|a| matches!(a, AgentAction::Compile { .. }))
        );
        assert!(result.tools_executed().contains(&COMPILE.to_string()));
    }

    #[test]
    fn agent_loop_rejects_unauthorized_action() {
        // B
        let mut harness = Harness::new(10);
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = RejectedThenFinishAgent::new("echo");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("B"));

        assert!(!result.history.rejected_actions.is_empty());
        assert!(
            result.history.rejected_actions[0]
                .1
                .contains("herramienta no autorizada")
        );
        assert_eq!(result.status, LoopStatus::Completed);
    }

    #[test]
    fn rejected_action_does_not_execute_tool_in_loop() {
        // C
        let executed = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(TrackingTool {
            name: "echo",
            executed: Arc::clone(&executed),
            calls: Arc::clone(&calls),
        }));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = RejectedThenFinishAgent::new("echo");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("C"));

        assert!(!executed.load(Ordering::SeqCst));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(
            result
                .history
                .steps
                .iter()
                .any(|s| !s.tool_executed && s.rejected_reason.is_some())
        );
    }

    #[test]
    fn tool_result_becomes_observation_for_agent() {
        // D
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(EchoTool));

        let mut agent = ObservationDrivenEchoAgent::new();
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("D"));

        assert!(agent.saw_first_observation);
        assert!(
            result
                .final_context
                .observation_history
                .iter()
                .any(|o| matches!(
                    o,
                    AgentObservation::ToolOutcome {
                        output,
                        success: true,
                        ..
                    } if output == "first"
                ))
        );
    }

    #[test]
    fn agent_changes_decision_based_on_observation() {
        // E — causalidad observation → decision
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(EchoTool));

        let mut agent = ObservationDrivenEchoAgent::new();
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("E"));

        assert_eq!(agent.second_input.as_deref(), Some("after:first"));
        assert!(result.history.proposed_actions.iter().any(|a| matches!(
            a,
            AgentAction::InvokeTool { input, .. } if input == "after:first"
        )));
        assert_ne!(
            result.history.proposed_actions[0],
            result.history.proposed_actions[1]
        );
        assert_eq!(result.status, LoopStatus::Completed);
    }

    #[test]
    fn agent_loop_runs_multiple_iterations() {
        // F
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = PermittedThenFinishAgent::new("fn main() {}\n");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("F"));

        assert!(result.iterations >= 2);
        assert!(result.history.proposed_actions.len() >= 2);
    }

    #[test]
    fn agent_loop_completes_on_finish() {
        // G
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = PermittedThenFinishAgent::new("fn main() { let _x = 1; }\n");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("G"));

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(result.termination_reason.starts_with("finish:"));
    }

    #[test]
    fn agent_loop_terminates_on_failure() {
        // H
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = FailThenStopAgent::new("fn main() { println!(\"x\"\n");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("H"));

        assert_eq!(result.status, LoopStatus::Failed);
        assert!(result.termination_reason.contains("fail"));
    }

    #[test]
    fn agent_loop_never_exceeds_max_iterations() {
        // I
        let max = 3;
        let harness = Harness::new(100);
        let mut agent = NeverFinishAgent;
        let result = AgentLoop::new(max).run(&harness, &mut agent, AgentContext::new("I"));

        assert_eq!(result.status, LoopStatus::MaxIterations);
        assert_eq!(result.iterations, max);
        assert!(result.iterations <= max);
        assert_eq!(result.history.proposed_actions.len(), max as usize);
    }

    #[test]
    fn loop_history_preserves_actions_results_and_evidence() {
        // J
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = PermittedThenFinishAgent::new("fn main() { println!(\"hist\"); }\n");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("J"));

        assert!(!result.history.proposed_actions.is_empty());
        assert!(!result.history.executed_actions.is_empty());
        assert!(!result.history.tool_results.is_empty());
        assert!(!result.history.evidence.is_empty());
        assert!(!result.history.evaluations.is_empty());
        assert!(!result.history.observations.is_empty());
        assert!(!result.history.steps.is_empty());
    }

    #[test]
    fn agent_loop_integration_uses_compile_tool() {
        // K
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = PermittedThenFinishAgent::new("fn main() { println!(\"K\"); }\n");
        let result = AgentLoop::new(5).run(&harness, &mut agent, AgentContext::new("K"));

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(result.tools_executed().iter().any(|t| t == COMPILE));
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::ToolOutcome {
                tool_name,
                success: true,
                ..
            } if tool_name == COMPILE
        )));
    }

    #[test]
    fn agent_loop_integration_uses_validation_tool_with_repair() {
        // FAIL → RepairDiagnostic → ApplyCorrection → Validate PASS → Finish
        let invalid = introduce_validation_defect(&api_valid_code());
        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let mut agent = ValidateThenRepairAgent::new("Crear una API REST", "Api", invalid.clone());
        let result = AgentLoop::new(8).run(&harness, &mut agent, AgentContext::new("L"));

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(result.iterations >= 5);
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
                .any(|t| t == crate::harness::tools::APPLY_CORRECTION)
        );

        let validate_outcomes: Vec<_> = result
            .history
            .observations
            .iter()
            .filter_map(|o| match o {
                AgentObservation::ToolOutcome {
                    tool_name, success, ..
                } if tool_name == VALIDATE => Some(*success),
                _ => None,
            })
            .collect();

        assert_eq!(validate_outcomes.first(), Some(&false));
        assert_eq!(validate_outcomes.get(1), Some(&true));

        assert!(matches!(
            result.history.proposed_actions[0],
            AgentAction::Validate { code: Some(_), .. }
        ));
        assert!(matches!(
            result.history.proposed_actions[1],
            AgentAction::RepairDiagnostic { .. }
        ));
        assert!(matches!(
            result.history.proposed_actions[2],
            AgentAction::ApplyCorrection { .. }
        ));
        assert!(matches!(
            result.history.proposed_actions[3],
            AgentAction::Validate { code: Some(_), .. }
        ));
        assert!(agent.proposed_corrections.is_some());
    }

    #[test]
    fn evaluation_fail_in_loop_changes_next_agent_action() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_observation::EvaluationAwareAgent;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-loop-fail", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let broken = "fn main() { println!(\"x\"";
        let ctx = AgentContext::new("eval-loop-fail")
            .with_working_code(broken)
            .with_evaluation_specification(spec);

        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let result = AgentLoop::new(5).run(&harness, &mut agent, ctx);

        assert!(matches!(
            result.history.proposed_actions[0],
            AgentAction::Compile { .. }
        ));
        assert!(matches!(
            result.history.proposed_actions[1],
            AgentAction::RepairDiagnostic { .. }
        ));
        assert!(
            result
                .history
                .criterion_evaluations
                .iter()
                .any(|e| e.verdict == EvaluationVerdict::Fail)
        );
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Fail,
                kind: CriterionKind::Compile,
                ..
            }
        )));
    }

    #[test]
    fn evaluation_pass_in_loop_changes_next_agent_action() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_observation::EvaluationAwareAgent;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-loop-pass", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let ctx = AgentContext::new("eval-loop-pass")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);

        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let result = AgentLoop::new(5).run(&harness, &mut agent, ctx);

        assert_eq!(result.status, LoopStatus::Completed);
        assert!(matches!(
            result.history.proposed_actions[0],
            AgentAction::Compile { .. }
        ));
        assert!(matches!(
            &result.history.proposed_actions[1],
            AgentAction::Finish { summary } if summary.contains("pass")
        ));
        assert!(
            result
                .history
                .criterion_evaluations
                .iter()
                .any(|e| e.verdict == EvaluationVerdict::Pass)
        );
        assert!(!matches!(
            result.history.proposed_actions[1],
            AgentAction::RepairDiagnostic { .. }
        ));
    }

    #[test]
    fn insufficient_evidence_in_loop_is_not_pass() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_observation::EvaluationAwareAgent;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
        use crate::harness::tool::ToolResult;

        struct IncompleteCompileTool;

        impl Tool for IncompleteCompileTool {
            fn name(&self) -> &str {
                COMPILE
            }

            fn execute(&self, _input: &str, _ctx: &AgentContext) -> ToolResult {
                ToolResult::success(
                    "incomplete".to_string(),
                    vec![Evidence::new("tool", COMPILE)],
                )
            }
        }

        let spec = Specification::new("spec-loop-insuf", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(IncompleteCompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let ctx = AgentContext::new("eval-loop-insuf")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);

        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let result = AgentLoop::new(5).run(&harness, &mut agent, ctx);

        assert!(
            result
                .history
                .criterion_evaluations
                .iter()
                .any(|e| e.verdict == EvaluationVerdict::InsufficientEvidence)
        );
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::InsufficientEvidence,
                ..
            }
        )));
        assert!(matches!(
            &result.history.proposed_actions[1],
            AgentAction::Finish { summary } if summary.contains("insufficient")
        ));
    }

    #[test]
    fn loop_preserves_traceability_ids_kind_and_evidence() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_observation::EvaluationAwareAgent;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        let spec = Specification::new("spec-trace", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-xyz", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let ctx = AgentContext::new("eval-trace")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);

        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let result = AgentLoop::new(5).run(&harness, &mut agent, ctx);

        let evaluation = result
            .history
            .criterion_evaluations
            .first()
            .expect("criterion evaluation");
        assert_eq!(evaluation.criterion_id.as_str(), "ac-xyz");
        assert_eq!(evaluation.kind, CriterionKind::Compile);
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
        assert!(
            evaluation
                .evidence_used
                .iter()
                .any(|e| e.label == "compile_status")
        );

        match result
            .history
            .observations
            .iter()
            .find(|o| matches!(o, AgentObservation::CriterionEvaluated { .. }))
        {
            Some(AgentObservation::CriterionEvaluated {
                specification_id,
                criterion_id,
                kind,
                evidence,
                ..
            }) => {
                assert_eq!(specification_id.as_str(), "spec-trace");
                assert_eq!(criterion_id.as_str(), "ac-xyz");
                assert_eq!(*kind, CriterionKind::Compile);
                assert!(!evidence.is_empty());
            }
            other => panic!("expected CriterionEvaluated, got {other:?}"),
        }
    }

    #[test]
    fn multiple_criteria_identity_preserved_via_evaluate_tool_evidence() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_observation::evaluate_tool_evidence;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
        use crate::harness::tools::{COMPILE, RUN_TESTS, VALIDATE};

        let spec = Specification::new("spec-multi-loop", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-1", "calidad")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-1")]),
                AcceptanceCriterion::new("ac-v", "valida", CriterionKind::Validate)
                    .satisfying([crate::harness::RequirementId::new("req-1")]),
                AcceptanceCriterion::new("ac-t", "tests", CriterionKind::RunTests)
                    .satisfying([crate::harness::RequirementId::new("req-1")]),
            ]);

        let compile_ev = vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
        ];
        let validate_ev = vec![
            Evidence::new("tool", VALIDATE),
            Evidence::new("validate_status", "ok"),
        ];
        let tests_ev = vec![
            Evidence::new("tool", RUN_TESTS),
            Evidence::new("exit_status", "101"),
        ];

        let c = evaluate_tool_evidence(&spec, COMPILE, &compile_ev).expect("compile");
        let v = evaluate_tool_evidence(&spec, VALIDATE, &validate_ev).expect("validate");
        let t = evaluate_tool_evidence(&spec, RUN_TESTS, &tests_ev).expect("tests");

        assert_eq!(c.evaluation.criterion_id.as_str(), "ac-c");
        assert_eq!(c.evaluation.verdict, EvaluationVerdict::Pass);
        assert_eq!(v.evaluation.criterion_id.as_str(), "ac-v");
        assert_eq!(v.evaluation.verdict, EvaluationVerdict::Pass);
        assert_eq!(t.evaluation.criterion_id.as_str(), "ac-t");
        assert_eq!(t.evaluation.kind, CriterionKind::RunTests);
        assert_eq!(t.evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn evaluation_engine_remains_independent_of_agent_loop() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_engine::EvaluationEngine;
        use crate::harness::specification::AcceptanceCriterion;
        use crate::harness::tools::COMPILE;

        let evaluation = EvaluationEngine::new().evaluate_criterion(
            &AcceptanceCriterion::new("ac-1", "c", CriterionKind::Compile),
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn ai_agent_receives_evaluation_observation_in_model_request() {
        use crate::harness::ai_agent::AiAgent;
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_observation::observation_from_criterion_evaluation;
        use crate::harness::model::{AiSessionConfig, MockModelClient};
        use crate::harness::specification::{AcceptanceCriterion, SpecificationId};
        use crate::harness::tools::COMPILE;

        let evaluation = crate::harness::EvaluationEngine::new().evaluate_criterion(
            &AcceptanceCriterion::new("ac-1", "c", CriterionKind::Compile),
            &[
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "error"),
            ],
        );
        let observation =
            observation_from_criterion_evaluation(SpecificationId::new("spec-ai"), &evaluation);

        let mut ctx = AgentContext::new("ai-eval").with_working_code("fn main() {}");
        ctx.push_observation(observation);

        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());
        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let _ = agent.propose(&ctx);

        assert!(!agent.trace.requests.is_empty());
        let request = &agent.trace.requests[0];
        let last = request.last_observation.as_ref().expect("observation");
        assert_eq!(last.kind, "criterion_evaluated");
        assert_eq!(last.evaluation_verdict.as_deref(), Some("Fail"));
        assert_eq!(last.specification_id.as_deref(), Some("spec-ai"));
        assert_eq!(last.criterion_id.as_deref(), Some("ac-1"));
        assert_eq!(last.criterion_kind.as_deref(), Some("Compile"));
        assert!(!last.evidence_labels.is_empty());
        assert!(last.summary.contains("verdict=Fail"));
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn without_evaluation_observation_agent_does_not_hardcode_repair() {
        use crate::harness::evaluation_observation::EvaluationAwareAgent;

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let broken = "fn main() { println!(\"x\"";
        let ctx = AgentContext::new("no-eval").with_working_code(broken);
        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let result = AgentLoop::new(5).run(&harness, &mut agent, ctx);

        assert!(matches!(
            result.history.proposed_actions[0],
            AgentAction::Compile { .. }
        ));
        assert!(
            !matches!(
                result.history.proposed_actions[1],
                AgentAction::RepairDiagnostic { .. }
            ),
            "sin CriterionEvaluated el Agent no debe inventar RepairDiagnostic"
        );
        assert!(result.history.criterion_evaluations.is_empty());
    }

    #[test]
    fn evaluation_cycle_respects_max_iterations() {
        use crate::harness::agent::Agent;
        use crate::harness::criterion::CriterionKind;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        struct AlwaysCompileAgent;

        impl Agent for AlwaysCompileAgent {
            fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
                AgentAction::Compile {
                    code: ctx
                        .working_code()
                        .map(str::to_string)
                        .unwrap_or_else(|| "fn main() {}".to_string()),
                }
            }
        }

        let spec = Specification::new("spec-max", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let ctx = AgentContext::new("eval-max")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);

        let mut agent = AlwaysCompileAgent;
        let result = AgentLoop::new(3).run(&harness, &mut agent, ctx);
        assert_eq!(result.status, LoopStatus::MaxIterations);
        assert_eq!(result.iterations, 3);
        assert!(!result.history.criterion_evaluations.is_empty());
    }

    #[test]
    fn ai_agent_does_not_execute_tools_during_propose() {
        use crate::harness::ai_agent::AiAgent;
        use crate::harness::model::{AiSessionConfig, MockModelClient};

        let executed = Arc::new(AtomicBool::new(false));
        let mut harness = Harness::new(2);
        harness.register_tool(Box::new(TrackingTool {
            name: COMPILE,
            executed: Arc::clone(&executed),
            calls: Arc::new(AtomicUsize::new(0)),
        }));

        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());
        let mut agent = AiAgent::new(Box::new(MockModelClient::new()), session);
        let action = agent.propose(&AgentContext::new("no-tool").with_working_code("fn main() {}"));
        assert!(!executed.load(Ordering::SeqCst));
        assert!(matches!(action, AgentAction::Validate { .. }));
        let _ = harness;
    }

    #[test]
    fn e2e_tool_evaluation_observation_agent_harness_cycle() {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::evaluation::EvaluationVerdict;
        use crate::harness::evaluation_observation::EvaluationAwareAgent;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
        use crate::harness::specification_planner::plan_specification;

        let spec = Specification::new("spec-e2e-loop", "Crear una API REST")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "El código compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ]);
        let planned = plan_specification(&spec).expect("plan");
        assert_eq!(planned.specification_id, spec.id);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let broken = "fn main() { println!(\"broken\"";
        let ctx = AgentContext::new("e2e-loop")
            .with_working_code(broken)
            .with_evaluation_specification(spec);

        let mut agent = EvaluationAwareAgent::new("Crear una API REST");
        let result = AgentLoop::new(6).run(&harness, &mut agent, ctx);

        assert!(result.tools_executed().iter().any(|t| t == COMPILE));
        assert!(
            result
                .tools_executed()
                .iter()
                .any(|t| t == REPAIR_DIAGNOSTIC),
            "RepairDiagnostic debe ejecutarse tras Evaluation FAIL"
        );
        assert!(
            result
                .history
                .observations
                .iter()
                .any(|o| matches!(o, AgentObservation::ToolOutcome { .. }))
        );
        assert!(result.history.observations.iter().any(|o| matches!(
            o,
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Fail,
                ..
            }
        )));
        assert_ne!(
            result.history.proposed_actions[0],
            result.history.proposed_actions[1]
        );
    }
}
