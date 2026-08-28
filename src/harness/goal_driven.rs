//! Goal-Driven Autonomous Software Construction — capa mínima sobre arquitectura existente.
//!
//! Reutiliza [`Specification`], [`EvaluationEngine`], [`Evidence`], [`AgentLoop`] y
//! [`AgentObservation`] sin duplicar sistemas de evaluación ni Artifact canónico.
//!
//! Ciclo: RECEIVE GOAL → EVALUATE → IDENTIFY GAP → SELECT ACTION → EXECUTE →
//! COLLECT EVIDENCE → RE-EVALUATE → SATISFIED | LOOP | ESCALATE

use crate::harness::action::AgentAction;
use crate::harness::agent::Agent;
use crate::harness::agent_loop::{AgentLoop, LoopResult, LoopStatus};
use crate::harness::context::AgentContext;
use crate::harness::criterion::CriterionKind;
use crate::harness::evaluation::{EvaluationVerdict, Evidence};
use crate::harness::evaluation_engine::{
    EvaluationEngine, SpecificationEvaluation, SpecificationEvaluationStatus,
};
use crate::harness::evaluation_observation::{
    evaluate_tool_evidence, observation_from_specification_evaluation,
};
use crate::harness::observation::AgentObservation;
use crate::harness::runtime::Harness;
use crate::harness::specification::{AcceptanceCriterionId, Specification, SpecificationId};

/// Meta verificable: el RESULTADO deseado, no la próxima acción.
///
/// Envuelve [`Specification`] (goal + requirements + acceptance criteria).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub specification: Specification,
}

impl Goal {
    pub fn from_specification(specification: Specification) -> Self {
        Self { specification }
    }

    pub fn description(&self) -> &str {
        self.specification.goal.as_str()
    }

    pub fn id(&self) -> &SpecificationId {
        &self.specification.id
    }
}

/// Estado terminal de evaluación de una Goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Satisfied,
    Unsatisfied,
    Inconclusive,
}

impl GoalStatus {
    pub fn from_specification_status(status: SpecificationEvaluationStatus) -> Self {
        match status {
            SpecificationEvaluationStatus::Pass => Self::Satisfied,
            SpecificationEvaluationStatus::Fail => Self::Unsatisfied,
            SpecificationEvaluationStatus::InsufficientEvidence => Self::Inconclusive,
        }
    }
}

/// Plan de evaluación derivado de los Acceptance Criteria (criterion → kind → tool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationPlanEntry {
    pub criterion_id: AcceptanceCriterionId,
    pub kind: CriterionKind,
    pub description: String,
}

/// Estrategia de evaluación proporcional a la Goal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvaluationPlan {
    pub entries: Vec<EvaluationPlanEntry>,
}

impl EvaluationPlan {
    pub fn from_specification(spec: &Specification) -> Self {
        Self {
            entries: spec
                .acceptance_criteria
                .iter()
                .map(|criterion| EvaluationPlanEntry {
                    criterion_id: criterion.id.clone(),
                    kind: criterion.kind,
                    description: criterion.description.clone(),
                })
                .collect(),
        }
    }
}

/// Diferencia entre estado esperado y actual para un criterio insatisfecho.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionGap {
    pub criterion_id: AcceptanceCriterionId,
    pub kind: CriterionKind,
    pub verdict: EvaluationVerdict,
    pub message: String,
    /// Hipótesis de acción sugerida para cerrar el gap (medio, no fin).
    pub suggested_action: Option<&'static str>,
}

/// EXPECTED − CURRENT = GOAL GAP
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalGap {
    pub unsatisfied: Vec<CriterionGap>,
}

impl GoalGap {
    pub fn from_evaluation(evaluation: &SpecificationEvaluation) -> Self {
        let unsatisfied = evaluation
            .criteria
            .iter()
            .filter(|item| item.verdict != EvaluationVerdict::Pass)
            .map(|item| CriterionGap {
                criterion_id: item.criterion_id.clone(),
                kind: item.kind,
                verdict: item.verdict,
                message: item.message.clone(),
                suggested_action: suggested_tool_for_kind(item.kind),
            })
            .collect();
        Self { unsatisfied }
    }

    pub fn is_empty(&self) -> bool {
        self.unsatisfied.is_empty()
    }

    pub fn primary(&self) -> Option<&CriterionGap> {
        self.unsatisfied.first()
    }
}

fn suggested_tool_for_kind(kind: CriterionKind) -> Option<&'static str> {
    use crate::harness::tools::{CHECK_FORMAT, COMPILE, RUN_CLIPPY, RUN_TESTS, VALIDATE};
    match kind {
        CriterionKind::Compile => Some(COMPILE),
        CriterionKind::Validate => Some(VALIDATE),
        CriterionKind::RunTests => Some(RUN_TESTS),
        CriterionKind::Clippy => Some(RUN_CLIPPY),
        CriterionKind::CheckFormat => Some(CHECK_FORMAT),
        CriterionKind::Unknown => None,
    }
}

/// Resultado de evaluar Goal + Acceptance Criteria + Evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalEvaluation {
    pub goal_id: SpecificationId,
    pub status: GoalStatus,
    pub specification_evaluation: SpecificationEvaluation,
    pub gap: GoalGap,
    pub evaluation_plan: EvaluationPlan,
}

/// Evalúa si la meta está satisfecha usando [`EvaluationEngine`] existente.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoalEvaluator;

impl GoalEvaluator {
    pub fn new() -> Self {
        Self
    }

    pub fn evaluate(&self, goal: &Goal, evidence: &[Evidence]) -> GoalEvaluation {
        let specification_evaluation =
            EvaluationEngine::new().evaluate_specification(&goal.specification, evidence);
        let status = GoalStatus::from_specification_status(specification_evaluation.status);
        let gap = GoalGap::from_evaluation(&specification_evaluation);
        GoalEvaluation {
            goal_id: goal.specification.id.clone(),
            status,
            gap,
            evaluation_plan: EvaluationPlan::from_specification(&goal.specification),
            specification_evaluation,
        }
    }
}

/// Señal de progreso entre evaluaciones consecutivas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressSignal {
    Improved,
    Unchanged,
    Regressed,
}

/// Rastrea progreso medible y detecta ausencia de avance / regresiones.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalProgressTracker {
    pass_counts: Vec<usize>,
    fingerprints: Vec<String>,
    stale_iterations: u32,
}

impl GoalProgressTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, evaluation: &GoalEvaluation) -> ProgressSignal {
        let pass_count = evaluation
            .specification_evaluation
            .criteria
            .iter()
            .filter(|item| item.verdict == EvaluationVerdict::Pass)
            .count();
        let fingerprint = progress_fingerprint(evaluation);

        let signal = match self.pass_counts.last() {
            None => ProgressSignal::Unchanged,
            Some(prev) if pass_count > *prev => ProgressSignal::Improved,
            Some(prev) if pass_count < *prev => ProgressSignal::Regressed,
            Some(_) => {
                if self.fingerprints.last() == Some(&fingerprint) {
                    ProgressSignal::Unchanged
                } else {
                    ProgressSignal::Improved
                }
            }
        };

        if signal == ProgressSignal::Unchanged && !self.pass_counts.is_empty() {
            self.stale_iterations += 1;
        } else if signal == ProgressSignal::Improved {
            self.stale_iterations = 0;
        }

        self.pass_counts.push(pass_count);
        self.fingerprints.push(fingerprint);
        signal
    }

    pub fn stale_iterations(&self) -> u32 {
        self.stale_iterations
    }

    pub fn pass_count(&self) -> usize {
        self.pass_counts.last().copied().unwrap_or(0)
    }
}

fn progress_fingerprint(evaluation: &GoalEvaluation) -> String {
    evaluation
        .specification_evaluation
        .criteria
        .iter()
        .map(|item| format!("{}={:?}", item.criterion_id.as_str(), item.verdict))
        .collect::<Vec<_>>()
        .join("|")
}

/// Bloqueo escalado a humano con evidencia estructurada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalEscalation {
    pub goal_description: String,
    pub reason: String,
    pub evidence: Vec<Evidence>,
    pub attempts: u32,
    pub last_gap: GoalGap,
    pub stale_iterations: u32,
}

/// Estado terminal del loop orientado a Goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalDrivenStatus {
    GoalSatisfied,
    Failed,
    MaxIterations,
    Escalated,
}

/// Historial de evaluaciones de Goal durante el loop.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalDrivenHistory {
    pub evaluations: Vec<GoalEvaluation>,
    pub gaps: Vec<GoalGap>,
    pub progress_signals: Vec<ProgressSignal>,
}

/// Resultado del loop goal-driven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalDrivenResult {
    pub status: GoalDrivenStatus,
    pub loop_result: LoopResult,
    pub final_evaluation: GoalEvaluation,
    pub history: GoalDrivenHistory,
    pub escalation: Option<GoalEscalation>,
    pub termination_reason: String,
}

impl GoalDrivenResult {
    pub fn is_goal_satisfied(&self) -> bool {
        matches!(self.status, GoalDrivenStatus::GoalSatisfied)
    }
}

/// Loop que evoluciona AgentLoop con evaluación de Goal, gap analysis y anti-loop.
pub struct GoalDrivenLoop {
    inner: AgentLoop,
    evaluator: GoalEvaluator,
    progress: GoalProgressTracker,
    max_stale_iterations: u32,
}

impl GoalDrivenLoop {
    pub fn new(max_iterations: u32, max_stale_iterations: u32) -> Self {
        Self {
            inner: AgentLoop::new(max_iterations),
            evaluator: GoalEvaluator::new(),
            progress: GoalProgressTracker::new(),
            max_stale_iterations: max_stale_iterations.max(1),
        }
    }

    pub fn with_defaults(max_iterations: u32) -> Self {
        Self::new(max_iterations, 3)
    }

    /// Ejecuta el ciclo goal-driven completo.
    pub fn run(
        &mut self,
        harness: &Harness,
        agent: &mut dyn Agent,
        goal: &Goal,
        mut ctx: AgentContext,
    ) -> GoalDrivenResult {
        ctx.evaluation_specification = Some(goal.specification.clone());

        let mut history = GoalDrivenHistory::default();
        let initial_evidence = collect_evidence_from_observations(&ctx);
        let initial = self.evaluator.evaluate(goal, &initial_evidence);
        history.evaluations.push(initial.clone());
        history.gaps.push(initial.gap.clone());
        history
            .progress_signals
            .push(self.progress.record(&initial));

        if initial.status == GoalStatus::Satisfied {
            let loop_result = empty_satisfied_loop_result(ctx.clone());
            return GoalDrivenResult {
                status: GoalDrivenStatus::GoalSatisfied,
                loop_result,
                final_evaluation: initial,
                history,
                escalation: None,
                termination_reason: "goal satisfecha antes del loop (evidencia preexistente)"
                    .to_string(),
            };
        }

        ctx.push_observation(goal_observation(&initial));

        let loop_result = self.inner.run(harness, agent, ctx);
        let evidence = &loop_result.history.evidence;

        let mut final_evaluation = self.evaluator.evaluate(goal, evidence);
        history.evaluations.push(final_evaluation.clone());
        history.gaps.push(final_evaluation.gap.clone());
        let progress = self.progress.record(&final_evaluation);
        history.progress_signals.push(progress);

        if progress == ProgressSignal::Regressed {
            final_evaluation.status = GoalStatus::Unsatisfied;
        }

        let escalation = check_escalation(
            goal,
            &loop_result,
            &final_evaluation,
            &self.progress,
            self.max_stale_iterations,
        );

        let status = resolve_goal_status(&loop_result, &final_evaluation, escalation.is_some());
        let termination_reason = match &status {
            GoalDrivenStatus::GoalSatisfied => format!(
                "goal `{}` satisfecha: {}",
                goal.id().as_str(),
                final_evaluation.specification_evaluation.message
            ),
            GoalDrivenStatus::Escalated => escalation
                .as_ref()
                .map(|item| item.reason.clone())
                .unwrap_or_else(|| "escalación sin detalle".to_string()),
            GoalDrivenStatus::MaxIterations => loop_result.termination_reason.clone(),
            GoalDrivenStatus::Failed => format!(
                "goal no alcanzada: {} ({})",
                loop_result.termination_reason, final_evaluation.specification_evaluation.message
            ),
        };

        GoalDrivenResult {
            status,
            loop_result,
            final_evaluation,
            history,
            escalation,
            termination_reason,
        }
    }

    /// Re-evalúa Goal tras una acción (para tests y coordinación externa).
    pub fn re_evaluate(&self, goal: &Goal, evidence: &[Evidence]) -> GoalEvaluation {
        self.evaluator.evaluate(goal, evidence)
    }
}

fn empty_satisfied_loop_result(ctx: AgentContext) -> LoopResult {
    LoopResult {
        status: LoopStatus::Completed,
        iterations: 0,
        history: Default::default(),
        final_context: ctx,
        termination_reason: "goal pre-satisfecha".to_string(),
    }
}

fn goal_observation(evaluation: &GoalEvaluation) -> AgentObservation {
    observation_from_specification_evaluation(&evaluation.specification_evaluation)
}

fn check_escalation(
    goal: &Goal,
    loop_result: &LoopResult,
    evaluation: &GoalEvaluation,
    progress: &GoalProgressTracker,
    max_stale: u32,
) -> Option<GoalEscalation> {
    if evaluation.status == GoalStatus::Satisfied {
        return None;
    }

    let stale = progress.stale_iterations() >= max_stale;
    let exhausted = loop_result.status == LoopStatus::MaxIterations;
    let no_hypothesis = evaluation
        .gap
        .unsatisfied
        .iter()
        .all(|gap| gap.kind == CriterionKind::Unknown || gap.suggested_action.is_none());

    if !stale && !exhausted && !no_hypothesis {
        return None;
    }

    let reason = if stale {
        format!(
            "sin progreso durante {} evaluaciones consecutivas",
            progress.stale_iterations()
        )
    } else if exhausted {
        "máximo de iteraciones alcanzado sin satisfacer la goal".to_string()
    } else {
        "no existen acciones deterministas para los criterios pendientes".to_string()
    };

    Some(GoalEscalation {
        goal_description: goal.description().to_string(),
        reason,
        evidence: loop_result.history.evidence.clone(),
        attempts: loop_result.iterations,
        last_gap: evaluation.gap.clone(),
        stale_iterations: progress.stale_iterations(),
    })
}

fn resolve_goal_status(
    loop_result: &LoopResult,
    evaluation: &GoalEvaluation,
    escalated: bool,
) -> GoalDrivenStatus {
    if escalated {
        return GoalDrivenStatus::Escalated;
    }
    if evaluation.status == GoalStatus::Satisfied && loop_result.status == LoopStatus::Completed {
        return GoalDrivenStatus::GoalSatisfied;
    }
    match loop_result.status {
        LoopStatus::MaxIterations => GoalDrivenStatus::MaxIterations,
        LoopStatus::Failed | LoopStatus::Running => GoalDrivenStatus::Failed,
        LoopStatus::Completed => GoalDrivenStatus::Failed,
    }
}

/// Agent determinista orientado a gap: selecciona acción según criterio insatisfecho.
///
/// OBSERVATION → HYPOTHESIS (gap) → ACTION → MEASUREMENT (re-evaluación vía loop).
pub struct GapDrivenAgent {
    pub request: String,
    pub attempted_actions: Vec<String>,
}

impl GapDrivenAgent {
    pub fn new(request: impl Into<String>) -> Self {
        Self {
            request: request.into(),
            attempted_actions: Vec::new(),
        }
    }

    fn action_for_gap(&mut self, gap: &CriterionGap, ctx: &AgentContext) -> AgentAction {
        let label = gap.suggested_action.unwrap_or("unknown").to_string();
        self.attempted_actions.push(label.clone());

        match gap.kind {
            CriterionKind::Compile => AgentAction::Compile {
                code: ctx
                    .working_code()
                    .map(str::to_string)
                    .unwrap_or_else(|| "fn main() {}".to_string()),
            },
            CriterionKind::Validate => AgentAction::Validate {
                request: self.request.clone(),
                code: ctx.working_code().map(str::to_string),
                plan_kind: "Api".to_string(),
            },
            CriterionKind::RunTests => AgentAction::RunTests {
                filter: String::new(),
            },
            CriterionKind::Clippy => AgentAction::RunClippy,
            CriterionKind::CheckFormat => AgentAction::CheckFormat,
            CriterionKind::Unknown => AgentAction::Finish {
                summary: format!("sin acción para criterio `{}`", gap.criterion_id.as_str()),
            },
        }
    }
}

impl Agent for GapDrivenAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        if let Some(AgentObservation::SpecificationEvaluated {
            status: SpecificationEvaluationStatus::Pass,
            ..
        }) = &ctx.last_observation
        {
            return AgentAction::Finish {
                summary: "goal satisfied".to_string(),
            };
        }

        if let Some(AgentObservation::CriterionEvaluated {
            verdict: EvaluationVerdict::Pass,
            ..
        }) = &ctx.last_observation
        {
            if let Some(spec) = ctx.evaluation_specification.as_ref() {
                let evaluation = GoalEvaluator::new().evaluate(
                    &Goal::from_specification(spec.clone()),
                    &collect_evidence_from_observations(ctx),
                );
                if evaluation.status == GoalStatus::Satisfied {
                    return AgentAction::Finish {
                        summary: "goal satisfied".to_string(),
                    };
                }
                if let Some(gap) = evaluation.gap.primary() {
                    return self.action_for_gap(gap, ctx);
                }
            }
            return AgentAction::Finish {
                summary: "criterio pass pero goal incompleta".to_string(),
            };
        }

        if let Some(AgentObservation::CriterionEvaluated {
            verdict: EvaluationVerdict::Fail,
            kind: CriterionKind::Compile,
            evidence,
            message,
            ..
        }) = &ctx.last_observation
        {
            let mut errors: Vec<String> = evidence
                .iter()
                .filter(|item| item.label == "compiler_stderr")
                .map(|item| item.detail.clone())
                .collect();
            if errors.is_empty() {
                errors.push(message.clone());
            }
            return AgentAction::RepairDiagnostic { errors };
        }

        if let Some(spec) = ctx.evaluation_specification.as_ref() {
            let evaluation = GoalEvaluator::new().evaluate(
                &Goal::from_specification(spec.clone()),
                &collect_evidence_from_observations(ctx),
            );
            if let Some(gap) = evaluation.gap.primary() {
                return self.action_for_gap(gap, ctx);
            }
        }

        if let Some(code) = ctx.working_code() {
            AgentAction::Compile {
                code: code.to_string(),
            }
        } else {
            AgentAction::Finish {
                summary: format!("sin artifact para goal: {}", self.request),
            }
        }
    }
}

fn collect_evidence_from_observations(ctx: &AgentContext) -> Vec<Evidence> {
    let mut evidence = Vec::new();
    for observation in &ctx.observation_history {
        if let AgentObservation::ToolOutcome { evidence: ev, .. } = observation {
            evidence.extend(ev.clone());
        }
        if let AgentObservation::CriterionEvaluated { evidence: ev, .. } = observation {
            evidence.extend(ev.clone());
        }
    }
    evidence
}

/// Coordinador incremental: evalúa Goal tras evidencia de Tool (para loop manual).
pub fn evaluate_after_tool(
    goal: &Goal,
    tool_name: &str,
    evidence: &[Evidence],
) -> Option<(GoalEvaluation, AgentObservation)> {
    let step = evaluate_tool_evidence(&goal.specification, tool_name, evidence)?;
    let evaluation = GoalEvaluator::new().evaluate(goal, evidence);
    let observation = AgentObservation::CriterionEvaluated {
        specification_id: goal.specification.id.clone(),
        criterion_id: step.evaluation.criterion_id.clone(),
        kind: step.evaluation.kind,
        verdict: step.evaluation.verdict,
        message: step.evaluation.message.clone(),
        evidence: step.evaluation.evidence_used.clone(),
    };
    Some((evaluation, observation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::EvaluationEngine;
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::live_session::build_validate_compile_harness_with_policy;
    use crate::harness::specification::{AcceptanceCriterion, Requirement};
    use crate::harness::tool::Tool;
    use crate::harness::tool::ToolResult;
    use crate::harness::tool_permission::ToolPermissionConstraint;
    use crate::harness::tools::{COMPILE, CompileTool, RepairDiagnosticTool, VALIDATE};

    fn compile_only_goal() -> Goal {
        Goal::from_specification(
            Specification::new("spec-goal-compile", "El código debe compilar")
                .with_requirements(vec![Requirement::new("req-c", "compilar")])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                        .satisfying([crate::harness::RequirementId::new("req-c")]),
                ]),
        )
    }

    fn compile_and_validate_goal() -> Goal {
        Goal::from_specification(
            Specification::new("spec-goal-both", "API REST funcional")
                .with_requirements(vec![Requirement::new("req-q", "calidad")])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new("ac-v", "valida", CriterionKind::Validate)
                        .satisfying([crate::harness::RequirementId::new("req-q")]),
                    AcceptanceCriterion::new("ac-c", "compila", CriterionKind::Compile)
                        .satisfying([crate::harness::RequirementId::new("req-q")]),
                ]),
        )
    }

    #[test]
    fn goal_satisfied_immediately_without_loop() {
        // A
        let goal = compile_only_goal();
        let evidence = vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
        ];
        let evaluation = GoalEvaluator::new().evaluate(&goal, &evidence);
        assert_eq!(evaluation.status, GoalStatus::Satisfied);
        assert!(evaluation.gap.is_empty());

        let mut loop_ = GoalDrivenLoop::with_defaults(5);
        let harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());
        let mut agent = GapDrivenAgent::new("compilar");
        let mut ctx = AgentContext::new("goal-immediate")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(goal.specification.clone());
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: true,
            output: "ok".to_string(),
            evidence,
            verdict: EvaluationVerdict::Pass,
        });

        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert_eq!(result.status, GoalDrivenStatus::GoalSatisfied);
        assert_eq!(result.loop_result.iterations, 0);
    }

    #[test]
    fn goal_initially_unsatisfied_identifies_gap() {
        // B
        let goal = compile_only_goal();
        let evaluation = GoalEvaluator::new().evaluate(&goal, &[]);
        assert_eq!(evaluation.status, GoalStatus::Inconclusive);
        assert_eq!(evaluation.gap.unsatisfied.len(), 1);
        assert_eq!(
            evaluation.gap.primary().unwrap().criterion_id.as_str(),
            "ac-compile"
        );
        assert_eq!(
            evaluation.gap.primary().unwrap().kind,
            CriterionKind::Compile
        );
    }

    #[test]
    fn action_produces_evidence() {
        // C
        let tool = CompileTool;
        let ctx = AgentContext::new("evidence").with_working_code("fn main() {}");
        let result = tool.execute("", &ctx);
        assert!(!result.evidence.is_empty());
        assert!(result.evidence.iter().any(|e| e.label == "compile_status"));
    }

    #[test]
    fn re_evaluation_after_action() {
        // D
        let goal = compile_only_goal();
        let tool = CompileTool;
        let ctx = AgentContext::new("re-eval").with_working_code("fn main() {}");
        let tool_result = tool.execute("", &ctx);
        let before = GoalEvaluator::new().evaluate(&goal, &[]);
        assert_ne!(before.status, GoalStatus::Satisfied);
        let after = GoalEvaluator::new().evaluate(&goal, &tool_result.evidence);
        assert_eq!(after.status, GoalStatus::Satisfied);
    }

    #[test]
    fn goal_reached_after_multiple_actions() {
        // E
        let broken = "fn main() { println!(\"x\"";
        let goal = compile_only_goal();

        let mut harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());
        harness.register_tool(Box::new(RepairDiagnosticTool));

        let mut agent = GapDrivenAgent::new("compilar");
        let mut loop_ = GoalDrivenLoop::with_defaults(8);
        let ctx = AgentContext::new("multi-action")
            .with_working_code(broken)
            .with_evaluation_specification(goal.specification.clone());

        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert!(
            result.final_evaluation.status == GoalStatus::Satisfied
                || result.loop_result.iterations >= 2,
            "debe ejecutar múltiples acciones: {:?}",
            result.status
        );
    }

    #[test]
    fn failed_action_is_not_success() {
        // F
        let goal = compile_only_goal();
        let tool = CompileTool;
        let ctx = AgentContext::new("fail").with_working_code("fn main() { broken");
        let tool_result = tool.execute("", &ctx);
        assert!(!tool_result.success);
        let evaluation = GoalEvaluator::new().evaluate(&goal, &tool_result.evidence);
        assert_eq!(evaluation.status, GoalStatus::Unsatisfied);
        assert_ne!(evaluation.status, GoalStatus::Satisfied);
    }

    #[test]
    fn no_progress_triggers_stale_detection() {
        // G
        let mut tracker = GoalProgressTracker::new();
        let goal = compile_only_goal();
        let eval1 = GoalEvaluator::new().evaluate(&goal, &[]);
        let eval2 = GoalEvaluator::new().evaluate(&goal, &[]);
        assert_eq!(tracker.record(&eval1), ProgressSignal::Unchanged);
        assert_eq!(tracker.record(&eval2), ProgressSignal::Unchanged);
        assert_eq!(tracker.record(&eval2), ProgressSignal::Unchanged);
        assert!(tracker.stale_iterations() >= 2);
    }

    #[test]
    fn escalation_includes_evidence() {
        // H
        let goal = Goal::from_specification(
            Specification::new("spec-unknown", "meta no evaluable")
                .with_requirements(vec![Requirement::new("req-u", "endpoint")])
                .with_acceptance_criteria(vec![
                    AcceptanceCriterion::new(
                        "ac-unknown",
                        "GET /health responde 200",
                        CriterionKind::Unknown,
                    )
                    .satisfying([crate::harness::RequirementId::new("req-u")]),
                ]),
        );

        let mut loop_ = GoalDrivenLoop::new(2, 1);
        let harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());
        let mut agent = GapDrivenAgent::new("endpoint");
        let ctx = AgentContext::new("escalate")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(goal.specification.clone());

        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert!(
            matches!(
                result.status,
                GoalDrivenStatus::Escalated | GoalDrivenStatus::MaxIterations
            ),
            "debe escalar o agotar iteraciones: {:?}",
            result.status
        );
        if let Some(escalation) = &result.escalation {
            assert!(!escalation.goal_description.is_empty());
            assert!(!escalation.reason.is_empty());
        }
    }

    #[test]
    fn regression_detection_prevents_false_success() {
        // I
        let goal = compile_and_validate_goal();
        let mut evidence = vec![
            Evidence::new("tool", VALIDATE),
            Evidence::new("validate_status", "ok"),
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
        ];
        let pass = GoalEvaluator::new().evaluate(&goal, &evidence);
        assert_eq!(pass.status, GoalStatus::Satisfied);

        evidence.pop(); // remove compile_status ok
        evidence.push(Evidence::new("compile_status", "error"));
        let regressed = GoalEvaluator::new().evaluate(&goal, &evidence);
        assert_eq!(regressed.status, GoalStatus::Unsatisfied);

        let mut tracker = GoalProgressTracker::new();
        tracker.record(&pass);
        assert_eq!(tracker.record(&regressed), ProgressSignal::Regressed);
    }

    #[test]
    fn evaluation_plan_maps_criteria_to_methods() {
        let goal = compile_and_validate_goal();
        let plan = EvaluationPlan::from_specification(&goal.specification);
        assert_eq!(plan.entries.len(), 2);
        assert!(
            plan.entries
                .iter()
                .any(|e| e.kind == CriterionKind::Validate)
        );
        assert!(
            plan.entries
                .iter()
                .any(|e| e.kind == CriterionKind::Compile)
        );
    }

    #[test]
    fn goal_evaluator_reuses_evaluation_engine() {
        let goal = compile_only_goal();
        let evidence = vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
        ];
        let engine_eval =
            EvaluationEngine::new().evaluate_specification(&goal.specification, &evidence);
        let goal_eval = GoalEvaluator::new().evaluate(&goal, &evidence);
        assert_eq!(
            goal_eval.specification_evaluation.status,
            engine_eval.status
        );
        assert_eq!(goal_eval.status, GoalStatus::Satisfied);
    }

    struct AlwaysFailCompileTool;

    impl Tool for AlwaysFailCompileTool {
        fn name(&self) -> &str {
            COMPILE
        }

        fn execute(&self, _input: &str, ctx: &AgentContext) -> ToolResult {
            ctx.append_artifact_evidence(&mut vec![]);
            crate::harness::tool::ToolResult::failure(
                "always fails".to_string(),
                vec![
                    Evidence::new("tool", COMPILE),
                    Evidence::new("compile_status", "error"),
                    Evidence::new("compiler_stderr", "forced failure"),
                ],
            )
        }
    }

    #[test]
    fn gap_driven_agent_targets_unsatisfied_criterion() {
        let goal = compile_only_goal();
        let mut agent = GapDrivenAgent::new("compilar");
        let ctx = AgentContext::new("gap-agent")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(goal.specification.clone());
        let action = agent.propose(&ctx);
        assert!(matches!(action, AgentAction::Compile { .. }));
    }

    #[test]
    fn goal_pre_satisfied_skips_agent_loop() {
        let goal = compile_only_goal();
        let mut loop_ = GoalDrivenLoop::with_defaults(5);
        let harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());
        let mut ctx = AgentContext::new("pre-satisfied")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(goal.specification.clone());
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: COMPILE.to_string(),
            success: true,
            output: "ok".to_string(),
            evidence: vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
            ],
            verdict: EvaluationVerdict::Pass,
        });

        let mut agent = GapDrivenAgent::new("noop");
        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert_eq!(result.status, GoalDrivenStatus::GoalSatisfied);
        assert_eq!(result.loop_result.iterations, 0);
    }
}
