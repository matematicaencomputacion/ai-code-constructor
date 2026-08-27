//! Abstracción model-agnostic entre [`crate::harness::AiAgent`] y proveedores futuros.
//!
//! ModelClient no conoce Harness, Tools ni componentes del Constructor.

use crate::harness::context::AgentContext;
use crate::harness::correction::{Correction, CorrectionOperation, CorrectionTarget};
use crate::harness::evaluation::EvaluationVerdict;
use crate::harness::observation::AgentObservation;
use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC, VALIDATE};

/// Configuración de sesión que AiAgent necesita para serializar contexto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSessionConfig {
    pub user_request: String,
    pub plan_kind: String,
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
    /// Verdict de Evaluation (`Pass` / `Fail` / `InsufficientEvidence`), si aplica.
    pub evaluation_verdict: Option<String>,
    pub specification_id: Option<String>,
    pub criterion_id: Option<String>,
    pub criterion_kind: Option<String>,
    pub evaluation_message: Option<String>,
}

/// Petición estructurada enviada al modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub goal: String,
    pub step: u32,
    pub user_request: String,
    pub plan_kind: Option<String>,
    pub working_code: Option<String>,
    pub artifact_id: Option<String>,
    pub artifact_language: Option<String>,
    pub artifact_revision: Option<u64>,
    pub last_observation: Option<SerializedObservation>,
    pub recent_observations: Vec<SerializedObservation>,
    pub recent_evidence: Vec<(String, String)>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum StructuredCorrection {
    ReplaceText { search: String, replacement: String },
    InsertText { position: usize, text: String },
    RemoveText { start: usize, end: usize },
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
        last_observation,
        recent_observations,
        recent_evidence,
        system_prompt: crate::harness::agent_prompt::system_prompt_v1().to_string(),
    })
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
    match correction {
        StructuredCorrection::ReplaceText {
            search,
            replacement,
        } => format!(
            "{{\"operation\":\"replace_text\",\"search\":{},\"replacement\":{}}}",
            json_string(search),
            json_string(replacement)
        ),
        StructuredCorrection::InsertText { position, text } => format!(
            "{{\"operation\":\"insert_text\",\"position\":{position},\"text\":{}}}",
            json_string(text)
        ),
        StructuredCorrection::RemoveText { start, end } => {
            format!("{{\"operation\":\"remove_text\",\"start\":{start},\"end\":{end}}}")
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
            Ok(StructuredCorrection::InsertText { position, text })
        }
        "remove_text" => {
            let start = extract_number_field(raw, "start").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("remove_text sin start".to_string())
            })?;
            let end = extract_number_field(raw, "end").ok_or_else(|| {
                ModelResponseError::InvalidCorrection("remove_text sin end".to_string())
            })?;
            Ok(StructuredCorrection::RemoveText { start, end })
        }
        other => Err(ModelResponseError::InvalidCorrection(format!(
            "operation desconocida: {other}"
        ))),
    }
}

/// Convierte una decisión validada en [`Correction`] del Harness.
pub fn structured_to_correction(item: &StructuredCorrection) -> Correction {
    match item {
        StructuredCorrection::ReplaceText {
            search,
            replacement,
        } => Correction {
            target: CorrectionTarget::SessionCode,
            operation: CorrectionOperation::ReplaceText {
                search: search.clone(),
                replacement: replacement.clone(),
            },
        },
        StructuredCorrection::InsertText { position, text } => Correction {
            target: CorrectionTarget::SessionCode,
            operation: CorrectionOperation::InsertText {
                position: *position,
                text: text.clone(),
            },
        },
        StructuredCorrection::RemoveText { start, end } => Correction {
            target: CorrectionTarget::SessionCode,
            operation: CorrectionOperation::RemoveText {
                start: *start,
                end: *end,
            },
        },
    }
}

/// Valida que ApplyCorrection no intente reemplazar el programa completo.
pub fn validate_apply_correction(
    corrections: &[StructuredCorrection],
    working_code: Option<&str>,
) -> Result<(), ModelResponseError> {
    if let Some(code) = working_code {
        for correction in corrections {
            if let StructuredCorrection::ReplaceText {
                search,
                replacement,
            } = correction
            {
                if search.is_empty() {
                    return Err(ModelResponseError::InvalidCorrection(
                        "search vacío".to_string(),
                    ));
                }
                if replacement.len() >= code.len() && search.len() < code.len() / 2 {
                    return Err(ModelResponseError::InvalidCorrection(
                        "reemplazo de programa completo no permitido".to_string(),
                    ));
                }
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
            None => ModelDecision::Validate {
                request: request.user_request.clone(),
                code: request.working_code.clone(),
                plan_kind: request
                    .plan_kind
                    .clone()
                    .unwrap_or_else(|| "Generic".to_string()),
            },
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
                ModelDecision::Finish {
                    summary: "ai mock session completed after evaluation pass".to_string(),
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
                let corrections = infer_mock_corrections(request);
                ModelDecision::ApplyCorrection { corrections }
            }
            Some(obs)
                if obs.tool_name.as_deref() == Some(APPLY_CORRECTION)
                    && obs.success == Some(true) =>
            {
                ModelDecision::Validate {
                    request: request.user_request.clone(),
                    code: request.working_code.clone(),
                    plan_kind: request
                        .plan_kind
                        .clone()
                        .unwrap_or_else(|| "Generic".to_string()),
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
                ModelDecision::Finish {
                    summary: "ai mock session completed".to_string(),
                }
            }
            Some(_) => ModelDecision::Finish {
                summary: "ai mock stop".to_string(),
            },
        }
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
    let mut errors = obs.validator_errors.clone();
    if errors.is_empty() {
        for recent in request.recent_observations.iter().rev() {
            if !recent.validator_errors.is_empty() {
                errors = recent.validator_errors.clone();
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
        errors.push("mock repair: validation failed".to_string());
    }
    errors
}

fn infer_mock_corrections(request: &ModelRequest) -> Vec<StructuredCorrection> {
    let code = request.working_code.as_deref().unwrap_or_default();
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
        .filter(|(required, substitute)| !code.contains(required) && code.contains(substitute))
        .map(|(required, substitute)| StructuredCorrection::ReplaceText {
            search: (*substitute).to_string(),
            replacement: (*required).to_string(),
        })
        .collect()
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
        let session = AiSessionConfig {
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        };
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
        let session = AiSessionConfig {
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        };
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
        let corrections = vec![StructuredCorrection::ReplaceText {
            search: "x".to_string(),
            replacement: "a very long replacement".to_string(),
        }];
        let err = validate_apply_correction(&corrections, Some(code)).unwrap_err();
        assert!(matches!(err, ModelResponseError::InvalidCorrection(_)));
    }

    #[test]
    fn mock_model_client_changes_decision_with_observation() {
        let client = MockModelClient::new();
        let session = AiSessionConfig {
            user_request: "Crear una API REST".to_string(),
            plan_kind: "Api".to_string(),
        };

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
}
