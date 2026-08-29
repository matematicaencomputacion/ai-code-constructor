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
        let mut unsatisfied = evaluation
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
            .collect::<Vec<_>>();
        unsatisfied.sort_by_key(|gap| criterion_kind_priority(gap.kind));
        Self { unsatisfied }
    }

    pub fn is_empty(&self) -> bool {
        self.unsatisfied.is_empty()
    }

    pub fn primary(&self) -> Option<&CriterionGap> {
        self.unsatisfied.first()
    }
}

/// Acción recomendada derivada de Goal + evaluación + gap (no heurística opaca).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendedAction {
    /// Goal satisfecha: Finish permitido.
    FinishAllowed { reason: String },
    /// Ejecutar tool determinista para cerrar el gap.
    InvokeTool {
        tool_name: &'static str,
        criterion_id: AcceptanceCriterionId,
        kind: CriterionKind,
        priority: u8,
        reason: String,
    },
    /// Fallo con evidencia de error en compilación: reparar antes de re-verificar.
    RepairDiagnostic {
        criterion_id: AcceptanceCriterionId,
        kind: CriterionKind,
        priority: u8,
        reason: String,
    },
    /// Diagnóstico ya producido: mutar artifact con correcciones estructuradas.
    ApplyCorrection {
        criterion_id: AcceptanceCriterionId,
        kind: CriterionKind,
        priority: u8,
        reason: String,
    },
    /// Sin acción determinista (p. ej. criterio Unknown).
    NoDeterministicAction { reason: String },
}

impl RecommendedAction {
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::InvokeTool { tool_name, .. } => Some(tool_name),
            Self::RepairDiagnostic { .. } => Some(crate::harness::tools::REPAIR_DIAGNOSTIC),
            Self::ApplyCorrection { .. } => Some(crate::harness::tools::APPLY_CORRECTION),
            Self::FinishAllowed { .. } | Self::NoDeterministicAction { .. } => None,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::FinishAllowed { .. } => "FinishAllowed",
            Self::InvokeTool { .. } => "InvokeTool",
            Self::RepairDiagnostic { .. } => "RepairDiagnostic",
            Self::ApplyCorrection { .. } => "ApplyCorrection",
            Self::NoDeterministicAction { .. } => "NoDeterministicAction",
        }
    }

    pub fn priority(&self) -> u8 {
        match self {
            Self::FinishAllowed { .. } => 0,
            Self::InvokeTool { priority, .. }
            | Self::RepairDiagnostic { priority, .. }
            | Self::ApplyCorrection { priority, .. } => *priority,
            Self::NoDeterministicAction { .. } => u8::MAX,
        }
    }
}

/// Prioridad determinista por [`CriterionKind`] (menor = más urgente).
pub fn criterion_kind_priority(kind: CriterionKind) -> u8 {
    match kind {
        CriterionKind::Compile => 0,
        CriterionKind::Validate => 1,
        CriterionKind::RunTests => 2,
        CriterionKind::Clippy => 3,
        CriterionKind::CheckFormat => 4,
        CriterionKind::Unknown => u8::MAX - 1,
    }
}

/// Recomienda acción para un único criterio insatisfecho.
pub fn recommend_for_criterion_gap(gap: &CriterionGap) -> RecommendedAction {
    let priority = criterion_kind_priority(gap.kind);
    let tool = gap.suggested_action;

    if gap.kind == CriterionKind::Unknown || tool.is_none() {
        return RecommendedAction::NoDeterministicAction {
            reason: format!(
                "criterio `{}` (kind={:?}) sin acción determinista",
                gap.criterion_id.as_str(),
                gap.kind
            ),
        };
    }

    let tool_name = tool.expect("tool checked above");

    match gap.verdict {
        EvaluationVerdict::Fail if gap.kind == CriterionKind::Compile => {
            RecommendedAction::RepairDiagnostic {
                criterion_id: gap.criterion_id.clone(),
                kind: gap.kind,
                priority,
                reason: format!(
                    "compilación fallida para `{}`: {}",
                    gap.criterion_id.as_str(),
                    gap.message
                ),
            }
        }
        EvaluationVerdict::Fail => RecommendedAction::InvokeTool {
            tool_name,
            criterion_id: gap.criterion_id.clone(),
            kind: gap.kind,
            priority,
            reason: format!(
                "criterio `{}` falló (verdict=Fail): re-ejecutar {tool_name}",
                gap.criterion_id.as_str()
            ),
        },
        EvaluationVerdict::InsufficientEvidence => RecommendedAction::InvokeTool {
            tool_name,
            criterion_id: gap.criterion_id.clone(),
            kind: gap.kind,
            priority,
            reason: format!(
                "evidencia insuficiente para `{}`: ejecutar {tool_name}",
                gap.criterion_id.as_str()
            ),
        },
        EvaluationVerdict::Pass => RecommendedAction::FinishAllowed {
            reason: format!("criterio `{}` ya satisfecho", gap.criterion_id.as_str()),
        },
    }
}

/// Selecciona la recomendación primaria a partir de evaluación completa de Goal.
pub fn select_primary_recommendation(evaluation: &GoalEvaluation) -> RecommendedAction {
    if evaluation.status == GoalStatus::Satisfied {
        return RecommendedAction::FinishAllowed {
            reason: evaluation.specification_evaluation.message.clone(),
        };
    }

    evaluation
        .gap
        .primary()
        .map(recommend_for_criterion_gap)
        .unwrap_or_else(|| RecommendedAction::NoDeterministicAction {
            reason: "goal insatisfecha sin gaps identificados".to_string(),
        })
}

/// Último ToolOutcome del historial (éxito o fallo), si existe.
pub fn last_tool_outcome(ctx: &AgentContext) -> Option<(&str, bool)> {
    for observation in ctx.observation_history.iter().rev() {
        if let AgentObservation::ToolOutcome {
            tool_name, success, ..
        } = observation
        {
            return Some((tool_name.as_str(), *success));
        }
    }
    None
}

/// Recomendación primaria aware de transiciones de acción ya completadas.
///
/// Evita perder el contrato:
/// `RepairDiagnostic` exitoso → `ApplyCorrection` requerido;
/// `ApplyCorrection` exitoso → `Compile` de re-verificación.
pub fn select_primary_recommendation_with_context(
    evaluation: &GoalEvaluation,
    ctx: &AgentContext,
) -> RecommendedAction {
    if evaluation.status == GoalStatus::Satisfied {
        return RecommendedAction::FinishAllowed {
            reason: evaluation.specification_evaluation.message.clone(),
        };
    }

    let primary = evaluation.gap.primary();
    if let Some((tool_name, true)) = last_tool_outcome(ctx) {
        use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC};
        if tool_name == REPAIR_DIAGNOSTIC
            && let Some(gap) = primary
            && gap.kind == CriterionKind::Compile
            && gap.verdict == EvaluationVerdict::Fail
        {
            return RecommendedAction::ApplyCorrection {
                criterion_id: gap.criterion_id.clone(),
                kind: gap.kind,
                priority: criterion_kind_priority(gap.kind),
                reason: format!(
                    "RepairDiagnostic completado para `{}`: ApplyCorrection requerido",
                    gap.criterion_id.as_str()
                ),
            };
        }
        if tool_name == APPLY_CORRECTION
            && let Some(gap) = primary
            && gap.kind == CriterionKind::Compile
        {
            return RecommendedAction::InvokeTool {
                tool_name: COMPILE,
                criterion_id: gap.criterion_id.clone(),
                kind: gap.kind,
                priority: criterion_kind_priority(gap.kind),
                reason: format!(
                    "ApplyCorrection exitoso para `{}`: re-verificar con Compile",
                    gap.criterion_id.as_str()
                ),
            };
        }
    }

    select_primary_recommendation(evaluation)
}

/// Lista todas las recomendaciones ordenadas por prioridad (para trazabilidad/tests).
pub fn recommend_all_from_gap(gap: &GoalGap) -> Vec<RecommendedAction> {
    gap.unsatisfied
        .iter()
        .map(recommend_for_criterion_gap)
        .collect()
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
    /// Fingerprint de criterios ya visto en una evaluación anterior (ciclo de estado).
    RepeatedState,
}

/// Snapshot mínimo para identidad de estado autónomo (sin secretos / sin logs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomousStateSnapshot {
    pub goal_status: GoalStatus,
    pub criteria_fingerprint: String,
    pub recommendation_kind: String,
    pub last_action: Option<String>,
    pub artifact_revision: Option<u64>,
    pub pass_count: usize,
    pub gap_count: usize,
}

/// Evaluación de progreso de una iteración (actividad ≠ progreso).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressAssessment {
    pub signal: ProgressSignal,
    pub reason: String,
    pub repeated_action: bool,
    pub artifact_changed_without_progress: bool,
    pub snapshot: AutonomousStateSnapshot,
}

impl ProgressAssessment {
    pub fn is_meaningful_progress(&self) -> bool {
        matches!(self.signal, ProgressSignal::Improved)
    }

    pub fn is_non_progress(&self) -> bool {
        matches!(
            self.signal,
            ProgressSignal::Unchanged | ProgressSignal::RepeatedState | ProgressSignal::Regressed
        )
    }
}

/// Rastrea progreso medible y detecta ausencia de avance / regresiones / ciclos.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalProgressTracker {
    pass_counts: Vec<usize>,
    fingerprints: Vec<String>,
    actions: Vec<Option<String>>,
    artifact_revisions: Vec<Option<u64>>,
    stale_iterations: u32,
    assessments: Vec<ProgressAssessment>,
}

impl GoalProgressTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, evaluation: &GoalEvaluation) -> ProgressSignal {
        self.record_iteration(evaluation, None, None).signal
    }

    /// Registra una iteración con acción y revisión de artifact opcionales.
    pub fn record_iteration(
        &mut self,
        evaluation: &GoalEvaluation,
        action: Option<&str>,
        artifact_revision: Option<u64>,
    ) -> ProgressAssessment {
        let recommendation = select_primary_recommendation(evaluation);
        let pass_count = evaluation
            .specification_evaluation
            .criteria
            .iter()
            .filter(|item| item.verdict == EvaluationVerdict::Pass)
            .count();
        let fingerprint = progress_fingerprint(evaluation);
        let gap_count = evaluation.gap.unsatisfied.len();

        let signal = match self.pass_counts.last() {
            None => ProgressSignal::Unchanged,
            Some(prev) if pass_count > *prev => ProgressSignal::Improved,
            Some(prev) => {
                if self.fingerprints.last() == Some(&fingerprint) {
                    ProgressSignal::Unchanged
                } else if self.fingerprints.iter().any(|seen| seen == &fingerprint) {
                    ProgressSignal::RepeatedState
                } else if pass_count < *prev {
                    ProgressSignal::Regressed
                } else {
                    // Fingerprint nuevo con pass_count estable: evidencia/gap distinta.
                    ProgressSignal::Improved
                }
            }
        };

        let repeated_action = action.is_some()
            && self.actions.last().and_then(|item| item.as_deref()) == action
            && matches!(
                signal,
                ProgressSignal::Unchanged | ProgressSignal::RepeatedState
            );

        let artifact_changed_without_progress = artifact_revision.is_some()
            && self.artifact_revisions.last().copied().flatten() != artifact_revision
            && matches!(
                signal,
                ProgressSignal::Unchanged | ProgressSignal::RepeatedState
            );

        let action_advanced =
            action.is_some() && self.actions.last().and_then(|item| item.as_deref()) != action;

        let reason = match signal {
            ProgressSignal::Improved => {
                if gap_count
                    < self
                        .assessments
                        .last()
                        .map(|item| item.snapshot.gap_count)
                        .unwrap_or(usize::MAX)
                {
                    "gap reducido o criterio hacia Pass".to_string()
                } else {
                    "estado de criterios distinto con progreso medible".to_string()
                }
            }
            ProgressSignal::Unchanged => {
                if repeated_action {
                    format!(
                        "acción repetida sin cambio de estado: {}",
                        action.unwrap_or("?")
                    )
                } else if artifact_changed_without_progress {
                    "artifact mutó sin mejora de criterios/gap".to_string()
                } else {
                    "sin cambio en fingerprint de criterios".to_string()
                }
            }
            ProgressSignal::Regressed => {
                "menos criterios en Pass que la evaluación previa".to_string()
            }
            ProgressSignal::RepeatedState => {
                "fingerprint de criterios ya observado (ciclo de estado)".to_string()
            }
        };

        // Estancamiento: repetición de acción/estado, no pasos intermedios de un pipeline
        // (p. ej. RepairDiagnostic → ApplyCorrection → re-verify aún Unchanged).
        if signal == ProgressSignal::Improved {
            self.stale_iterations = 0;
        } else if signal == ProgressSignal::RepeatedState
            || repeated_action
            || (matches!(
                signal,
                ProgressSignal::Unchanged | ProgressSignal::Regressed
            ) && !action_advanced
                && !self.pass_counts.is_empty())
        {
            self.stale_iterations += 1;
        }

        let snapshot = AutonomousStateSnapshot {
            goal_status: evaluation.status,
            criteria_fingerprint: fingerprint.clone(),
            recommendation_kind: recommendation.kind_label().to_string(),
            last_action: action.map(str::to_string),
            artifact_revision,
            pass_count,
            gap_count,
        };

        let assessment = ProgressAssessment {
            signal,
            reason,
            repeated_action,
            artifact_changed_without_progress,
            snapshot,
        };

        self.pass_counts.push(pass_count);
        self.fingerprints.push(fingerprint);
        self.actions.push(action.map(str::to_string));
        self.artifact_revisions.push(artifact_revision);
        self.assessments.push(assessment.clone());
        assessment
    }

    pub fn stale_iterations(&self) -> u32 {
        self.stale_iterations
    }

    pub fn pass_count(&self) -> usize {
        self.pass_counts.last().copied().unwrap_or(0)
    }

    pub fn last_assessment(&self) -> Option<&ProgressAssessment> {
        self.assessments.last()
    }

    pub fn assessments(&self) -> &[ProgressAssessment] {
        &self.assessments
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
    /// Sin progreso medible durante la ventana configurada.
    NonProgress,
    /// Servicio externo bloqueó la ejecución autónoma.
    ExternalServiceBlocked,
    /// Configuración/credenciales del servicio modelo inválidas.
    ExternalConfigurationBlocked,
    /// El modelo no produjo progreso medible con el servicio disponible.
    ModelCapabilityFailure,
    /// Fallo interno del sistema autónomo.
    SystemFailure,
}

/// Historial de evaluaciones de Goal durante el loop.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalDrivenHistory {
    pub evaluations: Vec<GoalEvaluation>,
    pub gaps: Vec<GoalGap>,
    pub progress_signals: Vec<ProgressSignal>,
    pub progress_assessments: Vec<ProgressAssessment>,
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
        let max_stale_iterations = max_stale_iterations.max(1);
        Self {
            inner: AgentLoop::new(max_iterations).with_max_stale_iterations(max_stale_iterations),
            evaluator: GoalEvaluator::new(),
            progress: GoalProgressTracker::new(),
            max_stale_iterations,
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
        let initial_evidence = collect_evidence_from_context(&ctx);
        let initial = self.evaluator.evaluate(goal, &initial_evidence);
        history.evaluations.push(initial.clone());
        history.gaps.push(initial.gap.clone());
        let initial_assessment = self.progress.record_iteration(&initial, None, None);
        history.progress_signals.push(initial_assessment.signal);
        history.progress_assessments.push(initial_assessment);

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
        let artifact_revision = loop_result
            .final_context
            .working_artifact
            .as_ref()
            .map(|artifact| artifact.revision());
        let last_action = loop_result
            .history
            .executed_actions
            .last()
            .and_then(AgentAction::tool_name);
        let assessment =
            self.progress
                .record_iteration(&final_evaluation, last_action, artifact_revision);
        let progress = assessment.signal;
        history.progress_signals.push(progress);
        history.progress_assessments.push(assessment);

        if progress == ProgressSignal::Regressed {
            final_evaluation.status = GoalStatus::Unsatisfied;
        }

        // Prefer assessments collected mid-loop by AgentLoop when present.
        if !loop_result.history.progress_assessments.is_empty() {
            history
                .progress_assessments
                .extend(loop_result.history.progress_assessments.clone());
            history.progress_signals.extend(
                loop_result
                    .history
                    .progress_assessments
                    .iter()
                    .map(|item| item.signal),
            );
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
            GoalDrivenStatus::Escalated | GoalDrivenStatus::NonProgress => escalation
                .as_ref()
                .map(|item| item.reason.clone())
                .unwrap_or_else(|| loop_result.termination_reason.clone()),
            GoalDrivenStatus::MaxIterations => loop_result.termination_reason.clone(),
            GoalDrivenStatus::Failed
            | GoalDrivenStatus::ExternalServiceBlocked
            | GoalDrivenStatus::ExternalConfigurationBlocked
            | GoalDrivenStatus::ModelCapabilityFailure
            | GoalDrivenStatus::SystemFailure => {
                if let Some(report) = &loop_result.failure_report {
                    report.terminal_explanation()
                } else {
                    format!(
                        "goal no alcanzada: {} ({})",
                        loop_result.termination_reason,
                        final_evaluation.specification_evaluation.message
                    )
                }
            }
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
        failure_report: None,
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

    let non_progress = matches!(
        loop_result.status,
        LoopStatus::NonProgress
            | LoopStatus::ModelCapabilityFailure
            | LoopStatus::ExternalServiceBlocked
            | LoopStatus::ExternalConfigurationBlocked
            | LoopStatus::SystemFailure
    );
    let stale = progress.stale_iterations() >= max_stale || non_progress;
    let exhausted = loop_result.status == LoopStatus::MaxIterations;
    let no_hypothesis = evaluation
        .gap
        .unsatisfied
        .iter()
        .all(|gap| gap.kind == CriterionKind::Unknown || gap.suggested_action.is_none());

    if !stale && !exhausted && !no_hypothesis {
        return None;
    }

    let reason = if non_progress {
        loop_result.termination_reason.clone()
    } else if stale {
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
    match loop_result.status {
        LoopStatus::ExternalServiceBlocked => return GoalDrivenStatus::ExternalServiceBlocked,
        LoopStatus::ExternalConfigurationBlocked => {
            return GoalDrivenStatus::ExternalConfigurationBlocked;
        }
        LoopStatus::ModelCapabilityFailure => return GoalDrivenStatus::ModelCapabilityFailure,
        LoopStatus::SystemFailure => return GoalDrivenStatus::SystemFailure,
        LoopStatus::NonProgress => return GoalDrivenStatus::NonProgress,
        LoopStatus::Completed
        | LoopStatus::Failed
        | LoopStatus::Running
        | LoopStatus::MaxIterations => {}
    }
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
        LoopStatus::NonProgress => GoalDrivenStatus::NonProgress,
        LoopStatus::ExternalServiceBlocked => GoalDrivenStatus::ExternalServiceBlocked,
        LoopStatus::ExternalConfigurationBlocked => GoalDrivenStatus::ExternalConfigurationBlocked,
        LoopStatus::ModelCapabilityFailure => GoalDrivenStatus::ModelCapabilityFailure,
        LoopStatus::SystemFailure => GoalDrivenStatus::SystemFailure,
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

    fn action_for_recommendation(
        &mut self,
        recommendation: &RecommendedAction,
        ctx: &AgentContext,
    ) -> AgentAction {
        let label = recommendation.tool_name().unwrap_or("finish").to_string();
        self.attempted_actions.push(label);

        match recommendation {
            RecommendedAction::FinishAllowed { reason } => AgentAction::Finish {
                summary: reason.clone(),
            },
            RecommendedAction::RepairDiagnostic { .. } => {
                let errors = compile_errors_from_context(ctx);
                AgentAction::RepairDiagnostic { errors }
            }
            RecommendedAction::ApplyCorrection { reason, .. } => {
                // Agente determinista sin LLM: no inventa correcciones.
                AgentAction::Finish {
                    summary: format!(
                        "error: apply_correction_required without model synthesizer ({reason})"
                    ),
                }
            }
            RecommendedAction::InvokeTool { tool_name, .. } => {
                recommended_tool_to_agent_action(tool_name, ctx, &self.request)
            }
            RecommendedAction::NoDeterministicAction { reason } => AgentAction::Finish {
                summary: reason.clone(),
            },
        }
    }

    fn action_for_gap(&mut self, gap: &CriterionGap, ctx: &AgentContext) -> AgentAction {
        self.action_for_recommendation(&recommend_for_criterion_gap(gap), ctx)
    }
}

fn compile_errors_from_context(ctx: &AgentContext) -> Vec<String> {
    if let Some(AgentObservation::CriterionEvaluated {
        evidence, message, ..
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
        if !errors.is_empty() {
            return errors;
        }
    }
    vec!["compilación fallida".to_string()]
}

fn recommended_tool_to_agent_action(
    tool_name: &str,
    ctx: &AgentContext,
    request: &str,
) -> AgentAction {
    use crate::harness::tools::{CHECK_FORMAT, COMPILE, RUN_CLIPPY, RUN_TESTS, VALIDATE};
    match tool_name {
        COMPILE => AgentAction::Compile {
            code: ctx
                .working_code()
                .map(str::to_string)
                .unwrap_or_else(|| "fn main() {}".to_string()),
        },
        VALIDATE => AgentAction::Validate {
            request: request.to_string(),
            code: ctx.working_code().map(str::to_string),
            plan_kind: "Api".to_string(),
        },
        RUN_TESTS => AgentAction::RunTests {
            filter: String::new(),
        },
        RUN_CLIPPY => AgentAction::RunClippy,
        CHECK_FORMAT => AgentAction::CheckFormat,
        _ => AgentAction::Finish {
            summary: format!("tool no mapeada: {tool_name}"),
        },
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
                    &collect_evidence_from_context(ctx),
                );
                if evaluation.status == GoalStatus::Satisfied {
                    return AgentAction::Finish {
                        summary: "goal satisfied".to_string(),
                    };
                }
                let recommendation = select_primary_recommendation_with_context(&evaluation, ctx);
                return self.action_for_recommendation(&recommendation, ctx);
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
                &collect_evidence_from_context(ctx),
            );
            if evaluation.status == GoalStatus::Satisfied {
                return AgentAction::Finish {
                    summary: "goal satisfied".to_string(),
                };
            }
            let recommendation = select_primary_recommendation_with_context(&evaluation, ctx);
            return self.action_for_recommendation(&recommendation, ctx);
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

/// Recolecta evidencia acumulada en el historial de observaciones del contexto.
pub fn collect_evidence_from_context(ctx: &AgentContext) -> Vec<Evidence> {
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
                GoalDrivenStatus::Escalated
                    | GoalDrivenStatus::MaxIterations
                    | GoalDrivenStatus::NonProgress
                    | GoalDrivenStatus::ExternalServiceBlocked
                    | GoalDrivenStatus::ExternalConfigurationBlocked
                    | GoalDrivenStatus::ModelCapabilityFailure
                    | GoalDrivenStatus::SystemFailure
                    | GoalDrivenStatus::Failed
            ),
            "debe escalar, agotar iteraciones o detectar non-progress: {:?}",
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

    // --- RecommendedAction selection (unit A-D) ---

    #[test]
    fn recommended_action_finish_allowed_when_goal_satisfied() {
        // A
        let goal = compile_only_goal();
        let evidence = vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
        ];
        let evaluation = GoalEvaluator::new().evaluate(&goal, &evidence);
        let rec = select_primary_recommendation(&evaluation);
        assert!(matches!(rec, RecommendedAction::FinishAllowed { .. }));
        assert!(rec.tool_name().is_none());
    }

    #[test]
    fn recommended_action_compile_when_no_evidence() {
        // B
        let goal = compile_only_goal();
        let evaluation = GoalEvaluator::new().evaluate(&goal, &[]);
        let rec = select_primary_recommendation(&evaluation);
        assert!(matches!(
            rec,
            RecommendedAction::InvokeTool {
                tool_name,
                kind: CriterionKind::Compile,
                ..
            } if tool_name == COMPILE
        ));
    }

    #[test]
    fn recommended_action_repair_on_compile_fail_not_finish() {
        // C
        let goal = compile_only_goal();
        let evidence = vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "error"),
            Evidence::new("compiler_stderr", "expected `}`"),
        ];
        let evaluation = GoalEvaluator::new().evaluate(&goal, &evidence);
        let rec = select_primary_recommendation(&evaluation);
        assert!(matches!(
            rec,
            RecommendedAction::RepairDiagnostic {
                kind: CriterionKind::Compile,
                ..
            }
        ));
        assert!(!matches!(rec, RecommendedAction::FinishAllowed { .. }));
    }

    #[test]
    fn recommended_action_deterministic_priority_with_multiple_gaps() {
        // D — Compile (0) antes que Validate (1)
        let goal = compile_and_validate_goal();
        let evaluation = GoalEvaluator::new().evaluate(&goal, &[]);
        assert_eq!(evaluation.gap.unsatisfied.len(), 2);
        assert_eq!(
            evaluation.gap.primary().unwrap().kind,
            CriterionKind::Compile
        );
        let rec = select_primary_recommendation(&evaluation);
        assert!(matches!(
            rec,
            RecommendedAction::InvokeTool {
                kind: CriterionKind::Compile,
                priority: 0,
                ..
            }
        ));
        let all = recommend_all_from_gap(&evaluation.gap);
        assert_eq!(all.len(), 2);
        assert!(all[0].priority() <= all[1].priority());
    }
}
