//! Abstracción model-agnostic entre [`crate::harness::AiAgent`] y proveedores futuros.
//!
//! ModelClient no conoce Harness, Tools ni componentes del Constructor.

use crate::harness::artifact::RustArtifact;
use crate::harness::artifact_file_operation::ArtifactFileOperation;
use crate::harness::artifact_path::ArtifactPath;
use crate::harness::context::AgentContext;
use crate::harness::correction::{Correction, CorrectionOperation, CorrectionTarget};
use crate::harness::criterion::CriterionKind;
use crate::harness::evaluation::EvaluationVerdict;
use crate::harness::goal_driven::{
    Goal, GoalEvaluator, GoalStatus, RecommendedAction, collect_evidence_from_context,
    select_primary_recommendation,
};
use crate::harness::observation::AgentObservation;
use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC, VALIDATE};

/// Configuración de sesión que AiAgent necesita para serializar contexto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSessionConfig {
    pub user_request: String,
    pub plan_kind: String,
    /// Redirige Finish prematuro vía [`apply_gap_guidance`] cuando está activo.
    pub gap_guidance: bool,
}

impl AiSessionConfig {
    pub fn new(user_request: impl Into<String>, plan_kind: impl Into<String>) -> Self {
        Self {
            user_request: user_request.into(),
            plan_kind: plan_kind.into(),
            gap_guidance: false,
        }
    }

    pub fn with_gap_guidance(mut self, enabled: bool) -> Self {
        self.gap_guidance = enabled;
        self
    }
}

/// Contexto diagnóstico agregado desde observaciones recientes (Tools → Evidence).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SerializedDiagnosticContext {
    pub compile_status: Option<String>,
    pub compiler_stderr: Vec<String>,
    pub validator_errors: Vec<String>,
    pub repairer_feedback: Vec<String>,
    /// Pares label→detail de evidencia diagnóstica reciente (sin truncar agresivamente).
    pub evidence_pairs: Vec<(String, String)>,
}

/// Observación serializada para el modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedObservation {
    pub kind: String,
    pub tool_name: Option<String>,
    pub success: Option<bool>,
    pub summary: String,
    pub validator_errors: Vec<String>,
    pub repairer_feedback: Vec<String>,
    pub evidence_labels: Vec<String>,
    /// Detalle de evidencia diagnóstica (p. ej. compiler_stderr completo hasta límite).
    pub evidence_details: Vec<(String, String)>,
    /// Verdict de Evaluation (`Pass` / `Fail` / `InsufficientEvidence`), si aplica.
    pub evaluation_verdict: Option<String>,
    pub specification_id: Option<String>,
    pub criterion_id: Option<String>,
    pub criterion_kind: Option<String>,
    pub evaluation_message: Option<String>,
}

/// Snapshot de un archivo del Artifact para serialización al modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFileSnapshot {
    pub path: String,
    pub source: String,
}

/// Snapshot serializado de una acción recomendada para el modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedRecommendedAction {
    pub kind: String,
    pub tool_name: Option<String>,
    pub criterion_id: Option<String>,
    pub criterion_kind: Option<String>,
    pub priority: u8,
    pub reason: String,
}

/// Snapshot serializado de un criterio insatisfecho (Goal Gap).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedCriterionGap {
    pub criterion_id: String,
    pub kind: String,
    pub verdict: String,
    pub message: String,
    pub suggested_action: Option<String>,
}

/// Snapshot serializado del Goal Gap (criterios pendientes).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SerializedGoalGap {
    pub unsatisfied_count: usize,
    pub gaps: Vec<SerializedCriterionGap>,
}

impl SerializedGoalGap {
    pub fn primary(&self) -> Option<&SerializedCriterionGap> {
        self.gaps.first()
    }

    pub fn is_empty(&self) -> bool {
        self.gaps.is_empty()
    }
}

/// Resumen serializado de evaluación de Goal para el modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedGoalEvaluation {
    pub goal_id: String,
    pub status: String,
    pub criteria_total: usize,
    pub criteria_pass: usize,
    pub criteria_fail: usize,
    pub criteria_insufficient: usize,
    pub message: String,
}

/// Petición estructurada enviada al modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub goal: String,
    pub step: u32,
    pub user_request: String,
    pub plan_kind: Option<String>,
    /// Contenido del archivo primary (compat single-file).
    pub working_code: Option<String>,
    pub artifact_id: Option<String>,
    pub artifact_language: Option<String>,
    pub artifact_revision: Option<u64>,
    /// Ruta lógica del archivo primary dentro del Artifact.
    pub artifact_primary_path: Option<String>,
    /// Árbol completo del Artifact (incluye primary y archivos secundarios).
    pub artifact_files: Vec<ArtifactFileSnapshot>,
    pub last_observation: Option<SerializedObservation>,
    pub recent_observations: Vec<SerializedObservation>,
    pub recent_evidence: Vec<(String, String)>,
    /// Evaluación de Goal cuando hay `evaluation_specification` en el contexto.
    pub goal_evaluation: Option<SerializedGoalEvaluation>,
    /// Gap de Goal: criterios insatisfechos con acciones sugeridas.
    pub goal_gap: Option<SerializedGoalGap>,
    /// Acción recomendada primaria derivada de Goal + evaluación + gap.
    pub recommended_action: Option<SerializedRecommendedAction>,
    /// Evidencia diagnóstica agregada de Tools recientes (compile, validate, repair).
    pub diagnostic_context: SerializedDiagnosticContext,
    /// Prompt system versionado incluido en cada petición al modelo.
    pub system_prompt: String,
}

/// Respuesta cruda del modelo (texto estructurado parseable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResponse {
    pub raw_text: String,
}

/// Decisión validada extraída de una respuesta del modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDecision {
    Validate {
        request: String,
        code: Option<String>,
        plan_kind: String,
    },
    RepairDiagnostic {
        errors: Vec<String>,
    },
    ApplyCorrection {
        corrections: Vec<StructuredCorrection>,
    },
    ApplyFileOperations {
        operations: Vec<StructuredFileOperation>,
    },
    Compile {
        code: String,
    },
    RunTests {
        filter: String,
    },
    RunClippy,
    CheckFormat,
    Finish {
        summary: String,
    },
}

/// Corrección estructurada en la respuesta del modelo (antes de mapear a [`Correction`]).
///
/// `path`: frontera de serialización del modelo (`Option<String>`). Ausente o vacío → primary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum StructuredCorrection {
    ReplaceText {
        path: Option<String>,
        search: String,
        replacement: String,
    },
    InsertText {
        path: Option<String>,
        position: usize,
        text: String,
    },
    RemoveText {
        path: Option<String>,
        start: usize,
        end: usize,
    },
}

/// Operación estructural en la respuesta del modelo (antes de [`ArtifactFileOperation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum StructuredFileOperation {
    CreateFile { path: String, source: String },
    DeleteFile { path: String },
    RenameFile { from: String, to: String },
}

fn structured_correction_path(item: &StructuredCorrection) -> &Option<String> {
    match item {
        StructuredCorrection::ReplaceText { path, .. }
        | StructuredCorrection::InsertText { path, .. }
        | StructuredCorrection::RemoveText { path, .. } => path,
    }
}

/// Traza estructurada de interacción AiAgent ↔ ModelClient.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelInteractionTrace {
    pub requests: Vec<ModelRequest>,
    pub responses: Vec<ModelResponse>,
    pub parsed_decisions: Vec<Result<ModelDecision, ModelResponseError>>,
    pub resulting_actions: Vec<Result<String, ModelResponseError>>,
}

impl ModelInteractionTrace {
    pub fn record_request(&mut self, request: ModelRequest) {
        self.requests.push(request);
    }

    pub fn record_response(&mut self, response: ModelResponse) {
        self.responses.push(response);
    }

    pub fn record_decision(&mut self, decision: Result<ModelDecision, ModelResponseError>) {
        self.parsed_decisions.push(decision);
    }

    pub fn record_action_label(&mut self, action: Result<String, ModelResponseError>) {
        self.resulting_actions.push(action);
    }
}

/// Error de transporte/consulta al modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    Configuration(String),
    Authentication(String),
    RateLimited(String),
    Timeout,
    Transport(String),
    Http { status: u16, category: String },
    InvalidResponse(String),
}

impl ModelError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited(_) | Self::Timeout => true,
            Self::Http { status, .. } => (500..600).contains(status),
            Self::Configuration(_)
            | Self::Authentication(_)
            | Self::Transport(_)
            | Self::InvalidResponse(_) => false,
        }
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(message) => write!(f, "configuración: {message}"),
            Self::Authentication(message) => write!(f, "autenticación: {message}"),
            Self::RateLimited(message) => write!(f, "rate limit: {message}"),
            Self::Timeout => write!(f, "timeout del modelo"),
            Self::Transport(message) => write!(f, "transporte: {message}"),
            Self::Http { status, category } => {
                write!(f, "http {status}: {category}")
            }
            Self::InvalidResponse(message) => write!(f, "respuesta inválida: {message}"),
        }
    }
}

/// Garantiza que errores públicos no filtren secretos.
pub fn redact_secrets(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.to_ascii_lowercase().contains("authorization") {
                "authorization: [REDACTED]".to_string()
            } else if line.to_ascii_lowercase().contains("api_key") {
                "api_key: [REDACTED]".to_string()
            } else if line.to_ascii_lowercase().contains("bearer ") {
                "bearer [REDACTED]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Error al serializar contexto o interpretar respuesta del modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelResponseError {
    ContextSerializationError(String),
    InvalidModelResponse(String),
    UnsupportedAction(String),
    InvalidCorrection(String),
    InvalidFileOperation(String),
}

impl std::fmt::Display for ModelResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContextSerializationError(message) => {
                write!(f, "serialización de contexto: {message}")
            }
            Self::InvalidModelResponse(message) => write!(f, "respuesta inválida: {message}"),
            Self::UnsupportedAction(message) => write!(f, "acción no soportada: {message}"),
            Self::InvalidCorrection(message) => write!(f, "corrección inválida: {message}"),
            Self::InvalidFileOperation(message) => {
                write!(f, "operación de archivo inválida: {message}")
            }
        }
    }
}

/// Contrato model-agnostic: consulta al modelo, sin efectos secundarios.
pub trait ModelClient: Send + Sync {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError>;
}

/// Serializa [`AgentContext`] + configuración de sesión en [`ModelRequest`].
pub fn model_request_from_context(
    ctx: &AgentContext,
    session: &AiSessionConfig,
) -> Result<ModelRequest, ModelResponseError> {
    if session.user_request.trim().is_empty() {
        return Err(ModelResponseError::ContextSerializationError(
            "user_request vacío".to_string(),
        ));
    }

    let last_observation = ctx.last_observation.as_ref().map(serialize_observation);

    let recent_observations = ctx
        .observation_history
        .iter()
        .rev()
        .take(5)
        .map(serialize_observation)
        .collect::<Vec<_>>();

    let recent_evidence = collect_recent_evidence(ctx);
    let diagnostic_context = collect_diagnostic_context(ctx);

    let (artifact_primary_path, artifact_files) = ctx
        .working_artifact
        .as_ref()
        .map(artifact_file_snapshots_from_artifact)
        .map(|(primary, files)| (Some(primary), files))
        .unwrap_or((None, Vec::new()));

    let (goal_evaluation, goal_gap, recommended_action) = ctx
        .evaluation_specification
        .as_ref()
        .map(|spec| {
            let evaluation = GoalEvaluator::new().evaluate(
                &Goal::from_specification(spec.clone()),
                &collect_evidence_from_context(ctx),
            );
            let recommendation = select_primary_recommendation(&evaluation);
            (
                Some(serialize_goal_evaluation(&evaluation)),
                if evaluation.gap.is_empty() {
                    None
                } else {
                    Some(serialize_goal_gap(&evaluation.gap))
                },
                Some(serialize_recommended_action(&recommendation)),
            )
        })
        .unwrap_or((None, None, None));

    Ok(ModelRequest {
        goal: ctx.goal.clone(),
        step: ctx.step,
        user_request: session.user_request.clone(),
        plan_kind: Some(session.plan_kind.clone()),
        working_code: ctx.working_code().map(str::to_string),
        artifact_id: ctx
            .working_artifact
            .as_ref()
            .map(|artifact| artifact.id().as_str().to_string()),
        artifact_language: ctx
            .working_artifact
            .as_ref()
            .map(|artifact| artifact.language().as_str().to_string()),
        artifact_revision: ctx
            .working_artifact
            .as_ref()
            .map(|artifact| artifact.revision()),
        artifact_primary_path,
        artifact_files,
        last_observation,
        recent_observations,
        recent_evidence,
        goal_evaluation,
        goal_gap,
        recommended_action,
        diagnostic_context,
        system_prompt: crate::harness::agent_prompt::system_prompt_v1().to_string(),
    })
}

const DIAGNOSTIC_EVIDENCE_LABELS: &[&str] = &[
    "compiler_stderr",
    "compile_status",
    "validate_status",
    "diagnostic_status",
    "correction_status",
];

fn is_diagnostic_evidence_label(label: &str) -> bool {
    DIAGNOSTIC_EVIDENCE_LABELS.contains(&label)
        || label.starts_with("validator_error_")
        || label.starts_with("repairer_feedback_")
}

fn collect_diagnostic_context(ctx: &AgentContext) -> SerializedDiagnosticContext {
    let mut compile_status = None;
    let mut compiler_stderr = Vec::new();
    let mut validator_errors = Vec::new();
    let mut repairer_feedback = Vec::new();
    let mut evidence_pairs = Vec::new();

    for observation in ctx.observation_history.iter().rev().take(8) {
        let obs_errors: Vec<String> = observation
            .validator_errors()
            .into_iter()
            .map(str::to_string)
            .collect();
        for error in obs_errors {
            if !validator_errors.contains(&error) {
                validator_errors.push(error);
            }
        }

        let obs_feedback: Vec<String> = observation
            .repairer_feedback()
            .into_iter()
            .map(str::to_string)
            .collect();
        for item in obs_feedback {
            if !repairer_feedback.contains(&item) {
                repairer_feedback.push(item);
            }
        }

        let evidence = match observation {
            AgentObservation::ToolOutcome { evidence, .. }
            | AgentObservation::CriterionEvaluated { evidence, .. } => evidence,
            AgentObservation::SpecificationEvaluated { criteria, .. } => {
                let flat: Vec<_> = criteria
                    .iter()
                    .flat_map(|item| item.evidence_used.iter())
                    .collect();
                for item in &flat {
                    push_diagnostic_evidence(
                        item,
                        &mut compile_status,
                        &mut compiler_stderr,
                        &mut validator_errors,
                        &mut repairer_feedback,
                        &mut evidence_pairs,
                    );
                }
                continue;
            }
            _ => continue,
        };

        for item in evidence {
            push_diagnostic_evidence(
                item,
                &mut compile_status,
                &mut compiler_stderr,
                &mut validator_errors,
                &mut repairer_feedback,
                &mut evidence_pairs,
            );
        }
    }

    SerializedDiagnosticContext {
        compile_status,
        compiler_stderr,
        validator_errors,
        repairer_feedback,
        evidence_pairs,
    }
}

fn push_diagnostic_evidence(
    item: &crate::harness::evaluation::Evidence,
    compile_status: &mut Option<String>,
    compiler_stderr: &mut Vec<String>,
    validator_errors: &mut Vec<String>,
    repairer_feedback: &mut Vec<String>,
    evidence_pairs: &mut Vec<(String, String)>,
) {
    if !is_diagnostic_evidence_label(&item.label) {
        return;
    }
    let detail = truncate_diagnostic_detail(&item.detail);
    if evidence_pairs
        .iter()
        .any(|(label, value)| label == &item.label && value == &detail)
    {
        return;
    }
    evidence_pairs.push((item.label.clone(), detail.clone()));

    match item.label.as_str() {
        "compile_status" if compile_status.is_none() => {
            *compile_status = Some(detail.clone());
        }
        "compiler_stderr" if !compiler_stderr.contains(&detail) => {
            compiler_stderr.push(detail);
        }
        label if label.starts_with("validator_error_") => {
            if !validator_errors.contains(&detail) {
                validator_errors.push(detail);
            }
        }
        label
            if label.starts_with("repairer_feedback_") && !repairer_feedback.contains(&detail) =>
        {
            repairer_feedback.push(detail);
        }
        _ => {}
    }
}

fn truncate_diagnostic_detail(text: &str) -> String {
    if text.chars().count() <= 4_000 {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(4_000).collect::<String>())
    }
}

/// Emite contexto diagnóstico agregado en el mensaje de usuario del modelo.
pub fn append_diagnostic_context_to_message_parts(
    parts: &mut Vec<String>,
    context: &SerializedDiagnosticContext,
) {
    if let Some(status) = &context.compile_status {
        parts.push(format!("diagnostic_compile_status={status}"));
    }
    for (index, stderr) in context.compiler_stderr.iter().enumerate() {
        parts.push(format!("diagnostic_compiler_stderr_{index}={stderr}"));
    }
    if !context.validator_errors.is_empty() {
        parts.push(format!(
            "diagnostic_validator_errors={}",
            context.validator_errors.join(" | ")
        ));
    }
    if !context.repairer_feedback.is_empty() {
        parts.push(format!(
            "diagnostic_repairer_feedback={}",
            context.repairer_feedback.join(" | ")
        ));
    }
    for (index, (label, detail)) in context.evidence_pairs.iter().enumerate() {
        parts.push(format!("diagnostic_evidence_{index}_label={label}"));
        parts.push(format!("diagnostic_evidence_{index}_detail={detail}"));
    }
}

/// Emite evidencia reciente (label+detail) en el mensaje de usuario.
pub fn append_recent_evidence_to_message_parts(
    parts: &mut Vec<String>,
    evidence: &[(String, String)],
) {
    for (index, (label, detail)) in evidence.iter().enumerate() {
        parts.push(format!("recent_evidence_{index}_label={label}"));
        parts.push(format!("recent_evidence_{index}_detail={detail}"));
    }
}

fn diagnostic_errors_from_request(request: &ModelRequest) -> Vec<String> {
    let mut errors = request.diagnostic_context.compiler_stderr.clone();
    errors.extend(request.diagnostic_context.validator_errors.clone());
    if errors.is_empty()
        && let Some(obs) = &request.last_observation
    {
        errors.extend(obs.validator_errors.clone());
        if let Some(message) = &obs.evaluation_message
            && !message.is_empty()
        {
            errors.push(message.clone());
        }
        if errors.is_empty() && !obs.summary.is_empty() {
            errors.push(obs.summary.clone());
        }
    }
    if errors.is_empty() {
        errors.push("diagnóstico requerido".to_string());
    }
    errors
}

fn artifact_file_snapshots_from_artifact(
    artifact: &RustArtifact,
) -> (String, Vec<ArtifactFileSnapshot>) {
    let primary = artifact.primary_path().as_str().to_string();
    let mut files = artifact
        .files()
        .map(|(path, source)| ArtifactFileSnapshot {
            path: path.as_str().to_string(),
            source: source.to_string(),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        let left_is_primary = left.path == primary;
        let right_is_primary = right.path == primary;
        match (left_is_primary, right_is_primary) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left.path.cmp(&right.path),
        }
    });
    (primary, files)
}

/// Añade campos multi-file del Artifact al mensaje de usuario del modelo.
pub fn append_artifact_files_to_message_parts(
    parts: &mut Vec<String>,
    primary_path: Option<&str>,
    files: &[ArtifactFileSnapshot],
) {
    if let Some(primary) = primary_path {
        parts.push(format!("artifact_primary_path={primary}"));
    }
    if files.is_empty() {
        return;
    }
    parts.push(format!("artifact_file_count={}", files.len()));
    for (index, file) in files.iter().enumerate() {
        parts.push(format!("artifact_file_{index}_path={}", file.path));
        parts.push(format!(
            "artifact_file_{index}_source_bytes={}",
            file.source.len()
        ));
        parts.push(format!("artifact_file_{index}_source={}", file.source));
    }
}

fn goal_status_label(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Satisfied => "Satisfied",
        GoalStatus::Unsatisfied => "Unsatisfied",
        GoalStatus::Inconclusive => "Inconclusive",
    }
}

fn evaluation_verdict_label(verdict: EvaluationVerdict) -> &'static str {
    match verdict {
        EvaluationVerdict::Pass => "Pass",
        EvaluationVerdict::Fail => "Fail",
        EvaluationVerdict::InsufficientEvidence => "InsufficientEvidence",
    }
}

fn serialize_goal_evaluation(
    evaluation: &crate::harness::goal_driven::GoalEvaluation,
) -> SerializedGoalEvaluation {
    let criteria = &evaluation.specification_evaluation.criteria;
    let criteria_pass = criteria
        .iter()
        .filter(|item| item.verdict == EvaluationVerdict::Pass)
        .count();
    let criteria_fail = criteria
        .iter()
        .filter(|item| item.verdict == EvaluationVerdict::Fail)
        .count();
    let criteria_insufficient = criteria
        .iter()
        .filter(|item| item.verdict == EvaluationVerdict::InsufficientEvidence)
        .count();
    SerializedGoalEvaluation {
        goal_id: evaluation.goal_id.as_str().to_string(),
        status: goal_status_label(evaluation.status).to_string(),
        criteria_total: criteria.len(),
        criteria_pass,
        criteria_fail,
        criteria_insufficient,
        message: evaluation.specification_evaluation.message.clone(),
    }
}

fn serialize_recommended_action(action: &RecommendedAction) -> SerializedRecommendedAction {
    match action {
        RecommendedAction::FinishAllowed { reason } => SerializedRecommendedAction {
            kind: "FinishAllowed".to_string(),
            tool_name: None,
            criterion_id: None,
            criterion_kind: None,
            priority: action.priority(),
            reason: reason.clone(),
        },
        RecommendedAction::InvokeTool {
            tool_name,
            criterion_id,
            kind,
            priority,
            reason,
        } => SerializedRecommendedAction {
            kind: "InvokeTool".to_string(),
            tool_name: Some((*tool_name).to_string()),
            criterion_id: Some(criterion_id.as_str().to_string()),
            criterion_kind: Some(format!("{kind:?}")),
            priority: *priority,
            reason: reason.clone(),
        },
        RecommendedAction::RepairDiagnostic {
            criterion_id,
            kind,
            priority,
            reason,
        } => SerializedRecommendedAction {
            kind: "RepairDiagnostic".to_string(),
            tool_name: Some(REPAIR_DIAGNOSTIC.to_string()),
            criterion_id: Some(criterion_id.as_str().to_string()),
            criterion_kind: Some(format!("{kind:?}")),
            priority: *priority,
            reason: reason.clone(),
        },
        RecommendedAction::NoDeterministicAction { reason } => SerializedRecommendedAction {
            kind: "NoDeterministicAction".to_string(),
            tool_name: None,
            criterion_id: None,
            criterion_kind: None,
            priority: action.priority(),
            reason: reason.clone(),
        },
    }
}

fn serialize_goal_gap(gap: &crate::harness::goal_driven::GoalGap) -> SerializedGoalGap {
    SerializedGoalGap {
        unsatisfied_count: gap.unsatisfied.len(),
        gaps: gap
            .unsatisfied
            .iter()
            .map(|item| SerializedCriterionGap {
                criterion_id: item.criterion_id.as_str().to_string(),
                kind: format!("{:?}", item.kind),
                verdict: evaluation_verdict_label(item.verdict).to_string(),
                message: item.message.clone(),
                suggested_action: item.suggested_action.map(str::to_string),
            })
            .collect(),
    }
}

/// Añade campos goal_evaluation / goal_gap al mensaje de usuario del modelo.
pub fn append_goal_context_to_message_parts(parts: &mut Vec<String>, request: &ModelRequest) {
    let Some(eval) = &request.goal_evaluation else {
        return;
    };
    parts.push(format!("goal_evaluation_goal_id={}", eval.goal_id));
    parts.push(format!("goal_evaluation_status={}", eval.status));
    parts.push(format!(
        "goal_evaluation_criteria_total={}",
        eval.criteria_total
    ));
    parts.push(format!(
        "goal_evaluation_criteria_pass={}",
        eval.criteria_pass
    ));
    parts.push(format!(
        "goal_evaluation_criteria_fail={}",
        eval.criteria_fail
    ));
    parts.push(format!(
        "goal_evaluation_criteria_insufficient={}",
        eval.criteria_insufficient
    ));
    parts.push(format!("goal_evaluation_message={}", eval.message));

    let Some(gap) = &request.goal_gap else {
        parts.push("goal_gap_unsatisfied_count=0".to_string());
        return;
    };
    parts.push(format!(
        "goal_gap_unsatisfied_count={}",
        gap.unsatisfied_count
    ));
    for (index, item) in gap.gaps.iter().enumerate() {
        parts.push(format!(
            "goal_gap_{index}_criterion_id={}",
            item.criterion_id
        ));
        parts.push(format!("goal_gap_{index}_kind={}", item.kind));
        parts.push(format!("goal_gap_{index}_verdict={}", item.verdict));
        parts.push(format!("goal_gap_{index}_message={}", item.message));
        if let Some(action) = &item.suggested_action {
            parts.push(format!("goal_gap_{index}_suggested_action={action}"));
        }
    }

    if let Some(rec) = &request.recommended_action {
        append_recommended_action_to_message_parts(parts, rec);
    }
}

/// Emite la directiva operacional de RecommendedAction de forma prominente.
pub fn append_recommended_action_to_message_parts(
    parts: &mut Vec<String>,
    rec: &SerializedRecommendedAction,
) {
    parts.push("recommended_action_directive=MUST_FOLLOW_WHEN_GOAL_UNSATISFIED".to_string());
    parts.push(format!("recommended_action_kind={}", rec.kind));
    if let Some(tool) = &rec.tool_name {
        parts.push(format!("recommended_action_tool={tool}"));
    }
    if let Some(id) = &rec.criterion_id {
        parts.push(format!("recommended_action_criterion_id={id}"));
    }
    if let Some(kind) = &rec.criterion_kind {
        parts.push(format!("recommended_action_criterion_kind={kind}"));
    }
    parts.push(format!("recommended_action_priority={}", rec.priority));
    parts.push(format!("recommended_action_reason={}", rec.reason));
}

/// Indica si una [`ModelDecision`] es compatible con la acción recomendada serializada.
pub fn decision_is_compatible_with_recommendation(
    decision: &ModelDecision,
    rec: &SerializedRecommendedAction,
) -> bool {
    match rec.kind.as_str() {
        "FinishAllowed" => true,
        "NoDeterministicAction" => !matches!(decision, ModelDecision::Finish { .. }),
        "InvokeTool" | "RepairDiagnostic" => rec
            .tool_name
            .as_deref()
            .is_some_and(|tool| decision_matches_recommended_tool(decision, tool)),
        _ => true,
    }
}

fn decision_matches_recommended_tool(decision: &ModelDecision, tool_name: &str) -> bool {
    use crate::harness::tools::{
        CHECK_FORMAT, COMPILE, REPAIR_DIAGNOSTIC, RUN_CLIPPY, RUN_TESTS, VALIDATE,
    };
    match tool_name {
        COMPILE => matches!(decision, ModelDecision::Compile { .. }),
        VALIDATE => matches!(decision, ModelDecision::Validate { .. }),
        RUN_TESTS => matches!(decision, ModelDecision::RunTests { .. }),
        RUN_CLIPPY => matches!(decision, ModelDecision::RunClippy),
        CHECK_FORMAT => matches!(decision, ModelDecision::CheckFormat),
        REPAIR_DIAGNOSTIC => matches!(decision, ModelDecision::RepairDiagnostic { .. }),
        _ => false,
    }
}

/// Valida y corrige determinísticamente una decisión incompatible con `recommended_action`.
///
/// Más amplio que el redirect de Finish: también redirige acciones de tipo distinto
/// (p. ej. Validate cuando se recomienda Compile).
pub fn validate_model_decision_against_recommendation(
    decision: ModelDecision,
    request: &ModelRequest,
) -> ModelDecision {
    if request
        .goal_evaluation
        .as_ref()
        .is_some_and(|eval| eval.status == "Satisfied")
    {
        return decision;
    }

    // Tras RepairDiagnostic exitoso, ApplyCorrection es el paso esperado aunque
    // el gap de Compile siga recomendando RepairDiagnostic.
    if matches!(decision, ModelDecision::ApplyCorrection { .. })
        && request.last_observation.as_ref().is_some_and(|obs| {
            obs.kind == "tool_outcome"
                && obs.tool_name.as_deref() == Some(REPAIR_DIAGNOSTIC)
                && obs.success == Some(true)
        })
    {
        return decision;
    }

    // Tras ApplyCorrection exitoso, re-compilar antes de re-evaluar el gap de Compile.
    if matches!(decision, ModelDecision::Compile { .. })
        && request.last_observation.as_ref().is_some_and(|obs| {
            obs.kind == "tool_outcome"
                && obs.tool_name.as_deref() == Some(APPLY_CORRECTION)
                && obs.success == Some(true)
        })
    {
        return decision;
    }

    // Tras FAIL de un criterio, RepairDiagnostic precede a re-ejecutar la Tool.
    if matches!(decision, ModelDecision::RepairDiagnostic { .. })
        && request.last_observation.as_ref().is_some_and(|obs| {
            obs.kind == "criterion_evaluated" && obs.evaluation_verdict.as_deref() == Some("Fail")
        })
    {
        return decision;
    }

    if let Some(rec) = &request.recommended_action {
        if rec.kind == "FinishAllowed" {
            return decision;
        }
        if decision_is_compatible_with_recommendation(&decision, rec) {
            return decision;
        }
        if let Some(redirected) = model_decision_from_recommended_action(rec, request) {
            return redirected;
        }
        return decision;
    }

    if matches!(decision, ModelDecision::Finish { .. })
        && let Some(gap) = &request.goal_gap
        && let Some(redirected) = decision_from_goal_gap(gap, request)
    {
        return redirected;
    }

    decision
}

/// Convierte acción recomendada serializada en [`ModelDecision`] ejecutable.
pub fn model_decision_from_recommended_action(
    action: &SerializedRecommendedAction,
    request: &ModelRequest,
) -> Option<ModelDecision> {
    match action.kind.as_str() {
        "FinishAllowed" => Some(ModelDecision::Finish {
            summary: action.reason.clone(),
        }),
        "InvokeTool" => {
            let kind_label = action.criterion_kind.as_deref()?;
            let kind = parse_criterion_kind_label(kind_label)?;
            decision_for_criterion_kind(kind, request)
        }
        "RepairDiagnostic" => Some(ModelDecision::RepairDiagnostic {
            errors: diagnostic_errors_from_request(request),
        }),
        "NoDeterministicAction" => None,
        _ => None,
    }
}

/// Mapea el gap primario a una [`ModelDecision`] sugerida (política gap-guided).
pub fn decision_from_goal_gap(
    gap: &SerializedGoalGap,
    request: &ModelRequest,
) -> Option<ModelDecision> {
    if let Some(rec) = &request.recommended_action {
        return model_decision_from_recommended_action(rec, request);
    }
    let primary = gap.primary()?;
    let kind = parse_criterion_kind_label(&primary.kind)?;
    decision_for_criterion_kind(kind, request)
}

fn parse_criterion_kind_label(label: &str) -> Option<CriterionKind> {
    match label {
        "Compile" => Some(CriterionKind::Compile),
        "Validate" => Some(CriterionKind::Validate),
        "RunTests" => Some(CriterionKind::RunTests),
        "Clippy" => Some(CriterionKind::Clippy),
        "CheckFormat" => Some(CriterionKind::CheckFormat),
        "Unknown" => Some(CriterionKind::Unknown),
        _ => None,
    }
}

fn decision_for_criterion_kind(
    kind: CriterionKind,
    request: &ModelRequest,
) -> Option<ModelDecision> {
    match kind {
        CriterionKind::Compile => Some(ModelDecision::Compile {
            code: request.working_code.clone().unwrap_or_default(),
        }),
        CriterionKind::Validate => Some(ModelDecision::Validate {
            request: request.user_request.clone(),
            code: request.working_code.clone(),
            plan_kind: request
                .plan_kind
                .clone()
                .unwrap_or_else(|| "Generic".to_string()),
        }),
        CriterionKind::RunTests => Some(ModelDecision::RunTests {
            filter: String::new(),
        }),
        CriterionKind::Clippy => Some(ModelDecision::RunClippy),
        CriterionKind::CheckFormat => Some(ModelDecision::CheckFormat),
        CriterionKind::Unknown => None,
    }
}

/// Redirige decisiones incompatibles con Goal/recommended_action (alias de validación amplia).
pub fn apply_gap_guidance(decision: ModelDecision, request: &ModelRequest) -> ModelDecision {
    validate_model_decision_against_recommendation(decision, request)
}

fn serialize_observation(observation: &AgentObservation) -> SerializedObservation {
    let evaluation_verdict = observation
        .evaluation_verdict()
        .map(|verdict| match verdict {
            EvaluationVerdict::Pass => "Pass".to_string(),
            EvaluationVerdict::Fail => "Fail".to_string(),
            EvaluationVerdict::InsufficientEvidence => "InsufficientEvidence".to_string(),
        });

    SerializedObservation {
        kind: observation_kind(observation),
        tool_name: observation.tool_name().map(str::to_string),
        success: match observation {
            AgentObservation::ToolOutcome { success, .. } => Some(*success),
            AgentObservation::CriterionEvaluated {
                verdict: EvaluationVerdict::Pass,
                ..
            }
            | AgentObservation::SpecificationEvaluated {
                status: crate::harness::SpecificationEvaluationStatus::Pass,
                ..
            }
            | AgentObservation::Finished { .. }
            | AgentObservation::NoOpDone => Some(true),
            AgentObservation::CriterionEvaluated { .. }
            | AgentObservation::SpecificationEvaluated { .. }
            | AgentObservation::ActionRejected { .. }
            | AgentObservation::UnknownTool { .. } => Some(false),
        },
        summary: observation.summary(),
        validator_errors: observation
            .validator_errors()
            .into_iter()
            .map(str::to_string)
            .collect(),
        repairer_feedback: observation
            .repairer_feedback()
            .into_iter()
            .map(str::to_string)
            .collect(),
        evidence_labels: match observation {
            AgentObservation::ToolOutcome { evidence, .. }
            | AgentObservation::CriterionEvaluated { evidence, .. } => {
                evidence.iter().map(|item| item.label.clone()).collect()
            }
            AgentObservation::SpecificationEvaluated { criteria, .. } => criteria
                .iter()
                .flat_map(|item| item.evidence_used.iter())
                .map(|item| item.label.clone())
                .collect(),
            _ => Vec::new(),
        },
        evidence_details: diagnostic_evidence_details(observation),
        evaluation_verdict,
        specification_id: observation
            .specification_id()
            .map(|id| id.as_str().to_string()),
        criterion_id: match observation {
            AgentObservation::CriterionEvaluated { criterion_id, .. } => {
                Some(criterion_id.as_str().to_string())
            }
            _ => None,
        },
        criterion_kind: observation
            .evaluation_kind()
            .map(|kind| format!("{kind:?}")),
        evaluation_message: match observation {
            AgentObservation::CriterionEvaluated { message, .. }
            | AgentObservation::SpecificationEvaluated { message, .. } => Some(message.clone()),
            _ => None,
        },
    }
}

fn diagnostic_evidence_details(observation: &AgentObservation) -> Vec<(String, String)> {
    let evidence: Vec<&crate::harness::evaluation::Evidence> = match observation {
        AgentObservation::ToolOutcome { evidence, .. }
        | AgentObservation::CriterionEvaluated { evidence, .. } => evidence.iter().collect(),
        AgentObservation::SpecificationEvaluated { criteria, .. } => criteria
            .iter()
            .flat_map(|item| item.evidence_used.iter())
            .collect(),
        _ => return Vec::new(),
    };

    evidence
        .iter()
        .filter(|item| is_diagnostic_evidence_label(&item.label))
        .map(|item| (item.label.clone(), truncate_diagnostic_detail(&item.detail)))
        .collect()
}

fn observation_kind(observation: &AgentObservation) -> String {
    match observation {
        AgentObservation::ToolOutcome { .. } => "tool_outcome".to_string(),
        AgentObservation::CriterionEvaluated { .. } => "criterion_evaluated".to_string(),
        AgentObservation::SpecificationEvaluated { .. } => "specification_evaluated".to_string(),
        AgentObservation::ActionRejected { .. } => "action_rejected".to_string(),
        AgentObservation::NoOpDone => "noop".to_string(),
        AgentObservation::Finished { .. } => "finished".to_string(),
        AgentObservation::UnknownTool { .. } => "unknown_tool".to_string(),
    }
}

fn collect_recent_evidence(ctx: &AgentContext) -> Vec<(String, String)> {
    ctx.observation_history
        .iter()
        .rev()
        .take(3)
        .flat_map(|observation| match observation {
            AgentObservation::ToolOutcome { evidence, .. }
            | AgentObservation::CriterionEvaluated { evidence, .. } => evidence
                .iter()
                .map(|item| (item.label.clone(), truncate_detail(&item.detail)))
                .collect(),
            AgentObservation::SpecificationEvaluated { criteria, .. } => criteria
                .iter()
                .flat_map(|item| item.evidence_used.iter())
                .map(|item| (item.label.clone(), truncate_detail(&item.detail)))
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn truncate_detail(text: &str) -> String {
    if text.chars().count() <= 200 {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(200).collect::<String>())
    }
}

/// Serializa una [`ModelDecision`] a JSON compacto (sin dependencias externas).
pub fn serialize_decision(decision: &ModelDecision) -> String {
    match decision {
        ModelDecision::Validate {
            request,
            code,
            plan_kind,
        } => {
            let code_json = code
                .as_ref()
                .map(|value| json_string(value))
                .unwrap_or_else(|| "null".to_string());
            format!(
                "{{\"action\":\"validate\",\"request\":{},\"plan_kind\":{},\"code\":{code_json}}}",
                json_string(request),
                json_string(plan_kind),
            )
        }
        ModelDecision::RepairDiagnostic { errors } => {
            let errors_json = errors
                .iter()
                .map(|value| json_string(value.as_str()))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"action\":\"repair_diagnostic\",\"errors\":[{errors_json}]}}")
        }
        ModelDecision::ApplyCorrection { corrections } => {
            let items = corrections
                .iter()
                .map(serialize_structured_correction)
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"action\":\"apply_correction\",\"corrections\":[{items}]}}")
        }
        ModelDecision::ApplyFileOperations { operations } => {
            let items = operations
                .iter()
                .map(serialize_structured_file_operation)
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"action\":\"apply_file_operations\",\"operations\":[{items}]}}")
        }
        ModelDecision::Compile { code } => {
            format!("{{\"action\":\"compile\",\"code\":{}}}", json_string(code))
        }
        ModelDecision::RunTests { filter } => {
            format!(
                "{{\"action\":\"run_tests\",\"filter\":{}}}",
                json_string(filter)
            )
        }
        ModelDecision::RunClippy => "{\"action\":\"run_clippy\"}".to_string(),
        ModelDecision::CheckFormat => "{\"action\":\"check_format\"}".to_string(),
        ModelDecision::Finish { summary } => {
            format!(
                "{{\"action\":\"finish\",\"summary\":{}}}",
                json_string(summary)
            )
        }
    }
}

fn serialize_structured_correction(correction: &StructuredCorrection) -> String {
    let path_json = structured_correction_path(correction)
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!(",\"path\":{}", json_string(value)))
        .unwrap_or_default();
    match correction {
        StructuredCorrection::ReplaceText {
            search,
            replacement,
            ..
        } => format!(
            "{{\"operation\":\"replace_text\"{path_json},\"search\":{},\"replacement\":{}}}",
            json_string(search),
            json_string(replacement)
        ),
        StructuredCorrection::InsertText { position, text, .. } => format!(
            "{{\"operation\":\"insert_text\"{path_json},\"position\":{position},\"text\":{}}}",
            json_string(text)
        ),
        StructuredCorrection::RemoveText { start, end, .. } => {
            format!("{{\"operation\":\"remove_text\"{path_json},\"start\":{start},\"end\":{end}}}")
        }
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::from('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Parsea y valida el texto del modelo antes de convertirlo en [`ModelDecision`].
pub fn parse_model_response(raw_text: &str) -> Result<ModelDecision, ModelResponseError> {
    let trimmed = raw_text.trim();
    let action = extract_string_field(trimmed, "action").ok_or_else(|| {
        ModelResponseError::InvalidModelResponse("campo action ausente".to_string())
    })?;

    match action.as_str() {
        "validate" => {
            let request = extract_string_field(trimmed, "request").ok_or_else(|| {
                ModelResponseError::InvalidModelResponse("validate sin request".to_string())
            })?;
            let plan_kind = extract_string_field(trimmed, "plan_kind").ok_or_else(|| {
                ModelResponseError::InvalidModelResponse("validate sin plan_kind".to_string())
            })?;
            let code = extract_optional_string_field(trimmed, "code");
            Ok(ModelDecision::Validate {
                request,
                code,
                plan_kind,
            })
        }
        "repair_diagnostic" => {
            let errors = extract_string_array(trimmed, "errors").ok_or_else(|| {
                ModelResponseError::InvalidModelResponse("repair_diagnostic sin errors".to_string())
            })?;
            if errors.is_empty() {
                return Err(ModelResponseError::InvalidModelResponse(
                    "repair_diagnostic requiere al menos un error".to_string(),
                ));
            }
            Ok(ModelDecision::RepairDiagnostic { errors })
        }
        "apply_correction" => {
            let corrections = parse_corrections_array(trimmed)?;
            if corrections.is_empty() {
                return Err(ModelResponseError::InvalidCorrection(
                    "apply_correction requiere al menos una corrección".to_string(),
                ));
            }
            Ok(ModelDecision::ApplyCorrection { corrections })
        }
        "apply_file_operations" => {
            let operations = parse_file_operations_array(trimmed)?;
            if operations.is_empty() {
                return Err(ModelResponseError::InvalidFileOperation(
                    "apply_file_operations requiere al menos una operación".to_string(),
                ));
            }
            Ok(ModelDecision::ApplyFileOperations { operations })
        }
        "compile" => {
            let code = extract_string_field(trimmed, "code").ok_or_else(|| {
                ModelResponseError::InvalidModelResponse("compile sin code".to_string())
            })?;
            Ok(ModelDecision::Compile { code })
        }
        "run_tests" => {
            let filter = extract_string_field(trimmed, "filter").unwrap_or_default();
            Ok(ModelDecision::RunTests { filter })
        }
        "run_clippy" => Ok(ModelDecision::RunClippy),
        "check_format" => Ok(ModelDecision::CheckFormat),
        "finish" => {
            let summary = extract_string_field(trimmed, "summary").ok_or_else(|| {
                ModelResponseError::InvalidModelResponse("finish sin summary".to_string())
            })?;
            Ok(ModelDecision::Finish { summary })
        }
        other => Err(ModelResponseError::UnsupportedAction(other.to_string())),
    }
}

fn parse_corrections_array(raw: &str) -> Result<Vec<StructuredCorrection>, ModelResponseError> {
    let array_body = extract_array_body(raw, "corrections")
        .ok_or_else(|| ModelResponseError::InvalidCorrection("corrections ausente".to_string()))?;
    let objects = split_top_level_objects(&array_body);
    objects
        .iter()
        .map(|item| parse_correction_object(item.as_str()))
        .collect()
}

fn parse_correction_object(raw: &str) -> Result<StructuredCorrection, ModelResponseError> {
    let operation = extract_string_field(raw, "operation")
        .ok_or_else(|| ModelResponseError::InvalidCorrection("operation ausente".to_string()))?;
    let path = extract_optional_string_field(raw, "path");
    match operation.as_str() {
        "replace_text" => {
            let search = extract_string_field(raw, "search").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("replace_text sin search".to_string())
            })?;
            let replacement = extract_string_field(raw, "replacement").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("replace_text sin replacement".to_string())
            })?;
            if search.is_empty() {
                return Err(ModelResponseError::InvalidCorrection(
                    "replace_text con search vacío".to_string(),
                ));
            }
            Ok(StructuredCorrection::ReplaceText {
                path,
                search,
                replacement,
            })
        }
        "insert_text" => {
            let position = extract_number_field(raw, "position").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("insert_text sin position".to_string())
            })?;
            let text = extract_string_field(raw, "text").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("insert_text sin text".to_string())
            })?;
            Ok(StructuredCorrection::InsertText {
                path,
                position,
                text,
            })
        }
        "remove_text" => {
            let start = extract_number_field(raw, "start").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("remove_text sin start".to_string())
            })?;
            let end = extract_number_field(raw, "end").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("remove_text sin end".to_string())
            })?;
            Ok(StructuredCorrection::RemoveText { path, start, end })
        }
        other => Err(ModelResponseError::InvalidCorrection(format!(
            "operation desconocida: {other}"
        ))),
    }
}

/// Convierte una decisión validada en [`Correction`] del Harness.
///
/// `path` inválido produce error; ausente → `Correction.path = None` (primary).
pub fn structured_to_correction(
    item: &StructuredCorrection,
) -> Result<Correction, ModelResponseError> {
    let artifact_path = match structured_correction_path(item) {
        None => None,
        Some(raw) if raw.trim().is_empty() => None,
        Some(raw) => Some(ArtifactPath::parse(raw).map_err(|message| {
            ModelResponseError::InvalidCorrection(format!("path inválido: {message}"))
        })?),
    };
    match item {
        StructuredCorrection::ReplaceText {
            search,
            replacement,
            ..
        } => Ok(Correction {
            target: CorrectionTarget::SessionCode,
            path: artifact_path,
            operation: CorrectionOperation::ReplaceText {
                search: search.clone(),
                replacement: replacement.clone(),
            },
        }),
        StructuredCorrection::InsertText { position, text, .. } => Ok(Correction {
            target: CorrectionTarget::SessionCode,
            path: artifact_path,
            operation: CorrectionOperation::InsertText {
                position: *position,
                text: text.clone(),
            },
        }),
        StructuredCorrection::RemoveText { start, end, .. } => Ok(Correction {
            target: CorrectionTarget::SessionCode,
            path: artifact_path,
            operation: CorrectionOperation::RemoveText {
                start: *start,
                end: *end,
            },
        }),
    }
}

fn serialize_structured_file_operation(operation: &StructuredFileOperation) -> String {
    match operation {
        StructuredFileOperation::CreateFile { path, source } => format!(
            "{{\"operation\":\"create_file\",\"path\":{},\"source\":{}}}",
            json_string(path),
            json_string(source)
        ),
        StructuredFileOperation::DeleteFile { path } => format!(
            "{{\"operation\":\"delete_file\",\"path\":{}}}",
            json_string(path)
        ),
        StructuredFileOperation::RenameFile { from, to } => format!(
            "{{\"operation\":\"rename_file\",\"from\":{},\"to\":{}}}",
            json_string(from),
            json_string(to)
        ),
    }
}

fn parse_file_operations_array(
    raw: &str,
) -> Result<Vec<StructuredFileOperation>, ModelResponseError> {
    let array_body = extract_array_body(raw, "operations").ok_or_else(|| {
        ModelResponseError::InvalidFileOperation("operations ausente".to_string())
    })?;
    let objects = split_top_level_objects(&array_body);
    objects
        .iter()
        .map(|item| parse_file_operation_object(item.as_str()))
        .collect()
}

fn parse_file_operation_object(raw: &str) -> Result<StructuredFileOperation, ModelResponseError> {
    let operation = extract_string_field(raw, "operation")
        .ok_or_else(|| ModelResponseError::InvalidFileOperation("operation ausente".to_string()))?;
    match operation.as_str() {
        "create_file" => {
            let path = extract_string_field(raw, "path").ok_or_else(|| {
                ModelResponseError::InvalidFileOperation("create_file sin path".to_string())
            })?;
            let source = extract_string_field(raw, "source").ok_or_else(|| {
                ModelResponseError::InvalidFileOperation("create_file sin source".to_string())
            })?;
            Ok(StructuredFileOperation::CreateFile { path, source })
        }
        "delete_file" => {
            let path = extract_string_field(raw, "path").ok_or_else(|| {
                ModelResponseError::InvalidFileOperation("delete_file sin path".to_string())
            })?;
            Ok(StructuredFileOperation::DeleteFile { path })
        }
        "rename_file" => {
            let from = extract_string_field(raw, "from").ok_or_else(|| {
                ModelResponseError::InvalidFileOperation("rename_file sin from".to_string())
            })?;
            let to = extract_string_field(raw, "to").ok_or_else(|| {
                ModelResponseError::InvalidFileOperation("rename_file sin to".to_string())
            })?;
            Ok(StructuredFileOperation::RenameFile { from, to })
        }
        other => Err(ModelResponseError::InvalidFileOperation(format!(
            "operation desconocida: {other}"
        ))),
    }
}

/// Convierte operaciones del modelo en [`ArtifactFileOperation`].
pub fn structured_to_file_operation(
    item: &StructuredFileOperation,
) -> Result<ArtifactFileOperation, ModelResponseError> {
    match item {
        StructuredFileOperation::CreateFile { path, source } => {
            let path = ArtifactPath::parse(path).map_err(|message| {
                ModelResponseError::InvalidFileOperation(format!("path inválido: {message}"))
            })?;
            Ok(ArtifactFileOperation::CreateFile {
                path,
                source: source.clone(),
            })
        }
        StructuredFileOperation::DeleteFile { path } => {
            let path = ArtifactPath::parse(path).map_err(|message| {
                ModelResponseError::InvalidFileOperation(format!("path inválido: {message}"))
            })?;
            Ok(ArtifactFileOperation::DeleteFile { path })
        }
        StructuredFileOperation::RenameFile { from, to } => {
            let from = ArtifactPath::parse(from).map_err(|message| {
                ModelResponseError::InvalidFileOperation(format!("from inválido: {message}"))
            })?;
            let to = ArtifactPath::parse(to).map_err(|message| {
                ModelResponseError::InvalidFileOperation(format!("to inválido: {message}"))
            })?;
            Ok(ArtifactFileOperation::RenameFile { from, to })
        }
    }
}

fn correction_target_source(
    correction: &StructuredCorrection,
    artifact: Option<&RustArtifact>,
) -> Result<Option<String>, ModelResponseError> {
    match structured_correction_path(correction) {
        None => Ok(artifact.map(|item| item.source().to_string())),
        Some(raw) if raw.trim().is_empty() => Ok(artifact.map(|item| item.source().to_string())),
        Some(raw) => {
            let path = ArtifactPath::parse(raw).map_err(|message| {
                ModelResponseError::InvalidCorrection(format!("path inválido: {message}"))
            })?;
            let Some(content) = artifact.and_then(|item| item.file(&path).map(str::to_string))
            else {
                return Err(ModelResponseError::InvalidCorrection(format!(
                    "archivo de corrección inexistente: {raw}"
                )));
            };
            Ok(Some(content))
        }
    }
}

/// Valida que ApplyCorrection no intente reemplazar el programa completo del archivo objetivo.
pub fn validate_apply_correction(
    corrections: &[StructuredCorrection],
    artifact: Option<&RustArtifact>,
) -> Result<(), ModelResponseError> {
    for correction in corrections {
        if let StructuredCorrection::ReplaceText {
            search,
            replacement,
            ..
        } = correction
        {
            if search.is_empty() {
                return Err(ModelResponseError::InvalidCorrection(
                    "search vacío".to_string(),
                ));
            }
            if let Some(code) = correction_target_source(correction, artifact)?
                && replacement.len() >= code.len()
                && search.len() < code.len() / 2
            {
                return Err(ModelResponseError::InvalidCorrection(
                    "reemplazo de programa completo no permitido".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn extract_string_field(raw: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\":");
    let start = raw.find(&pattern)? + pattern.len();
    parse_json_string_at(raw, start)
}

fn extract_optional_string_field(raw: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\":");
    let start = raw.find(&pattern)? + pattern.len();
    let slice = raw[start..].trim_start();
    if slice.starts_with("null") {
        return None;
    }
    parse_json_string_at(raw, start)
}

fn extract_number_field(raw: &str, field: &str) -> Option<usize> {
    let pattern = format!("\"{field}\":");
    let start = raw.find(&pattern)? + pattern.len();
    let slice = raw[start..].trim_start();
    let end = slice
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(slice.len());
    slice[..end].parse().ok()
}

fn extract_string_array(raw: &str, field: &str) -> Option<Vec<String>> {
    let body = extract_array_body(raw, field)?;
    if body.trim().is_empty() {
        return Some(Vec::new());
    }

    let mut results = Vec::new();
    let mut rest = body.as_str();
    while !rest.trim().is_empty() {
        rest = rest.trim_start().trim_start_matches(',');
        if rest.is_empty() {
            break;
        }
        let (value, next) = read_json_string(rest.as_bytes(), 0)?;
        results.push(value);
        rest = &rest[next..];
    }
    Some(results)
}

fn extract_array_body(raw: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\":");
    let start = raw.find(&pattern)? + pattern.len();
    let slice = raw[start..].trim_start();
    if !slice.starts_with('[') {
        return None;
    }
    let end = find_matching_bracket(slice, '[', ']')?;
    Some(slice[1..end].to_string())
}

fn split_top_level_objects(body: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut depth = 0;
    let mut start = None;
    for (index, ch) in body.char_indices() {
        match ch {
            '{' if depth == 0 => {
                start = Some(index);
                depth = 1;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0
                    && let Some(from) = start
                {
                    objects.push(body[from..=index].to_string());
                    start = None;
                }
            }
            _ => {}
        }
    }
    objects
}

fn split_top_level_strings(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let bytes = body.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() || bytes[index] == b',' {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        if bytes[index] == b'"' {
            if let Some((value, next)) = read_json_string(body.as_bytes(), index) {
                items.push(value);
                index = next;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    items
}

fn parse_json_string_at(raw: &str, start: usize) -> Option<String> {
    let slice = raw[start..].trim_start();
    read_json_string(slice.as_bytes(), 0).map(|(value, _)| value)
}

fn parse_json_string_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') {
        read_json_string(trimmed.as_bytes(), 0).map(|(value, _)| value)
    } else {
        None
    }
}

fn read_json_string(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    if bytes.get(start)? != &b'"' {
        return None;
    }
    let mut index = start + 1;
    let mut value = String::new();
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((value, index + 1)),
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index)?;
                value.push(match escaped {
                    b'"' => '"',
                    b'\\' => '\\',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    other => other as char,
                });
            }
            byte => value.push(byte as char),
        }
        index += 1;
    }
    None
}

fn find_matching_bracket(slice: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    for (index, ch) in slice.char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

/// Mock determinista y causal: la decisión depende del contenido de [`ModelRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockModelClient {
    pub force_invalid_response: bool,
}

impl MockModelClient {
    pub fn new() -> Self {
        Self {
            force_invalid_response: false,
        }
    }

    pub fn invalid() -> Self {
        Self {
            force_invalid_response: true,
        }
    }

    fn decision_for_request(&self, request: &ModelRequest) -> ModelDecision {
        if self.force_invalid_response {
            return ModelDecision::Finish {
                summary: "unused".to_string(),
            };
        }

        match &request.last_observation {
            None => {
                if let Some(rec) = &request.recommended_action
                    && rec.kind != "FinishAllowed"
                    && rec.kind != "NoDeterministicAction"
                    && let Some(decision) = model_decision_from_recommended_action(rec, request)
                {
                    return decision;
                }
                if let Some(gap) = &request.goal_gap
                    && let Some(decision) = decision_from_goal_gap(gap, request)
                {
                    return decision;
                }
                if request
                    .goal_evaluation
                    .as_ref()
                    .is_some_and(|eval| eval.status == "Satisfied")
                {
                    return ModelDecision::Finish {
                        summary: "ai mock session completed: goal satisfied".to_string(),
                    };
                }
                ModelDecision::Validate {
                    request: request.user_request.clone(),
                    code: request.working_code.clone(),
                    plan_kind: request
                        .plan_kind
                        .clone()
                        .unwrap_or_else(|| "Generic".to_string()),
                }
            }
            Some(obs) if obs.kind == "action_rejected" => {
                // Finish bloqueado por criterio en FAIL: reparar, no spamear Compile.
                if obs.summary.contains("en FAIL") {
                    ModelDecision::RepairDiagnostic {
                        errors: mock_repair_errors(request, obs),
                    }
                } else {
                    ModelDecision::Compile {
                        code: request.working_code.clone().unwrap_or_default(),
                    }
                }
            }
            Some(obs)
                if obs.kind == "criterion_evaluated"
                    && obs.evaluation_verdict.as_deref() == Some("Pass") =>
            {
                if request
                    .goal_evaluation
                    .as_ref()
                    .is_some_and(|eval| eval.status == "Satisfied")
                {
                    ModelDecision::Finish {
                        summary: "ai mock session completed after evaluation pass".to_string(),
                    }
                } else if let Some(decision) =
                    self.decision_from_recommendation_if_unsatisfied(request)
                {
                    decision
                } else {
                    ModelDecision::Finish {
                        summary: "ai mock session completed after evaluation pass".to_string(),
                    }
                }
            }
            Some(obs)
                if obs.kind == "criterion_evaluated"
                    && obs.evaluation_verdict.as_deref() == Some("Fail") =>
            {
                ModelDecision::RepairDiagnostic {
                    errors: mock_repair_errors(request, obs),
                }
            }
            Some(obs)
                if obs.tool_name.as_deref() == Some(VALIDATE) && obs.success == Some(false) =>
            {
                ModelDecision::RepairDiagnostic {
                    errors: mock_repair_errors(request, obs),
                }
            }
            Some(obs)
                if obs.tool_name.as_deref() == Some(REPAIR_DIAGNOSTIC)
                    && obs.success == Some(true) =>
            {
                let corrections = infer_corrections_from_diagnostic_context(request);
                if corrections.is_empty() {
                    ModelDecision::Compile {
                        code: request.working_code.clone().unwrap_or_default(),
                    }
                } else {
                    ModelDecision::ApplyCorrection { corrections }
                }
            }
            Some(obs)
                if obs.tool_name.as_deref() == Some(APPLY_CORRECTION)
                    && obs.success == Some(true) =>
            {
                if should_recompile_after_correction(request) {
                    ModelDecision::Compile {
                        code: request.working_code.clone().unwrap_or_default(),
                    }
                } else {
                    ModelDecision::Validate {
                        request: request.user_request.clone(),
                        code: request.working_code.clone(),
                        plan_kind: request
                            .plan_kind
                            .clone()
                            .unwrap_or_else(|| "Generic".to_string()),
                    }
                }
            }
            Some(obs)
                if obs.tool_name.as_deref() == Some(VALIDATE) && obs.success == Some(true) =>
            {
                ModelDecision::Compile {
                    code: request.working_code.clone().unwrap_or_default(),
                }
            }
            Some(obs) if obs.tool_name.as_deref() == Some(COMPILE) && obs.success == Some(true) => {
                if request
                    .goal_evaluation
                    .as_ref()
                    .is_some_and(|eval| eval.status == "Satisfied")
                {
                    ModelDecision::Finish {
                        summary: "ai mock session completed".to_string(),
                    }
                } else if let Some(decision) =
                    self.decision_from_recommendation_if_unsatisfied(request)
                {
                    decision
                } else {
                    ModelDecision::Finish {
                        summary: "ai mock session completed".to_string(),
                    }
                }
            }
            Some(_) => ModelDecision::Finish {
                summary: "ai mock stop".to_string(),
            },
        }
    }

    fn decision_from_recommendation_if_unsatisfied(
        &self,
        request: &ModelRequest,
    ) -> Option<ModelDecision> {
        let eval = request.goal_evaluation.as_ref()?;
        if eval.status == "Satisfied" {
            return None;
        }
        if let Some(rec) = &request.recommended_action
            && rec.kind != "FinishAllowed"
            && rec.kind != "NoDeterministicAction"
        {
            return model_decision_from_recommended_action(rec, request);
        }
        request
            .goal_gap
            .as_ref()
            .and_then(|gap| decision_from_goal_gap(gap, request))
    }
}

impl Default for MockModelClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelClient for MockModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        if self.force_invalid_response {
            return Ok(ModelResponse {
                raw_text: "{not valid json".to_string(),
            });
        }

        let decision = self.decision_for_request(request);
        Ok(ModelResponse {
            raw_text: serialize_decision(&decision),
        })
    }
}

fn mock_repair_errors(request: &ModelRequest, obs: &SerializedObservation) -> Vec<String> {
    let mut errors = obs
        .evidence_details
        .iter()
        .filter(|(label, _)| label == "compiler_stderr")
        .map(|(_, detail)| detail.clone())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        errors = request.diagnostic_context.compiler_stderr.clone();
    }
    if errors.is_empty() {
        errors = obs.validator_errors.clone();
    }
    if errors.is_empty() {
        for recent in request.recent_observations.iter().rev() {
            if !recent.validator_errors.is_empty() {
                errors = recent.validator_errors.clone();
                break;
            }
            let stderr: Vec<String> = recent
                .evidence_details
                .iter()
                .filter(|(label, _)| label == "compiler_stderr")
                .map(|(_, detail)| detail.clone())
                .collect();
            if !stderr.is_empty() {
                errors = stderr;
                break;
            }
        }
    }
    if errors.is_empty()
        && let Some(msg) = &obs.evaluation_message
        && !msg.is_empty()
    {
        errors.push(msg.clone());
    }
    if errors.is_empty() && !obs.summary.is_empty() {
        errors.push(obs.summary.clone());
    }
    if errors.is_empty() {
        errors = diagnostic_errors_from_request(request);
    }
    errors
}

fn should_recompile_after_correction(request: &ModelRequest) -> bool {
    if request.diagnostic_context.compile_status.as_deref() == Some("error") {
        return true;
    }
    if !request.diagnostic_context.compiler_stderr.is_empty() {
        return true;
    }
    request.recommended_action.as_ref().is_some_and(|rec| {
        rec.kind == "RepairDiagnostic"
            || rec.criterion_kind.as_deref() == Some("Compile")
            || rec.tool_name.as_deref() == Some(COMPILE)
    })
}

fn infer_corrections_from_diagnostic_context(request: &ModelRequest) -> Vec<StructuredCorrection> {
    if let Some(corrections) = infer_compile_corrections_from_diagnostics(request)
        && !corrections.is_empty()
    {
        return corrections;
    }
    infer_validation_corrections_from_diagnostics(request)
}

fn infer_compile_corrections_from_diagnostics(
    request: &ModelRequest,
) -> Option<Vec<StructuredCorrection>> {
    let stderr = request.diagnostic_context.compiler_stderr.join("\n");
    if stderr.trim().is_empty() {
        return None;
    }

    let feedback = request
        .last_observation
        .as_ref()
        .map(|obs| obs.repairer_feedback.clone())
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| request.diagnostic_context.repairer_feedback.clone());

    if feedback
        .iter()
        .any(|item| item.contains("delimitador") || item.contains("delimiter"))
        && let Some(code) = &request.working_code
    {
        let open = code.matches('{').count();
        let close = code.matches('}').count();
        if open > close {
            return Some(vec![StructuredCorrection::InsertText {
                path: None,
                position: code.len(),
                text: "\n}".repeat(open - close),
            }]);
        }
    }

    let mut corrections = Vec::new();
    for file in &request.artifact_files {
        if let Some(token) = extract_found_token_from_stderr(&stderr)
            && file.source.contains(&token)
        {
            let replacement = infer_replacement_for_token(&token, &file.source, &stderr);
            if !replacement.is_empty() && replacement != token {
                corrections.push(StructuredCorrection::ReplaceText {
                    path: Some(file.path.clone()),
                    search: token,
                    replacement,
                });
            }
        }
    }

    if corrections.is_empty()
        && let Some(code) = &request.working_code
        && let Some(token) = extract_found_token_from_stderr(&stderr)
        && code.contains(&token)
    {
        let replacement = infer_replacement_for_token(&token, code, &stderr);
        if !replacement.is_empty() && replacement != token {
            corrections.push(StructuredCorrection::ReplaceText {
                path: request.artifact_primary_path.clone(),
                search: token,
                replacement,
            });
        }
    }

    if corrections.is_empty() {
        None
    } else {
        Some(corrections)
    }
}

fn infer_validation_corrections_from_diagnostics(
    request: &ModelRequest,
) -> Vec<StructuredCorrection> {
    let code = request.working_code.as_deref().unwrap_or_default();
    let errors = if request.diagnostic_context.validator_errors.is_empty() {
        request
            .last_observation
            .as_ref()
            .map(|obs| obs.validator_errors.clone())
            .unwrap_or_default()
    } else {
        request.diagnostic_context.validator_errors.clone()
    };

    let mut corrections = Vec::new();
    for error in &errors {
        corrections.extend(infer_validation_marker_corrections(code, error));
    }
    corrections
}

fn infer_validation_marker_corrections(
    working_code: &str,
    error: &str,
) -> Vec<StructuredCorrection> {
    if !error.contains("API REST") {
        return Vec::new();
    }
    let pairs = [
        ("HTTP", "NET"),
        ("Endpoints", "Routes"),
        ("endpoint", "route"),
        ("/api", "/x"),
        ("GET", "READ"),
        ("POST", "WRITE"),
        ("Server", "Host"),
        ("server", "host"),
    ];
    pairs
        .iter()
        .filter(|(required, substitute)| {
            !working_code.contains(required) && working_code.contains(substitute)
        })
        .map(|(required, substitute)| StructuredCorrection::ReplaceText {
            path: None,
            search: (*substitute).to_string(),
            replacement: (*required).to_string(),
        })
        .collect()
}

fn extract_found_token_from_stderr(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        for marker in ["found `", "cannot find value `", "cannot find type `"] {
            if let Some(start) = line.find(marker) {
                let rest = &line[start + marker.len()..];
                if let Some(end) = rest.find('`') {
                    let token = rest[..end].to_string();
                    if !token.is_empty() {
                        return Some(token);
                    }
                }
            }
        }
    }
    None
}

fn infer_replacement_for_token(token: &str, source: &str, stderr: &str) -> String {
    if source.contains("-> i32") || source.contains("-> i64") || source.contains("-> u32") {
        return "0".to_string();
    }
    if source.contains("-> f32") || source.contains("-> f64") {
        return "0.0".to_string();
    }
    if source.contains("-> bool") {
        return "false".to_string();
    }
    if source.contains("-> &str") || source.contains("-> String") {
        return "\"\"".to_string();
    }
    if stderr.contains("unclosed delimiter") || stderr.contains("mismatched closing delimiter") {
        return String::new();
    }
    format!("/* fix {token} */")
}

/// Cliente mock genérico: decisiones derivadas de [`SerializedDiagnosticContext`]
/// y observaciones serializadas (sin reglas error→corrección opacas fuera del request).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticContextModelClient;

impl DiagnosticContextModelClient {
    pub fn new() -> Self {
        Self
    }
}

impl ModelClient for DiagnosticContextModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        MockModelClient::new().complete(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::EvaluationVerdict;
    use crate::harness::Evidence;

    #[test]
    fn model_client_trait_is_object_safe() {
        let _: Box<dyn ModelClient> = Box::new(MockModelClient::new());
    }

    #[test]
    fn model_request_from_context_includes_observation_and_code() {
        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());
        let mut ctx = AgentContext::new("ai").with_working_code("fn main() {}");
        ctx.step = 2;
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: VALIDATE.to_string(),
            success: false,
            output: "fail".to_string(),
            evidence: vec![Evidence::new("validator_error_0", "error api")],
            verdict: EvaluationVerdict::Fail,
        });

        let request = model_request_from_context(&ctx, &session).expect("request");
        assert_eq!(request.user_request, "Crear una API REST");
        assert_eq!(request.plan_kind.as_deref(), Some("Api"));
        assert_eq!(request.working_code.as_deref(), Some("fn main() {}"));
        assert_eq!(request.artifact_id.as_deref(), Some("artifact:main.rs"));
        assert_eq!(request.artifact_language.as_deref(), Some("Rust"));
        assert_eq!(request.artifact_revision, Some(0));
        assert!(request.last_observation.is_some());
        assert!(!request.recent_observations.is_empty());
        assert!(request.system_prompt.contains("validate"));
        assert!(request.system_prompt.contains("repair_diagnostic"));
        assert!(request.system_prompt.contains("artifact_files"));
    }

    #[test]
    fn model_request_from_context_includes_multi_file_artifact_tree() {
        use crate::harness::artifact::{ArtifactId, RustArtifact};
        use crate::harness::artifact_path::ArtifactPath;

        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        let artifact = RustArtifact::try_from_files(
            ArtifactId::new("art-model-contract"),
            "main.rs",
            main.clone(),
            [
                (main, "mod helper;\nfn main() {}\n".to_string()),
                (helper, "pub fn value() -> i32 { 1 }\n".to_string()),
            ],
        )
        .unwrap();
        let session = AiSessionConfig::new("Corregir helper".to_string(), "Api".to_string());
        let ctx = AgentContext::new("ai").with_working_artifact(artifact);

        let request = model_request_from_context(&ctx, &session).expect("request");
        assert_eq!(
            request.artifact_primary_path.as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(request.artifact_files.len(), 2);
        assert_eq!(request.artifact_files[0].path, "src/main.rs");
        assert_eq!(request.artifact_files[1].path, "src/helper.rs");
        assert_eq!(
            request.working_code.as_deref(),
            Some("mod helper;\nfn main() {}\n")
        );
    }

    #[test]
    fn append_artifact_files_to_message_parts_serializes_tree() {
        let files = vec![
            ArtifactFileSnapshot {
                path: "src/main.rs".to_string(),
                source: "fn main() {}".to_string(),
            },
            ArtifactFileSnapshot {
                path: "src/lib.rs".to_string(),
                source: "pub fn ok() {}".to_string(),
            },
        ];
        let mut parts = Vec::new();
        append_artifact_files_to_message_parts(&mut parts, Some("src/main.rs"), &files);
        let message = parts.join("\n");
        assert!(message.contains("artifact_primary_path=src/main.rs"));
        assert!(message.contains("artifact_file_count=2"));
        assert!(message.contains("artifact_file_0_path=src/main.rs"));
        assert!(message.contains("artifact_file_1_path=src/lib.rs"));
        assert!(message.contains("artifact_file_1_source=pub fn ok() {}"));
    }

    #[test]
    fn parse_valid_decisions() {
        let validate = parse_model_response(
            r#"{"action":"validate","request":"r","plan_kind":"Api","code":null}"#,
        )
        .expect("validate");
        assert!(matches!(validate, ModelDecision::Validate { .. }));

        let repair = parse_model_response(r#"{"action":"repair_diagnostic","errors":["e1"]}"#)
            .expect("repair");
        assert!(matches!(repair, ModelDecision::RepairDiagnostic { .. }));

        let correction = parse_model_response(
            r#"{"action":"apply_correction","corrections":[{"operation":"replace_text","search":"NET","replacement":"HTTP"}]}"#,
        )
        .expect("correction");
        assert!(matches!(correction, ModelDecision::ApplyCorrection { .. }));
    }

    #[test]
    fn parse_quality_decisions() {
        let tests = parse_model_response(r#"{"action":"run_tests","filter":"my_filter"}"#)
            .expect("run_tests");
        match tests {
            ModelDecision::RunTests { filter } => assert_eq!(filter, "my_filter"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            parse_model_response(r#"{"action":"run_clippy"}"#).expect("clippy"),
            ModelDecision::RunClippy
        ));
        assert!(matches!(
            parse_model_response(r#"{"action":"check_format"}"#).expect("fmt"),
            ModelDecision::CheckFormat
        ));
        let empty_filter =
            parse_model_response(r#"{"action":"run_tests"}"#).expect("run_tests default filter");
        assert!(matches!(
            empty_filter,
            ModelDecision::RunTests { filter } if filter.is_empty()
        ));
    }

    #[test]
    fn quality_decisions_round_trip_serialize_parse() {
        for decision in [
            ModelDecision::RunTests {
                filter: "suite::case".to_string(),
            },
            ModelDecision::RunClippy,
            ModelDecision::CheckFormat,
        ] {
            let raw = serialize_decision(&decision);
            let parsed = parse_model_response(&raw).expect("round-trip");
            assert_eq!(parsed, decision, "raw={raw}");
        }
    }

    #[test]
    fn parse_unknown_action_keeps_unsupported_error() {
        let err = parse_model_response(r#"{"action":"launch_missiles"}"#).unwrap_err();
        assert!(matches!(
            err,
            ModelResponseError::UnsupportedAction(name) if name == "launch_missiles"
        ));
    }

    #[test]
    fn system_prompt_in_model_request_lists_quality_actions() {
        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());
        let request = model_request_from_context(
            &AgentContext::new("ai").with_working_code("fn main() {}"),
            &session,
        )
        .expect("request");
        assert!(request.system_prompt.contains("run_tests"));
        assert!(request.system_prompt.contains("run_clippy"));
        assert!(request.system_prompt.contains("check_format"));
    }

    #[test]
    fn parse_rejects_invalid_response() {
        let err = parse_model_response("{bad").unwrap_err();
        assert!(matches!(err, ModelResponseError::InvalidModelResponse(_)));
    }

    #[test]
    fn apply_correction_rejects_full_program_replace() {
        let code = "short";
        let artifact = RustArtifact::new("main.rs", code);
        let corrections = vec![StructuredCorrection::ReplaceText {
            path: None,
            search: "x".to_string(),
            replacement: "a very long replacement".to_string(),
        }];
        let err = validate_apply_correction(&corrections, Some(&artifact)).unwrap_err();
        assert!(matches!(err, ModelResponseError::InvalidCorrection(_)));
    }

    #[test]
    fn mock_model_client_changes_decision_with_observation() {
        let client = MockModelClient::new();
        let session = AiSessionConfig::new("Crear una API REST".to_string(), "Api".to_string());

        let initial =
            model_request_from_context(&AgentContext::new("ai").with_working_code("NET"), &session)
                .expect("initial");
        let first = parse_model_response(&client.complete(&initial).expect("resp").raw_text)
            .expect("decision");
        assert!(matches!(first, ModelDecision::Validate { .. }));

        let mut ctx = AgentContext::new("ai").with_working_code("NET");
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: VALIDATE.to_string(),
            success: false,
            output: "fail".to_string(),
            evidence: vec![Evidence::new("validator_error_0", "API REST")],
            verdict: EvaluationVerdict::Fail,
        });
        let fail_req = model_request_from_context(&ctx, &session).expect("fail");
        let second = parse_model_response(&client.complete(&fail_req).expect("resp").raw_text)
            .expect("decision");
        assert!(matches!(second, ModelDecision::RepairDiagnostic { .. }));
    }

    fn compile_only_spec() -> crate::harness::specification::Specification {
        use crate::harness::criterion::CriterionKind;
        use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};

        Specification::new("spec-model-gap", "El código debe compilar")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    #[test]
    fn model_request_includes_goal_gap_when_unsatisfied() {
        let session = AiSessionConfig::new("compilar".to_string(), "Generic".to_string());
        let ctx = AgentContext::new("gap-test")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_only_spec());

        let request = model_request_from_context(&ctx, &session).expect("request");
        let eval = request.goal_evaluation.expect("goal_evaluation");
        assert_eq!(eval.status, "Inconclusive");
        assert_eq!(eval.criteria_total, 1);
        assert_eq!(eval.criteria_pass, 0);

        let gap = request.goal_gap.expect("goal_gap");
        assert_eq!(gap.unsatisfied_count, 1);
        assert_eq!(gap.gaps[0].criterion_id, "ac-compile");
        assert_eq!(gap.gaps[0].kind, "Compile");
        assert_eq!(gap.gaps[0].suggested_action.as_deref(), Some(COMPILE));
    }

    #[test]
    fn model_request_reflects_satisfied_goal() {
        use crate::harness::tools::COMPILE;

        let session = AiSessionConfig::new("compilar".to_string(), "Generic".to_string());
        let mut ctx = AgentContext::new("gap-satisfied")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_only_spec());
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

        let request = model_request_from_context(&ctx, &session).expect("request");
        let eval = request.goal_evaluation.expect("goal_evaluation");
        assert_eq!(eval.status, "Satisfied");
        assert_eq!(eval.criteria_pass, 1);
        assert!(request.goal_gap.is_none());
    }

    #[test]
    fn append_goal_context_serializes_gap_fields() {
        let request = ModelRequest {
            goal: "test".to_string(),
            step: 1,
            user_request: "r".to_string(),
            plan_kind: Some("Api".to_string()),
            working_code: Some("fn main() {}".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec-model-gap".to_string(),
                status: "Inconclusive".to_string(),
                criteria_total: 1,
                criteria_pass: 0,
                criteria_fail: 0,
                criteria_insufficient: 1,
                message: "evidencia insuficiente".to_string(),
            }),
            goal_gap: Some(SerializedGoalGap {
                unsatisfied_count: 1,
                gaps: vec![SerializedCriterionGap {
                    criterion_id: "ac-compile".to_string(),
                    kind: "Compile".to_string(),
                    verdict: "InsufficientEvidence".to_string(),
                    message: "falta compile".to_string(),
                    suggested_action: Some(COMPILE.to_string()),
                }],
            }),
            recommended_action: Some(SerializedRecommendedAction {
                kind: "InvokeTool".to_string(),
                tool_name: Some(COMPILE.to_string()),
                criterion_id: Some("ac-compile".to_string()),
                criterion_kind: Some("Compile".to_string()),
                priority: 0,
                reason: "evidencia insuficiente".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        };
        let mut parts = Vec::new();
        append_goal_context_to_message_parts(&mut parts, &request);
        let message = parts.join("\n");
        assert!(message.contains("goal_evaluation_status=Inconclusive"));
        assert!(message.contains("goal_gap_unsatisfied_count=1"));
        assert!(message.contains("goal_gap_0_criterion_id=ac-compile"));
        assert!(message.contains("goal_gap_0_suggested_action=compile"));
        assert!(message.contains("recommended_action_kind=InvokeTool"));
        assert!(message.contains("recommended_action_tool=compile"));
        assert!(message.contains("recommended_action_directive=MUST_FOLLOW_WHEN_GOAL_UNSATISFIED"));
        assert!(message.contains("recommended_action_criterion_kind=Compile"));
    }

    fn compile_invoke_request() -> ModelRequest {
        ModelRequest {
            goal: "test".to_string(),
            step: 1,
            user_request: "compilar".to_string(),
            plan_kind: Some("Generic".to_string()),
            working_code: Some("fn main() {}".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec".to_string(),
                status: "Inconclusive".to_string(),
                criteria_total: 1,
                criteria_pass: 0,
                criteria_fail: 0,
                criteria_insufficient: 1,
                message: "pending".to_string(),
            }),
            goal_gap: Some(SerializedGoalGap {
                unsatisfied_count: 1,
                gaps: vec![SerializedCriterionGap {
                    criterion_id: "ac-compile".to_string(),
                    kind: "Compile".to_string(),
                    verdict: "InsufficientEvidence".to_string(),
                    message: "falta".to_string(),
                    suggested_action: Some(COMPILE.to_string()),
                }],
            }),
            recommended_action: Some(SerializedRecommendedAction {
                kind: "InvokeTool".to_string(),
                tool_name: Some(COMPILE.to_string()),
                criterion_id: Some("ac-compile".to_string()),
                criterion_kind: Some("Compile".to_string()),
                priority: 0,
                reason: "evidencia insuficiente".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        }
    }

    #[test]
    fn finish_allowed_accepts_finish_decision() {
        let request = ModelRequest {
            goal: "test".to_string(),
            step: 3,
            user_request: "compilar".to_string(),
            plan_kind: Some("Generic".to_string()),
            working_code: Some("fn main() {}".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec".to_string(),
                status: "Satisfied".to_string(),
                criteria_total: 1,
                criteria_pass: 1,
                criteria_fail: 0,
                criteria_insufficient: 0,
                message: "ok".to_string(),
            }),
            goal_gap: None,
            recommended_action: Some(SerializedRecommendedAction {
                kind: "FinishAllowed".to_string(),
                tool_name: None,
                criterion_id: None,
                criterion_kind: None,
                priority: 0,
                reason: "goal satisfecha".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        };
        let finish = ModelDecision::Finish {
            summary: "done".to_string(),
        };
        let validated = validate_model_decision_against_recommendation(finish.clone(), &request);
        assert_eq!(validated, finish);
        assert!(decision_is_compatible_with_recommendation(
            &finish,
            request.recommended_action.as_ref().unwrap()
        ));
    }

    #[test]
    fn validate_redirects_incompatible_validate_when_compile_recommended() {
        let request = compile_invoke_request();
        let incompatible = ModelDecision::Validate {
            request: "r".to_string(),
            code: Some("fn main() {}".to_string()),
            plan_kind: "Generic".to_string(),
        };
        assert!(!decision_is_compatible_with_recommendation(
            &incompatible,
            request.recommended_action.as_ref().unwrap()
        ));
        let validated = validate_model_decision_against_recommendation(incompatible, &request);
        assert!(matches!(validated, ModelDecision::Compile { .. }));
    }

    #[test]
    fn validate_keeps_compatible_repair_diagnostic() {
        let request = ModelRequest {
            goal: "test".to_string(),
            step: 2,
            user_request: "compilar".to_string(),
            plan_kind: Some("Generic".to_string()),
            working_code: Some("fn main() { broken".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec".to_string(),
                status: "Unsatisfied".to_string(),
                criteria_total: 1,
                criteria_pass: 0,
                criteria_fail: 1,
                criteria_insufficient: 0,
                message: "compile fail".to_string(),
            }),
            goal_gap: None,
            recommended_action: Some(SerializedRecommendedAction {
                kind: "RepairDiagnostic".to_string(),
                tool_name: Some(REPAIR_DIAGNOSTIC.to_string()),
                criterion_id: Some("ac-compile".to_string()),
                criterion_kind: Some("Compile".to_string()),
                priority: 0,
                reason: "compilación fallida".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        };
        let repair = ModelDecision::RepairDiagnostic {
            errors: vec!["expected `}`".to_string()],
        };
        let validated = validate_model_decision_against_recommendation(repair.clone(), &request);
        assert_eq!(validated, repair);
    }

    #[test]
    fn validate_redirects_finish_when_compile_recommended() {
        let request = compile_invoke_request();
        let validated = validate_model_decision_against_recommendation(
            ModelDecision::Finish {
                summary: "too early".to_string(),
            },
            &request,
        );
        assert!(matches!(validated, ModelDecision::Compile { .. }));
    }

    #[test]
    fn no_deterministic_action_marks_finish_incompatible() {
        let rec = SerializedRecommendedAction {
            kind: "NoDeterministicAction".to_string(),
            tool_name: None,
            criterion_id: None,
            criterion_kind: None,
            priority: u8::MAX,
            reason: "sin acción determinista".to_string(),
        };
        let finish = ModelDecision::Finish {
            summary: "blocked".to_string(),
        };
        assert!(!decision_is_compatible_with_recommendation(&finish, &rec));
        assert!(decision_is_compatible_with_recommendation(
            &ModelDecision::Validate {
                request: "r".to_string(),
                code: None,
                plan_kind: "Api".to_string(),
            },
            &rec
        ));
    }

    #[test]
    fn mock_model_client_uses_goal_gap_for_initial_action() {
        let client = MockModelClient::new();
        let session = AiSessionConfig::new("compilar".to_string(), "Generic".to_string());
        let ctx = AgentContext::new("gap-mock")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_only_spec());
        let request = model_request_from_context(&ctx, &session).expect("request");
        let decision =
            parse_model_response(&client.complete(&request).expect("resp").raw_text).expect("dec");
        assert!(matches!(decision, ModelDecision::Compile { .. }));
    }

    #[test]
    fn apply_gap_guidance_redirects_premature_finish() {
        let request = ModelRequest {
            goal: "test".to_string(),
            step: 2,
            user_request: "compilar".to_string(),
            plan_kind: Some("Generic".to_string()),
            working_code: Some("fn main() {}".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec".to_string(),
                status: "Inconclusive".to_string(),
                criteria_total: 1,
                criteria_pass: 0,
                criteria_fail: 0,
                criteria_insufficient: 1,
                message: "pending".to_string(),
            }),
            goal_gap: Some(SerializedGoalGap {
                unsatisfied_count: 1,
                gaps: vec![SerializedCriterionGap {
                    criterion_id: "ac-compile".to_string(),
                    kind: "Compile".to_string(),
                    verdict: "InsufficientEvidence".to_string(),
                    message: "falta".to_string(),
                    suggested_action: Some(COMPILE.to_string()),
                }],
            }),
            recommended_action: Some(SerializedRecommendedAction {
                kind: "InvokeTool".to_string(),
                tool_name: Some(COMPILE.to_string()),
                criterion_id: Some("ac-compile".to_string()),
                criterion_kind: Some("Compile".to_string()),
                priority: 0,
                reason: "evidencia insuficiente para ac-compile".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        };
        let guided = apply_gap_guidance(
            ModelDecision::Finish {
                summary: "too early".to_string(),
            },
            &request,
        );
        assert!(matches!(guided, ModelDecision::Compile { .. }));
    }

    #[test]
    fn decision_from_goal_gap_maps_validate_kind() {
        let request = ModelRequest {
            goal: "test".to_string(),
            step: 1,
            user_request: "validar".to_string(),
            plan_kind: Some("Api".to_string()),
            working_code: Some("code".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: None,
            goal_gap: Some(SerializedGoalGap {
                unsatisfied_count: 1,
                gaps: vec![SerializedCriterionGap {
                    criterion_id: "ac-v".to_string(),
                    kind: "Validate".to_string(),
                    verdict: "InsufficientEvidence".to_string(),
                    message: "falta".to_string(),
                    suggested_action: Some(VALIDATE.to_string()),
                }],
            }),
            recommended_action: Some(SerializedRecommendedAction {
                kind: "InvokeTool".to_string(),
                tool_name: Some(VALIDATE.to_string()),
                criterion_id: Some("ac-v".to_string()),
                criterion_kind: Some("Validate".to_string()),
                priority: 1,
                reason: "evidencia insuficiente".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        };
        let gap = request.goal_gap.as_ref().expect("gap");
        let decision = decision_from_goal_gap(gap, &request).expect("decision");
        assert!(matches!(decision, ModelDecision::Validate { .. }));
    }

    #[test]
    fn model_request_includes_recommended_action() {
        let session = AiSessionConfig::new("compilar".to_string(), "Generic".to_string());
        let ctx = AgentContext::new("rec-test")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(compile_only_spec());

        let request = model_request_from_context(&ctx, &session).expect("request");
        let rec = request.recommended_action.expect("recommended_action");
        assert_eq!(rec.kind, "InvokeTool");
        assert_eq!(rec.tool_name.as_deref(), Some(COMPILE));
        assert_eq!(rec.priority, 0);
    }

    #[test]
    fn apply_gap_guidance_uses_recommended_action_not_hardcoded_kind() {
        // E — RepairDiagnostic cuando compile falló
        let request = ModelRequest {
            goal: "test".to_string(),
            step: 2,
            user_request: "compilar".to_string(),
            plan_kind: Some("Generic".to_string()),
            working_code: Some("fn main() { broken".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec".to_string(),
                status: "Unsatisfied".to_string(),
                criteria_total: 1,
                criteria_pass: 0,
                criteria_fail: 1,
                criteria_insufficient: 0,
                message: "compile fail".to_string(),
            }),
            goal_gap: Some(SerializedGoalGap {
                unsatisfied_count: 1,
                gaps: vec![SerializedCriterionGap {
                    criterion_id: "ac-compile".to_string(),
                    kind: "Compile".to_string(),
                    verdict: "Fail".to_string(),
                    message: "error".to_string(),
                    suggested_action: Some(COMPILE.to_string()),
                }],
            }),
            recommended_action: Some(SerializedRecommendedAction {
                kind: "RepairDiagnostic".to_string(),
                tool_name: Some(REPAIR_DIAGNOSTIC.to_string()),
                criterion_id: Some("ac-compile".to_string()),
                criterion_kind: Some("Compile".to_string()),
                priority: 0,
                reason: "compilación fallida".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        };
        let guided = apply_gap_guidance(
            ModelDecision::Finish {
                summary: "too early".to_string(),
            },
            &request,
        );
        assert!(matches!(guided, ModelDecision::RepairDiagnostic { .. }));
    }

    #[test]
    fn apply_gap_guidance_allows_finish_when_goal_satisfied() {
        // F
        let request = ModelRequest {
            goal: "test".to_string(),
            step: 3,
            user_request: "compilar".to_string(),
            plan_kind: Some("Generic".to_string()),
            working_code: Some("fn main() {}".to_string()),
            artifact_id: None,
            artifact_language: None,
            artifact_revision: None,
            artifact_primary_path: None,
            artifact_files: Vec::new(),
            last_observation: None,
            recent_observations: Vec::new(),
            recent_evidence: Vec::new(),
            goal_evaluation: Some(SerializedGoalEvaluation {
                goal_id: "spec".to_string(),
                status: "Satisfied".to_string(),
                criteria_total: 1,
                criteria_pass: 1,
                criteria_fail: 0,
                criteria_insufficient: 0,
                message: "ok".to_string(),
            }),
            goal_gap: None,
            recommended_action: Some(SerializedRecommendedAction {
                kind: "FinishAllowed".to_string(),
                tool_name: None,
                criterion_id: None,
                criterion_kind: None,
                priority: 0,
                reason: "goal satisfecha".to_string(),
            }),
            diagnostic_context: SerializedDiagnosticContext::default(),
            system_prompt: String::new(),
        };
        let finish = ModelDecision::Finish {
            summary: "done".to_string(),
        };
        let guided = apply_gap_guidance(finish.clone(), &request);
        assert_eq!(guided, finish);
    }
}
