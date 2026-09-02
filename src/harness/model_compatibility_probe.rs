//! Certificación acotada de compatibilidad entre modelos y el Harness.
//!
//! Las llamadas externas solo se ejecutan con un LiveProbePermit explícito.
//! Los reportes conservan categorías, contadores y nombres de acciones, pero no
//! prompts, respuestas crudas, credenciales ni cuerpos HTTP.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::harness::action_policy::{
    ActionPolicy, ApplyCorrectionConstraint, ApplyFileOperationsConstraint,
    ArtifactStateConstraint, FinishConstraint, RepairDiagnosticConstraint,
};
use crate::harness::adaptive_recovery::{
    AdaptiveRecoveryAction, AdaptiveRecoveryBudget, AdaptiveRecoveryReason, plan_adaptive_recovery,
};
use crate::harness::agent_loop::{AgentLoop, LoopResult, LoopStatus};
use crate::harness::ai_agent::AiAgent;
use crate::harness::artifact_path::ArtifactPath;
use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::failure_classification::{
    FailureClass, RecoveryBudget, RecoveryPlanReason, RecoveryStrategy, classify_model_error,
    plan_recovery,
};
use crate::harness::live_session::LiveSessionConfig;
use crate::harness::model::{
    AiSessionConfig, ArtifactFileSnapshot, ModelClient, ModelDecision, ModelError, ModelRequest,
    ModelResponse, parse_model_response,
};
use crate::harness::model_compatibility_scheduler::{
    ProbeScheduleOutcome, ProbeScheduler, ProbeSchedulerConfig,
};
use crate::harness::openai_compatible_client::{
    ModelClientConfig, OpenAICompatibleModelClient, ResponseFormatMode,
};
use crate::harness::runtime::Harness;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tool_permission::ToolPermissionConstraint;
use crate::harness::tools::{
    APPLY_CORRECTION, APPLY_FILE_OPERATIONS, COMPILE, CorrectionTool, FileOperationsTool,
    REPAIR_DIAGNOSTIC, RepairDiagnosticTool, VALIDATE, ValidationTool,
};

pub const NVIDIA_DEFAULT_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const NVIDIA_API_KEY_ENV: &str = "NVIDIA_API_KEY";
pub(crate) const MODEL_COMPATIBILITY_SUITE_VERSION: &str = "model-compatibility-v2";
pub const NVIDIA_DEFAULT_MODELS: [&str; 4] = [
    "moonshotai/kimi-k3",
    "deepseek-ai/deepseek-v4-pro-0813",
    "nvidia/nemotron-3.5-lightning-30b-a3b",
    "nvidia/nemotron-3-ultra-550b-a55b",
];

const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_ITERATIONS: u32 = 12;

/// Compile sintético exclusivo del probe.
///
/// Inspecciona el Artifact en memoria y nunca materializa archivos ni inicia
/// procesos. La certificación de ejecución real permanece fuera de alcance
/// hasta que exista un SandboxRunner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProbeCompileTool;

impl Tool for ProbeCompileTool {
    fn name(&self) -> &str {
        COMPILE
    }

    fn execute(&self, _input: &str, ctx: &AgentContext) -> ToolResult {
        let Some(artifact) = ctx.working_artifact.as_ref() else {
            return ToolResult::failure(
                format!("working_artifact ausente para tool `{COMPILE}`"),
                vec![
                    Evidence::new("tool", COMPILE),
                    Evidence::new("compile_status", "error"),
                    Evidence::new("missing_artifact", "working_artifact required"),
                ],
            );
        };

        for (path, source) in artifact.files() {
            if source.lines().any(|line| line.trim() == "broken") {
                let stderr = format!(
                    "error[E0425]: cannot find value `broken` in this scope\n --> {}:1:1",
                    path.as_str(),
                );
                return ToolResult::failure(
                    stderr.clone(),
                    vec![
                        Evidence::new("tool", COMPILE),
                        Evidence::new("compile_status", "error"),
                        Evidence::new("compiler_stderr", stderr),
                    ],
                );
            }
        }

        ToolResult::success(
            "compilación sintética exitosa".to_string(),
            vec![
                Evidence::new("tool", COMPILE),
                Evidence::new("compile_status", "ok"),
                Evidence::new("execution_mode", "synthetic_no_process"),
            ],
        )
    }
}

fn build_probe_repair_harness() -> Harness {
    let policy = ActionPolicy::new()
        .with_constraint(Box::new(ToolPermissionConstraint::new([
            VALIDATE,
            REPAIR_DIAGNOSTIC,
            APPLY_CORRECTION,
            APPLY_FILE_OPERATIONS,
            COMPILE,
        ])))
        .with_constraint(Box::new(ArtifactStateConstraint))
        .with_constraint(Box::new(RepairDiagnosticConstraint))
        .with_constraint(Box::new(ApplyCorrectionConstraint))
        .with_constraint(Box::new(ApplyFileOperationsConstraint))
        .with_constraint(Box::new(FinishConstraint));
    let mut harness = Harness::new(DEFAULT_MAX_ITERATIONS);
    harness.register_tool(Box::new(ValidationTool));
    harness.register_tool(Box::new(RepairDiagnosticTool));
    harness.register_tool(Box::new(CorrectionTool));
    harness.register_tool(Box::new(FileOperationsTool));
    harness.register_tool(Box::new(ProbeCompileTool));
    harness.register_constraint(Box::new(policy));
    harness
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeProfile {
    Smoke,
    Certify,
}

impl ProbeProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Certify => "certify",
        }
    }

    fn samples(self) -> u32 {
        match self {
            Self::Smoke => 1,
            Self::Certify => 3,
        }
    }

    fn default_max_calls(self) -> u32 {
        match self {
            Self::Smoke => 32,
            Self::Certify => 96,
        }
    }

    fn default_max_elapsed(self) -> Duration {
        match self {
            Self::Smoke => Duration::from_secs(180),
            Self::Certify => Duration::from_secs(600),
        }
    }
}

impl std::str::FromStr for ProbeProfile {
    type Err = ProbeCliError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.to_ascii_lowercase().as_str() {
            "smoke" => Ok(Self::Smoke),
            "certify" => Ok(Self::Certify),
            _ => Err(ProbeCliError::InvalidValue {
                flag: "--profile",
                expected: "smoke|certify",
            }),
        }
    }
}

/// Target público y deliberadamente libre de credenciales.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeTarget {
    pub provider: String,
    pub model: String,
    pub base_url: String,
}

impl ProbeTarget {
    pub fn nvidia(
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Result<Self, ProbeSetupError> {
        Self::new("nvidia", model, base_url)
    }

    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Result<Self, ProbeSetupError> {
        let provider = provider.into();
        let model = model.into();
        let base_url = base_url.into();
        if provider.trim().is_empty() || model.trim().is_empty() || base_url.trim().is_empty() {
            return Err(ProbeSetupError::InvalidTarget);
        }
        if base_url.contains('@') || base_url.contains('?') || base_url.contains('#') {
            return Err(ProbeSetupError::UnsafeBaseUrl);
        }
        if base_url.starts_with("http://") {
            return Err(ProbeSetupError::UnsafeBaseUrl);
        }
        if !base_url.starts_with("https://") {
            return Err(ProbeSetupError::InvalidTarget);
        }
        Ok(Self {
            provider,
            model,
            base_url,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeConfig {
    pub profile: ProbeProfile,
    pub timeout: Duration,
    pub max_calls: u32,
    pub max_iterations: u32,
    pub max_elapsed: Duration,
    pub max_response_bytes: usize,
}

impl ProbeConfig {
    pub fn new(
        profile: ProbeProfile,
        timeout: Duration,
        max_calls: u32,
    ) -> Result<Self, ProbeSetupError> {
        if timeout.is_zero() || max_calls == 0 {
            return Err(ProbeSetupError::InvalidLimit);
        }
        Ok(Self {
            profile,
            timeout,
            max_calls,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_elapsed: profile.default_max_elapsed(),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        })
    }

    pub fn for_profile(profile: ProbeProfile, timeout: Duration) -> Result<Self, ProbeSetupError> {
        Self::new(profile, timeout, profile.default_max_calls())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveProbePermit {
    max_calls: u32,
}

impl LiveProbePermit {
    pub fn acknowledge_external_calls_and_costs(max_calls: u32) -> Result<Self, ProbeSetupError> {
        if max_calls == 0 {
            return Err(ProbeSetupError::InvalidLimit);
        }
        Ok(Self { max_calls })
    }

    pub fn max_calls(self) -> u32 {
        self.max_calls
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeVerdict {
    Pass,
    Fail,
    Blocked,
    Inconclusive,
    NotTested,
}

impl ProbeVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Blocked => "blocked",
            Self::Inconclusive => "inconclusive",
            Self::NotTested => "not_tested",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeLayer {
    Adapter,
    Model,
    Harness,
    External,
    Budget,
}

impl ProbeLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Adapter => "adapter",
            Self::Model => "model",
            Self::Harness => "harness",
            Self::External => "external",
            Self::Budget => "budget",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeGate {
    TransportPromptOnly,
    JsonObjectStrict,
    ActionValidate,
    ActionRepairDiagnostic,
    ActionApplyCorrection,
    ActionApplyFileOperations,
    ActionCompile,
    ActionRunTests,
    ActionRunClippy,
    ActionCheckFormat,
    ActionFinish,
    AutonomousRepair,
    MultiFile,
    BoundedConvergence,
    RetryAfterSynthetic,
    NativeToolCalling,
}

impl ProbeGate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TransportPromptOnly => "transport_prompt_only",
            Self::JsonObjectStrict => "json_object_strict",
            Self::ActionValidate => "action_validate",
            Self::ActionRepairDiagnostic => "action_repair_diagnostic",
            Self::ActionApplyCorrection => "action_apply_correction",
            Self::ActionApplyFileOperations => "action_apply_file_operations",
            Self::ActionCompile => "action_compile",
            Self::ActionRunTests => "action_run_tests",
            Self::ActionRunClippy => "action_run_clippy",
            Self::ActionCheckFormat => "action_check_format",
            Self::ActionFinish => "action_finish",
            Self::AutonomousRepair => "autonomous_repair",
            Self::MultiFile => "multi_file",
            Self::BoundedConvergence => "bounded_convergence",
            Self::RetryAfterSynthetic => "retry_after_synthetic",
            Self::NativeToolCalling => "native_tool_calling",
        }
    }

    fn affects_overall(self) -> bool {
        !matches!(self, Self::NativeToolCalling)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeExecutionUnit {
    RetryAfterSynthetic,
    Transport,
    JsonObjectStrict,
    ActionValidate,
    ActionRepairDiagnostic,
    ActionApplyCorrection,
    ActionApplyFileOperations,
    ActionCompile,
    ActionRunTests,
    ActionRunClippy,
    ActionCheckFormat,
    ActionFinish,
    RepairBundle,
    NativeToolCalling,
}

impl ProbeExecutionUnit {
    pub(crate) const ORDERED: [Self; 14] = [
        Self::RetryAfterSynthetic,
        Self::Transport,
        Self::JsonObjectStrict,
        Self::ActionValidate,
        Self::ActionRepairDiagnostic,
        Self::ActionApplyCorrection,
        Self::ActionApplyFileOperations,
        Self::ActionCompile,
        Self::ActionRunTests,
        Self::ActionRunClippy,
        Self::ActionCheckFormat,
        Self::ActionFinish,
        Self::RepairBundle,
        Self::NativeToolCalling,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RetryAfterSynthetic => "retry_after_synthetic",
            Self::Transport => "transport_prompt_only",
            Self::JsonObjectStrict => "json_object_strict",
            Self::ActionValidate => "action_validate",
            Self::ActionRepairDiagnostic => "action_repair_diagnostic",
            Self::ActionApplyCorrection => "action_apply_correction",
            Self::ActionApplyFileOperations => "action_apply_file_operations",
            Self::ActionCompile => "action_compile",
            Self::ActionRunTests => "action_run_tests",
            Self::ActionRunClippy => "action_run_clippy",
            Self::ActionCheckFormat => "action_check_format",
            Self::ActionFinish => "action_finish",
            Self::RepairBundle => "repair_bundle",
            Self::NativeToolCalling => "native_tool_calling",
        }
    }

    pub(crate) fn is_external(self) -> bool {
        !matches!(self, Self::RetryAfterSynthetic | Self::NativeToolCalling)
    }

    fn expected_action(self) -> Option<ExpectedAction> {
        match self {
            Self::ActionValidate => Some(ExpectedAction::Validate),
            Self::ActionRepairDiagnostic => Some(ExpectedAction::RepairDiagnostic),
            Self::ActionApplyCorrection => Some(ExpectedAction::ApplyCorrection),
            Self::ActionApplyFileOperations => Some(ExpectedAction::ApplyFileOperations),
            Self::ActionCompile => Some(ExpectedAction::Compile),
            Self::ActionRunTests => Some(ExpectedAction::RunTests),
            Self::ActionRunClippy => Some(ExpectedAction::RunClippy),
            Self::ActionCheckFormat => Some(ExpectedAction::CheckFormat),
            Self::ActionFinish => Some(ExpectedAction::Finish),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProbePauseSignal {
    pub reason_code: &'static str,
    pub retry_after: Option<Duration>,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeUnitExecution {
    pub gates: Vec<ProbeGateResult>,
    pub calls_used: u32,
    pub active_elapsed: Duration,
    pub pause: Option<ProbePauseSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeMetric {
    pub name: String,
    pub value: u64,
}

impl ProbeMetric {
    fn new(name: impl Into<String>, value: u64) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeGateResult {
    pub gate: ProbeGate,
    pub verdict: ProbeVerdict,
    pub layer: ProbeLayer,
    pub reason_code: String,
    pub metrics: Vec<ProbeMetric>,
}

impl ProbeGateResult {
    fn single(
        gate: ProbeGate,
        verdict: ProbeVerdict,
        layer: ProbeLayer,
        reason_code: impl Into<String>,
    ) -> Self {
        Self {
            gate,
            verdict,
            layer,
            reason_code: reason_code.into(),
            metrics: Vec::new(),
        }
    }

    fn with_metric(mut self, name: impl Into<String>, value: u64) -> Self {
        self.metrics.push(ProbeMetric::new(name, value));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCompatibilityReport {
    pub suite_version: String,
    pub provider: String,
    pub model: String,
    pub profile: ProbeProfile,
    pub calls_used: u32,
    pub calls_limit: u32,
    pub gates: Vec<ProbeGateResult>,
}

impl ModelCompatibilityReport {
    pub fn overall_verdict(&self) -> ProbeVerdict {
        let relevant: Vec<_> = self
            .gates
            .iter()
            .filter(|gate| gate.gate.affects_overall())
            .collect();
        if relevant
            .iter()
            .any(|gate| gate.verdict == ProbeVerdict::Fail)
        {
            return ProbeVerdict::Fail;
        }
        if relevant
            .iter()
            .any(|gate| gate.verdict == ProbeVerdict::Blocked)
        {
            return ProbeVerdict::Blocked;
        }
        if relevant
            .iter()
            .any(|gate| gate.verdict == ProbeVerdict::Inconclusive)
        {
            return ProbeVerdict::Inconclusive;
        }
        if relevant
            .iter()
            .any(|gate| gate.verdict == ProbeVerdict::NotTested)
        {
            return ProbeVerdict::Inconclusive;
        }
        if !relevant.is_empty()
            && relevant
                .iter()
                .all(|gate| gate.verdict == ProbeVerdict::Pass)
        {
            ProbeVerdict::Pass
        } else {
            ProbeVerdict::NotTested
        }
    }

    pub fn to_text(&self) -> String {
        let mut lines = vec![
            "MODEL COMPATIBILITY PROBE".to_string(),
            format!("suite_version={}", self.suite_version),
            format!("provider={}", self.provider),
            format!("model={}", self.model),
            format!("profile={}", self.profile.as_str()),
            format!("calls={}/{}", self.calls_used, self.calls_limit),
            format!("overall={}", self.overall_verdict().as_str()),
        ];
        for gate in &self.gates {
            lines.push(format!(
                "gate={} verdict={} layer={} reason={}",
                gate.gate.as_str(),
                gate.verdict.as_str(),
                gate.layer.as_str(),
                gate.reason_code
            ));
        }
        lines.join("\n")
    }

    pub fn to_json_value(&self) -> Value {
        let gates: Vec<Value> = self
            .gates
            .iter()
            .map(|gate| {
                let metrics: BTreeMap<&str, u64> = gate
                    .metrics
                    .iter()
                    .map(|metric| (metric.name.as_str(), metric.value))
                    .collect();
                json!({
                    "gate": gate.gate.as_str(),
                    "verdict": gate.verdict.as_str(),
                    "layer": gate.layer.as_str(),
                    "reason_code": gate.reason_code,
                    "metrics": metrics,
                })
            })
            .collect();
        json!({
            "suite_version": self.suite_version,
            "provider": self.provider,
            "model": self.model,
            "profile": self.profile.as_str(),
            "calls_used": self.calls_used,
            "calls_limit": self.calls_limit,
            "overall_verdict": self.overall_verdict().as_str(),
            "gates": gates,
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_json_value())
            .expect("Probe report uses only serializable JSON values")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictJsonError {
    ResponseTooLarge,
    InvalidJson,
    NotObject,
    InvalidDecision,
}

pub fn parse_strict_model_decision(
    raw: &str,
    max_response_bytes: usize,
) -> Result<ModelDecision, StrictJsonError> {
    if raw.len() > max_response_bytes {
        return Err(StrictJsonError::ResponseTooLarge);
    }
    let value: Value = serde_json::from_str(raw).map_err(|_| StrictJsonError::InvalidJson)?;
    let object = value.as_object().ok_or(StrictJsonError::NotObject)?;
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .ok_or(StrictJsonError::InvalidDecision)?;
    let required_fields: &[&str] = match action {
        "validate" => &["request", "plan_kind", "code"],
        "repair_diagnostic" => &["errors"],
        "apply_correction" => &["corrections"],
        "apply_file_operations" => &["operations"],
        "compile" => &["code"],
        "run_tests" => &["filter"],
        "run_clippy" | "check_format" => &[],
        "finish" => &["summary"],
        _ => &[],
    };

    let mut strict_object = serde_json::Map::new();
    strict_object.insert("action".to_string(), Value::String(action.to_string()));
    for field in required_fields {
        let field_value = object.get(*field).ok_or(StrictJsonError::InvalidDecision)?;
        strict_object.insert((*field).to_string(), field_value.clone());
    }
    if action == "validate"
        && !matches!(
            strict_object.get("code"),
            Some(Value::Null | Value::String(_))
        )
    {
        return Err(StrictJsonError::InvalidDecision);
    }
    if action == "run_tests" && !matches!(strict_object.get("filter"), Some(Value::String(_))) {
        return Err(StrictJsonError::InvalidDecision);
    }

    let canonical = serde_json::to_string(&Value::Object(strict_object))
        .map_err(|_| StrictJsonError::InvalidJson)?;
    parse_model_response(&canonical).map_err(|_| StrictJsonError::InvalidDecision)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeSetupError {
    InvalidTarget,
    UnsafeBaseUrl,
    InvalidLimit,
}

impl fmt::Display for ProbeSetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget => write!(f, "target de probe inválido"),
            Self::UnsafeBaseUrl => {
                write!(
                    f,
                    "base URL insegura: se requiere HTTPS y no se permiten credenciales, query o fragment"
                )
            }
            Self::InvalidLimit => write!(f, "límite de probe inválido"),
        }
    }
}

#[derive(Debug, Clone)]
struct CallBudget {
    inner: Arc<Mutex<CallBudgetState>>,
}

#[derive(Debug, Clone, Copy)]
struct CallBudgetState {
    used: u32,
    max: u32,
    rejected: bool,
    started: Instant,
    max_elapsed: Duration,
    pause: Option<ProbePauseSignal>,
}

impl CallBudget {
    fn new(max: u32, max_elapsed: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CallBudgetState {
                used: 0,
                max,
                rejected: false,
                started: Instant::now(),
                max_elapsed,
                pause: None,
            })),
        }
    }

    fn consume(&self) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        if state.used >= state.max || state.started.elapsed() >= state.max_elapsed {
            state.rejected = true;
            return false;
        }
        state.used = state.used.saturating_add(1);
        true
    }

    fn remaining(&self) -> Duration {
        self.inner
            .lock()
            .map(|state| state.max_elapsed.saturating_sub(state.started.elapsed()))
            .unwrap_or(Duration::ZERO)
    }

    fn rejection_reason(&self) -> &'static str {
        self.inner
            .lock()
            .map(|state| {
                if state.started.elapsed() >= state.max_elapsed {
                    "wall_clock_limit_reached"
                } else {
                    "call_limit_reached"
                }
            })
            .unwrap_or("budget_state_unavailable")
    }
    fn used(&self) -> u32 {
        self.inner.lock().map(|state| state.used).unwrap_or(0)
    }

    fn max(&self) -> u32 {
        self.inner.lock().map(|state| state.max).unwrap_or(0)
    }

    fn rejected(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.rejected)
            .unwrap_or(true)
    }

    fn record_model_error(&self, error: &ModelError) {
        let pause = match error {
            ModelError::RateLimited { retry_after, .. } => Some(ProbePauseSignal {
                reason_code: "rate_limited",
                retry_after: *retry_after,
                http_status: Some(429),
            }),
            ModelError::Timeout => Some(ProbePauseSignal {
                reason_code: "timeout",
                retry_after: None,
                http_status: None,
            }),
            ModelError::Transport { .. } => Some(ProbePauseSignal {
                reason_code: "transport_failure",
                retry_after: None,
                http_status: None,
            }),
            ModelError::Http {
                status,
                retry_after,
                ..
            } if *status >= 500 => Some(ProbePauseSignal {
                reason_code: "provider_server_error",
                retry_after: *retry_after,
                http_status: Some(*status),
            }),
            _ => None,
        };
        if let Some(pause) = pause
            && let Ok(mut state) = self.inner.lock()
        {
            state.pause = Some(pause);
        }
    }

    fn pause(&self) -> Option<ProbePauseSignal> {
        self.inner.lock().ok().and_then(|state| state.pause)
    }
}

struct BudgetedModelClient {
    config: ModelClientConfig,
    response_format_mode: ResponseFormatMode,
    budget: CallBudget,
}

impl BudgetedModelClient {
    fn new(
        config: ModelClientConfig,
        response_format_mode: ResponseFormatMode,
        budget: CallBudget,
    ) -> Self {
        Self {
            config,
            response_format_mode,
            budget,
        }
    }
}

impl ModelClient for BudgetedModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        if self.budget.pause().is_some() {
            return Err(ModelError::Configuration(
                "model compatibility probe external circuit is open".to_string(),
            ));
        }
        if !self.budget.consume() {
            return Err(ModelError::Configuration(
                "model compatibility probe budget exhausted".to_string(),
            ));
        }
        let remaining = self.budget.remaining();
        if remaining.is_zero() {
            return Err(ModelError::Timeout);
        }
        let mut config = self.config.clone();
        config.timeout = config.timeout.min(remaining);
        let result = OpenAICompatibleModelClient::new(config)
            .with_response_format_mode(self.response_format_mode)
            .complete(request);
        if let Err(error) = &result {
            self.budget.record_model_error(error);
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SampleOutcome {
    verdict: ProbeVerdict,
    layer: ProbeLayer,
    reason_code: &'static str,
}

impl SampleOutcome {
    fn pass() -> Self {
        Self {
            verdict: ProbeVerdict::Pass,
            layer: ProbeLayer::Model,
            reason_code: "sample_passed",
        }
    }

    fn budget(reason_code: &'static str) -> Self {
        Self {
            verdict: ProbeVerdict::NotTested,
            layer: ProbeLayer::Budget,
            reason_code,
        }
    }
}

fn sample_from_model_error(error: &ModelError) -> SampleOutcome {
    match error {
        ModelError::Configuration(_) => SampleOutcome {
            verdict: ProbeVerdict::Blocked,
            layer: ProbeLayer::Adapter,
            reason_code: "adapter_configuration",
        },
        ModelError::Authentication(_) => SampleOutcome {
            verdict: ProbeVerdict::Blocked,
            layer: ProbeLayer::External,
            reason_code: "authentication_rejected",
        },
        ModelError::RateLimited { .. } => SampleOutcome {
            verdict: ProbeVerdict::Inconclusive,
            layer: ProbeLayer::External,
            reason_code: "rate_limited",
        },
        ModelError::Timeout => SampleOutcome {
            verdict: ProbeVerdict::Inconclusive,
            layer: ProbeLayer::External,
            reason_code: "timeout",
        },
        ModelError::Transport { .. } => SampleOutcome {
            verdict: ProbeVerdict::Inconclusive,
            layer: ProbeLayer::External,
            reason_code: "transport_failure",
        },
        ModelError::Http { status, .. } if *status >= 500 => SampleOutcome {
            verdict: ProbeVerdict::Inconclusive,
            layer: ProbeLayer::External,
            reason_code: "provider_server_error",
        },
        ModelError::Http { .. } => SampleOutcome {
            verdict: ProbeVerdict::Fail,
            layer: ProbeLayer::Adapter,
            reason_code: "request_feature_rejected",
        },
        ModelError::InvalidResponse(_) => SampleOutcome {
            verdict: ProbeVerdict::Fail,
            layer: ProbeLayer::Adapter,
            reason_code: "response_envelope_invalid",
        },
    }
}

fn aggregate_samples(
    gate: ProbeGate,
    expected: u32,
    outcomes: Vec<SampleOutcome>,
) -> ProbeGateResult {
    let run = outcomes
        .iter()
        .filter(|outcome| outcome.verdict != ProbeVerdict::NotTested)
        .count() as u32;
    let passed = outcomes
        .iter()
        .filter(|outcome| outcome.verdict == ProbeVerdict::Pass)
        .count() as u32;

    let decisive = if let Some(outcome) = outcomes
        .iter()
        .find(|outcome| outcome.verdict == ProbeVerdict::Fail)
    {
        outcome.clone()
    } else if passed == expected && outcomes.len() as u32 == expected {
        SampleOutcome::pass()
    } else if let Some(outcome) = outcomes
        .iter()
        .find(|outcome| outcome.verdict == ProbeVerdict::Blocked)
    {
        if passed == 0 {
            outcome.clone()
        } else {
            SampleOutcome {
                verdict: ProbeVerdict::Inconclusive,
                layer: outcome.layer,
                reason_code: "partial_external_block",
            }
        }
    } else if let Some(outcome) = outcomes
        .iter()
        .find(|outcome| outcome.verdict == ProbeVerdict::Inconclusive)
    {
        outcome.clone()
    } else if outcomes
        .iter()
        .any(|outcome| outcome.verdict == ProbeVerdict::NotTested)
    {
        SampleOutcome {
            verdict: if run == 0 {
                ProbeVerdict::NotTested
            } else {
                ProbeVerdict::Inconclusive
            },
            layer: ProbeLayer::Budget,
            reason_code: "call_limit_reached",
        }
    } else {
        SampleOutcome {
            verdict: ProbeVerdict::Inconclusive,
            layer: ProbeLayer::Harness,
            reason_code: "insufficient_samples",
        }
    };

    ProbeGateResult::single(
        gate,
        decisive.verdict,
        decisive.layer,
        if decisive.verdict == ProbeVerdict::Pass {
            "all_samples_passed"
        } else {
            decisive.reason_code
        },
    )
    .with_metric("samples_expected", expected as u64)
    .with_metric("samples_run", run as u64)
    .with_metric("samples_passed", passed as u64)
}

fn prerequisite_not_tested(gate: ProbeGate, layer: ProbeLayer) -> ProbeGateResult {
    ProbeGateResult::single(
        gate,
        ProbeVerdict::NotTested,
        layer,
        "prerequisite_not_satisfied",
    )
}

fn opens_external_circuit(result: &ProbeGateResult) -> bool {
    result.layer == ProbeLayer::External
        && matches!(
            result.verdict,
            ProbeVerdict::Blocked | ProbeVerdict::Inconclusive
        )
}

#[derive(Debug, Clone, Copy)]
enum ExpectedAction {
    Validate,
    RepairDiagnostic,
    ApplyCorrection,
    ApplyFileOperations,
    Compile,
    RunTests,
    RunClippy,
    CheckFormat,
    Finish,
}

impl ExpectedAction {
    fn gate(self) -> ProbeGate {
        match self {
            Self::Validate => ProbeGate::ActionValidate,
            Self::RepairDiagnostic => ProbeGate::ActionRepairDiagnostic,
            Self::ApplyCorrection => ProbeGate::ActionApplyCorrection,
            Self::ApplyFileOperations => ProbeGate::ActionApplyFileOperations,
            Self::Compile => ProbeGate::ActionCompile,
            Self::RunTests => ProbeGate::ActionRunTests,
            Self::RunClippy => ProbeGate::ActionRunClippy,
            Self::CheckFormat => ProbeGate::ActionCheckFormat,
            Self::Finish => ProbeGate::ActionFinish,
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Self::Validate => {
                r#"Return exactly one JSON object: {"action":"validate","request":"probe","plan_kind":"Generic","code":null}"#
            }
            Self::RepairDiagnostic => {
                r#"Return exactly one JSON object: {"action":"repair_diagnostic","errors":["error: broken"]}"#
            }
            Self::ApplyCorrection => {
                r#"Return exactly one JSON object: {"action":"apply_correction","corrections":[{"operation":"replace_text","path":"src/helper.rs","search":"broken","replacement":"0"}]}"#
            }
            Self::ApplyFileOperations => {
                r#"Return exactly one JSON object: {"action":"apply_file_operations","operations":[{"operation":"create_file","path":"src/probe.rs","source":"pub fn ok() {}"}]}"#
            }
            Self::Compile => {
                r#"Return exactly one JSON object: {"action":"compile","code":"fn main() {}"}"#
            }
            Self::RunTests => {
                r#"Return exactly one JSON object: {"action":"run_tests","filter":"probe"}"#
            }
            Self::RunClippy => r#"Return exactly one JSON object: {"action":"run_clippy"}"#,
            Self::CheckFormat => r#"Return exactly one JSON object: {"action":"check_format"}"#,
            Self::Finish => {
                r#"Return exactly one JSON object: {"action":"finish","summary":"probe complete"}"#
            }
        }
    }

    fn matches(self, decision: &ModelDecision) -> bool {
        matches!(
            (self, decision),
            (Self::Validate, ModelDecision::Validate { .. })
                | (
                    Self::RepairDiagnostic,
                    ModelDecision::RepairDiagnostic { .. }
                )
                | (Self::ApplyCorrection, ModelDecision::ApplyCorrection { .. })
                | (
                    Self::ApplyFileOperations,
                    ModelDecision::ApplyFileOperations { .. }
                )
                | (Self::Compile, ModelDecision::Compile { .. })
                | (Self::RunTests, ModelDecision::RunTests { .. })
                | (Self::RunClippy, ModelDecision::RunClippy)
                | (Self::CheckFormat, ModelDecision::CheckFormat)
                | (Self::Finish, ModelDecision::Finish { .. })
        )
    }
}

const EXPECTED_ACTIONS: [ExpectedAction; 9] = [
    ExpectedAction::Validate,
    ExpectedAction::RepairDiagnostic,
    ExpectedAction::ApplyCorrection,
    ExpectedAction::ApplyFileOperations,
    ExpectedAction::Compile,
    ExpectedAction::RunTests,
    ExpectedAction::RunClippy,
    ExpectedAction::CheckFormat,
    ExpectedAction::Finish,
];

fn probe_request(instruction: &str) -> ModelRequest {
    ModelRequest {
        goal: "model-compatibility-probe".to_string(),
        step: 1,
        user_request: instruction.to_string(),
        plan_kind: Some("Generic".to_string()),
        working_code: Some("mod helper;\nfn main() {}\n".to_string()),
        artifact_id: Some("artifact:compatibility-probe".to_string()),
        artifact_language: Some("Rust".to_string()),
        artifact_revision: Some(0),
        artifact_primary_path: Some("src/main.rs".to_string()),
        artifact_files: vec![
            ArtifactFileSnapshot {
                path: "src/main.rs".to_string(),
                source: "mod helper;\nfn main() {}\n".to_string(),
            },
            ArtifactFileSnapshot {
                path: "src/helper.rs".to_string(),
                source: "pub fn value() -> i32 { broken }\n".to_string(),
            },
        ],
        last_observation: None,
        recent_observations: Vec::new(),
        recent_evidence: Vec::new(),
        goal_evaluation: None,
        goal_gap: None,
        recommended_action: None,
        diagnostic_context: Default::default(),
        system_prompt: instruction.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct ModelCompatibilityProbe {
    pub target: ProbeTarget,
    pub config: ProbeConfig,
}

impl ModelCompatibilityProbe {
    pub fn new(target: ProbeTarget, config: ProbeConfig) -> Self {
        Self { target, config }
    }

    fn model_config(&self, api_key: &str) -> ModelClientConfig {
        ModelClientConfig::new(
            self.target.base_url.clone(),
            self.target.model.clone(),
            Some(api_key.to_string()),
            self.config.timeout,
        )
    }

    fn direct_complete(
        &self,
        api_key: &str,
        mode: ResponseFormatMode,
        request: &ModelRequest,
        budget: &CallBudget,
    ) -> Result<ModelResponse, SampleOutcome> {
        if !budget.consume() {
            return Err(SampleOutcome::budget(budget.rejection_reason()));
        }
        let remaining = budget.remaining();
        if remaining.is_zero() {
            return Err(SampleOutcome::budget("wall_clock_limit_reached"));
        }
        let mut config = self.model_config(api_key);
        config.timeout = config.timeout.min(remaining);
        let result = OpenAICompatibleModelClient::new(config)
            .with_response_format_mode(mode)
            .complete(request);
        if let Err(error) = &result {
            budget.record_model_error(error);
        }
        result.map_err(|error| sample_from_model_error(&error))
    }

    fn run_transport_gate(&self, api_key: &str, budget: &CallBudget) -> ProbeGateResult {
        let expected = self.config.profile.samples();
        let mut outcomes = Vec::new();
        let request = probe_request(
            "Transport probe. Return a short non-empty acknowledgement. JSON is not required.",
        );
        for _ in 0..expected {
            let outcome = match self.direct_complete(
                api_key,
                ResponseFormatMode::PromptOnly,
                &request,
                budget,
            ) {
                Ok(response) if response.raw_text.trim().is_empty() => SampleOutcome {
                    verdict: ProbeVerdict::Fail,
                    layer: ProbeLayer::Model,
                    reason_code: "empty_message_content",
                },
                Ok(response) if response.raw_text.len() > self.config.max_response_bytes => {
                    SampleOutcome {
                        verdict: ProbeVerdict::Inconclusive,
                        layer: ProbeLayer::Budget,
                        reason_code: "response_size_limit",
                    }
                }
                Ok(_) => SampleOutcome::pass(),
                Err(outcome) => outcome,
            };
            outcomes.push(outcome);
            if budget.pause().is_some() {
                break;
            }
        }
        aggregate_samples(ProbeGate::TransportPromptOnly, expected, outcomes)
    }

    fn run_json_gate(&self, api_key: &str, budget: &CallBudget) -> ProbeGateResult {
        let expected = self.config.profile.samples();
        let mut outcomes = Vec::new();
        let request = probe_request(
            r#"Return exactly one JSON object: {"action":"finish","summary":"json mode verified"}"#,
        );
        for _ in 0..expected {
            let outcome = match self.direct_complete(
                api_key,
                ResponseFormatMode::JsonObject,
                &request,
                budget,
            ) {
                Ok(response) => match parse_strict_model_decision(
                    &response.raw_text,
                    self.config.max_response_bytes,
                ) {
                    Ok(_) => SampleOutcome::pass(),
                    Err(StrictJsonError::ResponseTooLarge) => SampleOutcome {
                        verdict: ProbeVerdict::Inconclusive,
                        layer: ProbeLayer::Budget,
                        reason_code: "response_size_limit",
                    },
                    Err(StrictJsonError::InvalidJson | StrictJsonError::NotObject) => {
                        SampleOutcome {
                            verdict: ProbeVerdict::Fail,
                            layer: ProbeLayer::Model,
                            reason_code: "strict_json_not_satisfied",
                        }
                    }
                    Err(StrictJsonError::InvalidDecision) => SampleOutcome {
                        verdict: ProbeVerdict::Fail,
                        layer: ProbeLayer::Model,
                        reason_code: "action_grammar_invalid",
                    },
                },
                Err(outcome) => outcome,
            };
            outcomes.push(outcome);
            if budget.pause().is_some() {
                break;
            }
        }
        aggregate_samples(ProbeGate::JsonObjectStrict, expected, outcomes)
    }

    fn run_action_gate(
        &self,
        api_key: &str,
        budget: &CallBudget,
        expected_action: ExpectedAction,
    ) -> ProbeGateResult {
        let expected = self.config.profile.samples();
        let mut outcomes = Vec::new();
        let request = probe_request(expected_action.instruction());
        for _ in 0..expected {
            let outcome = match self.direct_complete(
                api_key,
                ResponseFormatMode::JsonObject,
                &request,
                budget,
            ) {
                Ok(response) => match parse_strict_model_decision(
                    &response.raw_text,
                    self.config.max_response_bytes,
                ) {
                    Ok(decision) if expected_action.matches(&decision) => SampleOutcome::pass(),
                    Ok(_) => SampleOutcome {
                        verdict: ProbeVerdict::Fail,
                        layer: ProbeLayer::Model,
                        reason_code: "unexpected_action",
                    },
                    Err(StrictJsonError::ResponseTooLarge) => SampleOutcome {
                        verdict: ProbeVerdict::Inconclusive,
                        layer: ProbeLayer::Budget,
                        reason_code: "response_size_limit",
                    },
                    Err(_) => SampleOutcome {
                        verdict: ProbeVerdict::Fail,
                        layer: ProbeLayer::Model,
                        reason_code: "invalid_action_response",
                    },
                },
                Err(outcome) => outcome,
            };
            outcomes.push(outcome);
            if budget.pause().is_some() {
                break;
            }
        }
        aggregate_samples(expected_action.gate(), expected, outcomes)
    }

    fn run_repair_sample(&self, api_key: &str, budget: &CallBudget) -> RepairSample {
        let live = LiveSessionConfig::autonomous_compile_repair_artifact();
        let Some(initial_artifact) = live.working_artifact.clone() else {
            return RepairSample::harness_failure("fixture_missing_artifact");
        };
        let initial_primary = initial_artifact.source().to_string();
        let client: Box<dyn ModelClient> = Box::new(BudgetedModelClient::new(
            self.model_config(api_key),
            ResponseFormatMode::JsonObject,
            budget.clone(),
        ));
        let session = AiSessionConfig::new(live.user_request.clone(), live.plan_kind.clone())
            .with_gap_guidance(live.gap_guidance);
        let mut context = AgentContext::new(&live.goal).with_working_artifact(initial_artifact);
        if let Some(specification) = live.evaluation_specification.clone() {
            context = context.with_evaluation_specification(specification);
        }
        let mut agent = AiAgent::new(client, session);
        let harness = build_probe_repair_harness();
        // El probe no espera ni insiste ante un rate limit real. La recuperación
        // se certifica exclusivamente con el gate sintético.
        let adaptive =
            AdaptiveRecoveryBudget::new(RecoveryBudget::new(0, Duration::ZERO), 0, Duration::ZERO);
        let loop_result =
            AgentLoop::new(live.max_iterations.min(self.config.max_iterations).max(1))
                .with_max_stale_iterations(3)
                .with_adaptive_recovery_budget(adaptive)
                .run(&harness, &mut agent, context);

        if budget.rejected() {
            return RepairSample::budget(budget.rejection_reason());
        }
        if loop_result.status != LoopStatus::Completed {
            return RepairSample::from_terminal(&loop_result);
        }

        let tools = loop_result.tools_executed();
        let first_compile = tools.iter().position(|tool| tool == COMPILE);
        let last_compile = tools.iter().rposition(|tool| tool == COMPILE);
        let repair = tools.iter().position(|tool| tool == REPAIR_DIAGNOSTIC);
        let correction = tools.iter().position(|tool| tool == APPLY_CORRECTION);
        let causal = matches!(
            (first_compile, repair, correction, last_compile),
            (Some(c1), Some(r), Some(a), Some(c2)) if c1 < r && r < a && a < c2
        );
        let decisions_valid = !agent.trace.parsed_decisions.is_empty()
            && agent.trace.parsed_decisions.iter().all(Result::is_ok);
        let no_rejections = loop_result.history.rejected_actions.is_empty();
        let helper_path = ArtifactPath::parse("src/helper.rs").expect("static helper path");
        let final_artifact = loop_result.final_context.working_artifact.as_ref();
        let helper_fixed = final_artifact
            .and_then(|artifact| artifact.file(&helper_path))
            .is_some_and(|source| !source.contains("broken"));
        let primary_unchanged =
            final_artifact.is_some_and(|artifact| artifact.source() == initial_primary);

        RepairSample {
            base: SampleOutcome::pass(),
            repair_pass: causal && decisions_valid && no_rejections && helper_fixed,
            multi_file_pass: helper_fixed && primary_unchanged,
            convergence_pass: loop_result.iterations <= self.config.max_iterations,
            iterations: loop_result.iterations,
        }
    }

    pub(crate) fn execute_unit(
        &self,
        api_key: &str,
        unit: ProbeExecutionUnit,
        max_calls: u32,
        max_elapsed: Duration,
    ) -> ProbeUnitExecution {
        let started = Instant::now();
        let budget = CallBudget::new(max_calls, max_elapsed);
        let gates = match unit {
            ProbeExecutionUnit::RetryAfterSynthetic => vec![retry_after_synthetic_gate()],
            ProbeExecutionUnit::Transport => vec![self.run_transport_gate(api_key, &budget)],
            ProbeExecutionUnit::JsonObjectStrict => vec![self.run_json_gate(api_key, &budget)],
            ProbeExecutionUnit::RepairBundle => {
                let samples = self.config.profile.samples();
                let mut repair = Vec::new();
                let mut multi_file = Vec::new();
                let mut convergence = Vec::new();
                let mut max_iterations_observed = 0_u32;
                for _ in 0..samples {
                    let sample = self.run_repair_sample(api_key, &budget);
                    max_iterations_observed = max_iterations_observed.max(sample.iterations);
                    repair.push(sample.for_repair());
                    multi_file.push(sample.for_multi_file());
                    convergence.push(sample.for_convergence());
                    if budget.pause().is_some() {
                        break;
                    }
                }
                vec![
                    aggregate_samples(ProbeGate::AutonomousRepair, samples, repair),
                    aggregate_samples(ProbeGate::MultiFile, samples, multi_file),
                    aggregate_samples(ProbeGate::BoundedConvergence, samples, convergence)
                        .with_metric("max_iterations_observed", max_iterations_observed as u64),
                ]
            }
            ProbeExecutionUnit::NativeToolCalling => vec![ProbeGateResult::single(
                ProbeGate::NativeToolCalling,
                ProbeVerdict::NotTested,
                ProbeLayer::Adapter,
                "adapter_does_not_expose_tool_calls",
            )],
            action_unit => {
                let expected = action_unit
                    .expected_action()
                    .expect("only action units reach this branch");
                vec![self.run_action_gate(api_key, &budget, expected)]
            }
        };
        ProbeUnitExecution {
            gates,
            calls_used: budget.used(),
            active_elapsed: started.elapsed(),
            pause: budget.pause(),
        }
    }

    pub fn run_live(&self, permit: &LiveProbePermit, api_key: &str) -> ModelCompatibilityReport {
        let effective_limit = self.config.max_calls.min(permit.max_calls());
        let budget = CallBudget::new(effective_limit, self.config.max_elapsed);
        let mut gates = vec![retry_after_synthetic_gate()];

        let transport = self.run_transport_gate(api_key, &budget);
        let transport_passed = transport.verdict == ProbeVerdict::Pass;
        let transport_layer = transport.layer;
        gates.push(transport);

        if !transport_passed {
            gates.push(prerequisite_not_tested(
                ProbeGate::JsonObjectStrict,
                transport_layer,
            ));
            for expected in EXPECTED_ACTIONS {
                gates.push(prerequisite_not_tested(expected.gate(), transport_layer));
            }
            gates.push(prerequisite_not_tested(
                ProbeGate::AutonomousRepair,
                transport_layer,
            ));
            gates.push(prerequisite_not_tested(
                ProbeGate::MultiFile,
                transport_layer,
            ));
            gates.push(prerequisite_not_tested(
                ProbeGate::BoundedConvergence,
                transport_layer,
            ));
        } else {
            let json = self.run_json_gate(api_key, &budget);
            let json_passed = json.verdict == ProbeVerdict::Pass;
            let json_layer = json.layer;
            gates.push(json);
            if json_passed {
                let mut external_circuit_open = false;
                for expected in EXPECTED_ACTIONS {
                    if external_circuit_open {
                        gates.push(prerequisite_not_tested(
                            expected.gate(),
                            ProbeLayer::External,
                        ));
                        continue;
                    }
                    let action_gate = self.run_action_gate(api_key, &budget, expected);
                    external_circuit_open = opens_external_circuit(&action_gate);
                    gates.push(action_gate);
                }

                if external_circuit_open {
                    gates.push(prerequisite_not_tested(
                        ProbeGate::AutonomousRepair,
                        ProbeLayer::External,
                    ));
                    gates.push(prerequisite_not_tested(
                        ProbeGate::MultiFile,
                        ProbeLayer::External,
                    ));
                    gates.push(prerequisite_not_tested(
                        ProbeGate::BoundedConvergence,
                        ProbeLayer::External,
                    ));
                } else {
                    let samples = self.config.profile.samples();
                    let mut repair = Vec::new();
                    let mut multi_file = Vec::new();
                    let mut convergence = Vec::new();
                    let mut max_iterations_observed = 0_u32;
                    for _ in 0..samples {
                        let sample = self.run_repair_sample(api_key, &budget);
                        max_iterations_observed = max_iterations_observed.max(sample.iterations);
                        repair.push(sample.for_repair());
                        multi_file.push(sample.for_multi_file());
                        convergence.push(sample.for_convergence());
                        if budget.pause().is_some() {
                            break;
                        }
                    }
                    gates.push(aggregate_samples(
                        ProbeGate::AutonomousRepair,
                        samples,
                        repair,
                    ));
                    gates.push(aggregate_samples(ProbeGate::MultiFile, samples, multi_file));
                    gates.push(
                        aggregate_samples(ProbeGate::BoundedConvergence, samples, convergence)
                            .with_metric("max_iterations_observed", max_iterations_observed as u64),
                    );
                }
            } else {
                for expected in EXPECTED_ACTIONS {
                    gates.push(prerequisite_not_tested(expected.gate(), json_layer));
                }
                gates.push(prerequisite_not_tested(
                    ProbeGate::AutonomousRepair,
                    json_layer,
                ));
                gates.push(prerequisite_not_tested(ProbeGate::MultiFile, json_layer));
                gates.push(prerequisite_not_tested(
                    ProbeGate::BoundedConvergence,
                    json_layer,
                ));
            }
        }

        gates.push(ProbeGateResult::single(
            ProbeGate::NativeToolCalling,
            ProbeVerdict::NotTested,
            ProbeLayer::Adapter,
            "adapter_does_not_expose_tool_calls",
        ));

        ModelCompatibilityReport {
            suite_version: MODEL_COMPATIBILITY_SUITE_VERSION.to_string(),
            provider: self.target.provider.clone(),
            model: self.target.model.clone(),
            profile: self.config.profile,
            calls_used: budget.used(),
            calls_limit: budget.max(),
            gates,
        }
    }
}

#[derive(Debug, Clone)]
struct RepairSample {
    base: SampleOutcome,
    repair_pass: bool,
    multi_file_pass: bool,
    convergence_pass: bool,
    iterations: u32,
}

impl RepairSample {
    fn harness_failure(reason_code: &'static str) -> Self {
        Self {
            base: SampleOutcome {
                verdict: ProbeVerdict::Fail,
                layer: ProbeLayer::Harness,
                reason_code,
            },
            repair_pass: false,
            multi_file_pass: false,
            convergence_pass: false,
            iterations: 0,
        }
    }

    fn budget(reason_code: &'static str) -> Self {
        Self {
            base: SampleOutcome::budget(reason_code),
            repair_pass: false,
            multi_file_pass: false,
            convergence_pass: false,
            iterations: 0,
        }
    }

    fn from_terminal(result: &LoopResult) -> Self {
        let base = if let Some(report) = result.failure_report.as_ref() {
            match report.classification {
                FailureClass::ExternalTransient => SampleOutcome {
                    verdict: ProbeVerdict::Inconclusive,
                    layer: ProbeLayer::External,
                    reason_code: "external_transient",
                },
                FailureClass::ExternalPermanent => SampleOutcome {
                    verdict: ProbeVerdict::Blocked,
                    layer: ProbeLayer::External,
                    reason_code: "external_permanent",
                },
                FailureClass::ModelCapability | FailureClass::ConvergenceStalled => SampleOutcome {
                    verdict: ProbeVerdict::Fail,
                    layer: ProbeLayer::Model,
                    reason_code: "model_did_not_converge",
                },
                FailureClass::SystemFailure => SampleOutcome {
                    verdict: ProbeVerdict::Fail,
                    layer: ProbeLayer::Harness,
                    reason_code: "harness_failure",
                },
            }
        } else {
            match result.status {
                LoopStatus::MaxIterations | LoopStatus::NonProgress => SampleOutcome {
                    verdict: ProbeVerdict::Fail,
                    layer: ProbeLayer::Model,
                    reason_code: "model_did_not_converge",
                },
                LoopStatus::ExternalServiceBlocked => SampleOutcome {
                    verdict: ProbeVerdict::Inconclusive,
                    layer: ProbeLayer::External,
                    reason_code: "external_transient",
                },
                LoopStatus::ExternalConfigurationBlocked => SampleOutcome {
                    verdict: ProbeVerdict::Blocked,
                    layer: ProbeLayer::External,
                    reason_code: "external_permanent",
                },
                LoopStatus::SystemFailure => SampleOutcome {
                    verdict: ProbeVerdict::Fail,
                    layer: ProbeLayer::Harness,
                    reason_code: "harness_failure",
                },
                _ => SampleOutcome {
                    verdict: ProbeVerdict::Fail,
                    layer: ProbeLayer::Model,
                    reason_code: "repair_not_completed",
                },
            }
        };
        Self {
            base,
            repair_pass: false,
            multi_file_pass: false,
            convergence_pass: false,
            iterations: result.iterations,
        }
    }

    fn outcome_for(&self, passed: bool, failed_reason: &'static str) -> SampleOutcome {
        if self.base.verdict != ProbeVerdict::Pass {
            return self.base.clone();
        }
        if passed {
            SampleOutcome::pass()
        } else {
            SampleOutcome {
                verdict: ProbeVerdict::Fail,
                layer: ProbeLayer::Model,
                reason_code: failed_reason,
            }
        }
    }

    fn for_repair(&self) -> SampleOutcome {
        self.outcome_for(self.repair_pass, "repair_contract_not_satisfied")
    }

    fn for_multi_file(&self) -> SampleOutcome {
        self.outcome_for(self.multi_file_pass, "multi_file_contract_not_satisfied")
    }

    fn for_convergence(&self) -> SampleOutcome {
        self.outcome_for(self.convergence_pass, "convergence_contract_not_satisfied")
    }
}

pub fn retry_after_synthetic_gate() -> ProbeGateResult {
    let allowed_error = ModelError::rate_limited_with_retry_after(
        "synthetic_rate_limit",
        Some(Duration::from_secs(7)),
    );
    let allowed_evidence = classify_model_error(&allowed_error);
    let recovery_budget = RecoveryBudget::new(2, Duration::ZERO);
    let allowed_recovery = plan_recovery(&allowed_evidence, &recovery_budget);
    let mut adaptive_budget =
        AdaptiveRecoveryBudget::new(recovery_budget, 0, Duration::from_secs(10));
    let allowed = plan_adaptive_recovery(allowed_recovery.clone(), None, &adaptive_budget);
    let consumed = adaptive_budget.consume_recovery(allowed_recovery.wait);

    let denied_error = ModelError::rate_limited_with_retry_after(
        "synthetic_rate_limit",
        Some(Duration::from_secs(11)),
    );
    let denied_evidence = classify_model_error(&denied_error);
    let denied_recovery = plan_recovery(&denied_evidence, &RecoveryBudget::new(2, Duration::ZERO));
    let denied_budget = AdaptiveRecoveryBudget::new(
        RecoveryBudget::new(2, Duration::ZERO),
        0,
        Duration::from_secs(5),
    );
    let denied = plan_adaptive_recovery(denied_recovery, None, &denied_budget);

    let passed = allowed_evidence.http_status == Some(429)
        && allowed_recovery.strategy == RecoveryStrategy::WaitThenRetry
        && allowed_recovery.reason == RecoveryPlanReason::ProviderRetryAfter
        && allowed_recovery.wait == Duration::from_secs(7)
        && allowed.action == AdaptiveRecoveryAction::RetrySameModel
        && allowed.reason == AdaptiveRecoveryReason::RecoveryAuthorized
        && consumed
        && adaptive_budget.cumulative_wait == Duration::from_secs(7)
        && denied.action == AdaptiveRecoveryAction::Stop
        && denied.reason == AdaptiveRecoveryReason::CumulativeWaitExhausted;

    ProbeGateResult::single(
        ProbeGate::RetryAfterSynthetic,
        if passed {
            ProbeVerdict::Pass
        } else {
            ProbeVerdict::Fail
        },
        ProbeLayer::Harness,
        if passed {
            "adaptive_retry_after_verified"
        } else {
            "adaptive_retry_after_invariant_failed"
        },
    )
    .with_metric("authorized_wait_seconds", 7)
    .with_metric("denied_wait_seconds", 11)
    .with_metric("denied_budget_seconds", 5)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeCliOptions {
    pub models: Vec<String>,
    pub base_url: String,
    pub profile: ProbeProfile,
    pub timeout: Duration,
    pub max_calls: u32,
    pub checkpoint_dir: PathBuf,
    pub pacing: Duration,
    pub max_recovery_attempts: u32,
    pub max_single_wait: Duration,
    pub max_cumulative_wait: Duration,
    pub fallback_wait: Duration,
    pub json: bool,
    pub ack_live: bool,
    pub dry_run: bool,
}

impl ProbeCliOptions {
    pub fn dry_run_output(&self) -> String {
        if self.json {
            serde_json::to_string_pretty(&json!({
                "mode": "dry_run",
                "provider": "nvidia",
                "models": self.models,
                "base_url": self.base_url,
                "profile": self.profile.as_str(),
                "timeout_ms": self.timeout.as_millis() as u64,
                "max_calls_per_model": self.max_calls,
                "max_elapsed_ms_per_model": self.profile.default_max_elapsed().as_millis() as u64,
                "checkpoint_dir": self.checkpoint_dir,
                "pacing_ms": self.pacing.as_millis() as u64,
                "max_recovery_attempts": self.max_recovery_attempts,
                "max_single_wait_ms": self.max_single_wait.as_millis() as u64,
                "max_cumulative_wait_ms": self.max_cumulative_wait.as_millis() as u64,
                "fallback_wait_ms": self.fallback_wait.as_millis() as u64,
                "external_calls": 0,
            }))
            .expect("dry-run JSON is serializable")
        } else {
            format!(
                "MODEL COMPATIBILITY PROBE DRY RUN\nprovider=nvidia\nmodels={}\nbase_url={}\nprofile={}\ntimeout_ms={}\nmax_calls_per_model={}\nmax_elapsed_ms_per_model={}\nexternal_calls=0",
                self.models.join(","),
                self.base_url,
                self.profile.as_str(),
                self.timeout.as_millis(),
                self.max_calls,
                self.profile.default_max_elapsed().as_millis(),
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeCliError {
    MissingValue(&'static str),
    InvalidValue {
        flag: &'static str,
        expected: &'static str,
    },
    UnknownArgument(String),
    LiveAcknowledgementRequired,
    NvidiaApiKeyMissing,
    Setup(ProbeSetupError),
    Scheduler(String),
}

impl fmt::Display for ProbeCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue(flag) => write!(f, "falta valor para {flag}"),
            Self::InvalidValue { flag, expected } => {
                write!(f, "valor inválido para {flag}; esperado {expected}")
            }
            Self::UnknownArgument(flag) => write!(f, "argumento desconocido: {flag}"),
            Self::LiveAcknowledgementRequired => {
                write!(f, "se requiere --ack-live para llamadas externas")
            }
            Self::NvidiaApiKeyMissing => {
                write!(f, "variable NVIDIA_API_KEY no definida o vacía")
            }
            Self::Setup(error) => write!(f, "{error}"),
            Self::Scheduler(error) => write!(f, "scheduler: {error}"),
        }
    }
}

impl From<ProbeSetupError> for ProbeCliError {
    fn from(error: ProbeSetupError) -> Self {
        Self::Setup(error)
    }
}

pub fn probe_cli_usage() -> &'static str {
    "model-compatibility-probe [--model ID] [--base-url URL] [--profile smoke|certify] [--timeout-ms N] [--max-calls N] [--checkpoint-dir PATH] [--pacing-ms N] [--max-recoveries N] [--max-retry-after-ms N] [--max-cumulative-wait-ms N] [--fallback-wait-ms N] [--json] [--dry-run] --ack-live"
}

pub fn parse_probe_cli_args<I, S>(args: I) -> Result<ProbeCliOptions, ProbeCliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut index = 0;
    let mut models = Vec::new();
    let mut base_url = NVIDIA_DEFAULT_BASE_URL.to_string();
    let mut profile = ProbeProfile::Smoke;
    let mut timeout_ms = 60_000_u64;
    let mut max_calls = None;
    let mut checkpoint_dir = PathBuf::from(".model-probe-state");
    let mut pacing_ms = 2_000_u64;
    let mut max_recovery_attempts = 3_u32;
    let mut max_single_wait_ms = 120_000_u64;
    let mut max_cumulative_wait_ms = 300_000_u64;
    let mut fallback_wait_ms = 5_000_u64;
    let mut json_output = false;
    let mut ack_live = false;
    let mut dry_run = false;

    while index < args.len() {
        match args[index].as_str() {
            "--model" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--model"))?;
                if value.trim().is_empty() {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--model",
                        expected: "non-empty model id",
                    });
                }
                models.push(value.clone());
            }
            "--base-url" => {
                index += 1;
                base_url = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--base-url"))?
                    .clone();
            }
            "--profile" => {
                index += 1;
                profile = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--profile"))?
                    .parse()?;
            }
            "--timeout-ms" => {
                index += 1;
                timeout_ms = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--timeout-ms"))?
                    .parse()
                    .map_err(|_| ProbeCliError::InvalidValue {
                        flag: "--timeout-ms",
                        expected: "positive integer",
                    })?;
                if timeout_ms == 0 {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--timeout-ms",
                        expected: "positive integer",
                    });
                }
            }
            "--max-calls" => {
                index += 1;
                let parsed = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--max-calls"))?
                    .parse::<u32>()
                    .map_err(|_| ProbeCliError::InvalidValue {
                        flag: "--max-calls",
                        expected: "positive integer",
                    })?;
                if parsed == 0 {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--max-calls",
                        expected: "positive integer",
                    });
                }
                max_calls = Some(parsed);
            }
            "--checkpoint-dir" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--checkpoint-dir"))?;
                if value.trim().is_empty() {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--checkpoint-dir",
                        expected: "non-empty path",
                    });
                }
                checkpoint_dir = PathBuf::from(value);
            }
            "--pacing-ms" => {
                index += 1;
                pacing_ms = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--pacing-ms"))?
                    .parse()
                    .map_err(|_| ProbeCliError::InvalidValue {
                        flag: "--pacing-ms",
                        expected: "non-negative integer",
                    })?;
            }
            "--max-recoveries" => {
                index += 1;
                max_recovery_attempts = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--max-recoveries"))?
                    .parse()
                    .map_err(|_| ProbeCliError::InvalidValue {
                        flag: "--max-recoveries",
                        expected: "positive integer",
                    })?;
                if max_recovery_attempts == 0 {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--max-recoveries",
                        expected: "positive integer",
                    });
                }
            }
            "--max-retry-after-ms" => {
                index += 1;
                max_single_wait_ms = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--max-retry-after-ms"))?
                    .parse()
                    .map_err(|_| ProbeCliError::InvalidValue {
                        flag: "--max-retry-after-ms",
                        expected: "positive integer",
                    })?;
                if max_single_wait_ms == 0 {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--max-retry-after-ms",
                        expected: "positive integer",
                    });
                }
            }
            "--max-cumulative-wait-ms" => {
                index += 1;
                max_cumulative_wait_ms = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--max-cumulative-wait-ms"))?
                    .parse()
                    .map_err(|_| ProbeCliError::InvalidValue {
                        flag: "--max-cumulative-wait-ms",
                        expected: "positive integer",
                    })?;
                if max_cumulative_wait_ms == 0 {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--max-cumulative-wait-ms",
                        expected: "positive integer",
                    });
                }
            }
            "--fallback-wait-ms" => {
                index += 1;
                fallback_wait_ms = args
                    .get(index)
                    .ok_or(ProbeCliError::MissingValue("--fallback-wait-ms"))?
                    .parse()
                    .map_err(|_| ProbeCliError::InvalidValue {
                        flag: "--fallback-wait-ms",
                        expected: "positive integer",
                    })?;
                if fallback_wait_ms == 0 {
                    return Err(ProbeCliError::InvalidValue {
                        flag: "--fallback-wait-ms",
                        expected: "positive integer",
                    });
                }
            }
            "--json" => json_output = true,
            "--ack-live" => ack_live = true,
            "--dry-run" => dry_run = true,
            other => return Err(ProbeCliError::UnknownArgument(other.to_string())),
        }
        index += 1;
    }

    if models.is_empty() {
        models = NVIDIA_DEFAULT_MODELS
            .iter()
            .map(|model| model.to_string())
            .collect();
    }
    ProbeTarget::nvidia(models[0].clone(), base_url.clone())?;

    Ok(ProbeCliOptions {
        models,
        base_url,
        profile,
        timeout: Duration::from_millis(timeout_ms),
        max_calls: max_calls.unwrap_or_else(|| profile.default_max_calls()),
        checkpoint_dir,
        pacing: Duration::from_millis(pacing_ms),
        max_recovery_attempts,
        max_single_wait: Duration::from_millis(max_single_wait_ms),
        max_cumulative_wait: Duration::from_millis(max_cumulative_wait_ms),
        fallback_wait: Duration::from_millis(fallback_wait_ms),
        json: json_output,
        ack_live,
        dry_run,
    })
}

pub fn run_model_compatibility_probe_cli<I, S>(args: I) -> Result<String, ProbeCliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let options = parse_probe_cli_args(args)?;
    if options.dry_run {
        return Ok(options.dry_run_output());
    }
    if !options.ack_live {
        return Err(ProbeCliError::LiveAcknowledgementRequired);
    }
    let api_key = std::env::var(NVIDIA_API_KEY_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ProbeCliError::NvidiaApiKeyMissing)?;
    let permit = LiveProbePermit::acknowledge_external_calls_and_costs(options.max_calls)?;
    let config = ProbeConfig::new(options.profile, options.timeout, options.max_calls)?;
    let mut scheduler_config = ProbeSchedulerConfig::in_directory(&options.checkpoint_dir);
    scheduler_config.pacing = options.pacing;
    scheduler_config.max_recovery_attempts = options.max_recovery_attempts;
    scheduler_config.max_single_wait = options.max_single_wait;
    scheduler_config.max_cumulative_wait = options.max_cumulative_wait;
    scheduler_config.fallback_wait = options.fallback_wait;
    let scheduler = ProbeScheduler::new(scheduler_config)
        .map_err(|error| ProbeCliError::Scheduler(error.to_string()))?;
    let mut reports = Vec::new();
    for model in &options.models {
        let target = ProbeTarget::nvidia(model.clone(), options.base_url.clone())?;
        let probe = ModelCompatibilityProbe::new(target, config.clone());
        reports.push(
            scheduler
                .run_live(&probe, &permit, &api_key)
                .map_err(|error| ProbeCliError::Scheduler(error.to_string()))?,
        );
    }

    if options.json {
        let values: Vec<Value> = reports
            .iter()
            .map(ProbeScheduleOutcome::to_json_value)
            .collect();
        Ok(serde_json::to_string_pretty(&values)
            .expect("probe reports use only serializable JSON values"))
    } else {
        Ok(reports
            .iter()
            .map(ProbeScheduleOutcome::to_text)
            .collect::<Vec<_>>()
            .join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_json_requires_one_object_and_valid_action() {
        let valid = r#"{"action":"finish","summary":"ok"}"#;
        assert!(matches!(
            parse_strict_model_decision(valid, 1024),
            Ok(ModelDecision::Finish { .. })
        ));
        assert_eq!(
            parse_strict_model_decision(
                "~~~json\n{\"action\":\"finish\",\"summary\":\"ok\"}\n~~~",
                1024,
            ),
            Err(StrictJsonError::InvalidJson)
        );
        assert_eq!(
            parse_strict_model_decision(
                "{\"action\":\"finish\",\"summary\":\"ok\"} trailing",
                1024,
            ),
            Err(StrictJsonError::InvalidJson)
        );
        assert_eq!(
            parse_strict_model_decision("[]", 1024),
            Err(StrictJsonError::NotObject)
        );
        assert_eq!(
            parse_strict_model_decision(r#"{"message":"ok"}"#, 1024),
            Err(StrictJsonError::InvalidDecision)
        );
    }

    #[test]
    fn strict_json_rejects_wrapped_or_nested_required_fields() {
        for invalid in [
            r#"{"result":{"action":"finish","summary":"ok"}}"#,
            r#"{"action":"finish","payload":{"summary":"ok"}}"#,
            r#"{"action":"validate","request":"probe","plan_kind":"Generic","payload":{"code":null}}"#,
            r#"{"action":"run_tests","payload":{"filter":"probe"}}"#,
        ] {
            assert_eq!(
                parse_strict_model_decision(invalid, 1024),
                Err(StrictJsonError::InvalidDecision),
                "invalid wrapper passed strict parsing: {invalid}"
            );
        }
    }

    #[test]
    fn strict_json_accepts_every_canonical_action_shape() {
        for expected in EXPECTED_ACTIONS {
            let raw = expected
                .instruction()
                .split_once(": ")
                .expect("probe instruction")
                .1;
            let decision = parse_strict_model_decision(raw, 4096)
                .unwrap_or_else(|error| panic!("canonical action failed: {raw}: {error:?}"));
            assert!(expected.matches(&decision), "unexpected decision for {raw}");
        }
    }

    #[test]
    fn cli_dry_run_uses_nvidia_defaults_without_live_ack_or_secrets() {
        let output = run_model_compatibility_probe_cli([
            "--dry-run",
            "--profile",
            "smoke",
            "--timeout-ms",
            "1000",
            "--checkpoint-dir",
            "state-test",
            "--pacing-ms",
            "0",
            "--max-recoveries",
            "2",
            "--max-retry-after-ms",
            "9000",
            "--max-cumulative-wait-ms",
            "12000",
            "--fallback-wait-ms",
            "1000",
            "--json",
        ])
        .expect("dry-run");
        assert!(output.contains("moonshotai/kimi-k3"));
        assert!(output.contains("\"external_calls\": 0"));
        assert!(output.contains("\"checkpoint_dir\": \"state-test\""));
        assert!(output.contains("\"pacing_ms\": 0"));
        assert!(output.contains("\"max_recovery_attempts\": 2"));
        assert!(output.contains("\"max_single_wait_ms\": 9000"));
        assert!(output.contains("\"max_cumulative_wait_ms\": 12000"));
        assert!(output.contains("\"fallback_wait_ms\": 1000"));
        assert!(!output.contains("Bearer "));
        assert!(!output.contains("nvapi-"));
        assert_eq!(
            ProbeTarget::nvidia(
                "model",
                "https://integrate.api.nvidia.com/v1?api_key=secret",
            ),
            Err(ProbeSetupError::UnsafeBaseUrl)
        );
    }

    #[test]
    fn probe_target_requires_https_before_bearer_authentication() {
        let target = ProbeTarget::nvidia("model", "https://integrate.api.nvidia.com/v1")
            .expect("HTTPS target");
        assert_eq!(target.base_url, "https://integrate.api.nvidia.com/v1");
        assert_eq!(
            ProbeTarget::nvidia("model", "http://integrate.api.nvidia.com/v1"),
            Err(ProbeSetupError::UnsafeBaseUrl)
        );
        assert_eq!(
            ProbeTarget::nvidia("model", "ftp://integrate.api.nvidia.com/v1"),
            Err(ProbeSetupError::InvalidTarget)
        );
    }

    #[test]
    fn cli_requires_explicit_ack_before_reading_api_key() {
        let error =
            run_model_compatibility_probe_cli(["--model", "nvidia/test-model", "--max-calls", "1"])
                .expect_err("ack required");
        assert_eq!(error, ProbeCliError::LiveAcknowledgementRequired);
    }

    #[test]
    fn retry_after_gate_uses_adaptive_coordinator_without_external_call() {
        let result = retry_after_synthetic_gate();
        assert_eq!(result.verdict, ProbeVerdict::Pass);
        assert_eq!(result.layer, ProbeLayer::Harness);
        assert_eq!(result.reason_code, "adaptive_retry_after_verified");
        assert_eq!(
            result
                .metrics
                .iter()
                .find(|metric| metric.name == "authorized_wait_seconds")
                .map(|metric| metric.value),
            Some(7)
        );
    }

    #[test]
    fn external_transient_result_opens_the_live_circuit() {
        let rate_limited = ProbeGateResult::single(
            ProbeGate::ActionValidate,
            ProbeVerdict::Inconclusive,
            ProbeLayer::External,
            "rate_limited",
        );
        let model_failure = ProbeGateResult::single(
            ProbeGate::ActionValidate,
            ProbeVerdict::Fail,
            ProbeLayer::Model,
            "unexpected_action",
        );

        assert!(opens_external_circuit(&rate_limited));
        assert!(!opens_external_circuit(&model_failure));
    }

    #[test]
    fn expired_wall_clock_budget_rejects_without_consuming_a_call() {
        let budget = CallBudget::new(3, Duration::ZERO);

        assert!(!budget.consume());
        assert_eq!(budget.used(), 0);
        assert_eq!(budget.rejection_reason(), "wall_clock_limit_reached");
        assert_eq!(budget.remaining(), Duration::ZERO);
    }

    #[test]
    fn open_external_circuit_rejects_before_consuming_another_call() {
        let budget = CallBudget::new(3, Duration::from_secs(1));
        budget.record_model_error(&ModelError::Timeout);
        let client = BudgetedModelClient::new(
            ModelClientConfig::new(
                "http://127.0.0.1:1/v1",
                "test-model",
                Some("test-key".to_string()),
                Duration::from_millis(10),
            ),
            ResponseFormatMode::JsonObject,
            budget.clone(),
        );

        let error = client
            .complete(&probe_request("return JSON"))
            .expect_err("open circuit must reject locally");

        assert!(matches!(error, ModelError::Configuration(_)));
        assert_eq!(budget.used(), 0);
        assert_eq!(
            budget.pause().map(|pause| pause.reason_code),
            Some("timeout")
        );
    }

    #[test]
    fn repair_harness_rejects_tools_that_can_spawn_processes() {
        let harness = build_probe_repair_harness();
        let mut context = AgentContext::new("probe safety");
        let outcome = harness.execute_step(
            crate::harness::action::AgentAction::RunTests {
                filter: "probe".to_string(),
            },
            &mut context,
        );

        assert!(!outcome.permitted);
        assert_eq!(
            outcome.rejected_constraint.as_deref(),
            Some("tool_permission")
        );
        assert!(outcome.tool_result.is_none());
    }

    #[test]
    fn report_summary_ignores_native_tools_not_tested() {
        let report = ModelCompatibilityReport {
            suite_version: "test".to_string(),
            provider: "nvidia".to_string(),
            model: "test-model".to_string(),
            profile: ProbeProfile::Smoke,
            calls_used: 2,
            calls_limit: 3,
            gates: vec![
                ProbeGateResult::single(
                    ProbeGate::TransportPromptOnly,
                    ProbeVerdict::Pass,
                    ProbeLayer::Adapter,
                    "ok",
                ),
                ProbeGateResult::single(
                    ProbeGate::NativeToolCalling,
                    ProbeVerdict::NotTested,
                    ProbeLayer::Adapter,
                    "adapter_does_not_expose_tool_calls",
                ),
            ],
        };
        assert_eq!(report.overall_verdict(), ProbeVerdict::Pass);
        assert!(report.to_text().contains("overall=pass"));
        assert!(report.to_json().contains("\"overall_verdict\": \"pass\""));
        assert!(!report.to_json().contains("raw_response"));
    }

    #[test]
    fn failed_required_gate_dominates_summary() {
        let report = ModelCompatibilityReport {
            suite_version: "test".to_string(),
            provider: "nvidia".to_string(),
            model: "test-model".to_string(),
            profile: ProbeProfile::Smoke,
            calls_used: 1,
            calls_limit: 1,
            gates: vec![
                ProbeGateResult::single(
                    ProbeGate::TransportPromptOnly,
                    ProbeVerdict::Pass,
                    ProbeLayer::Adapter,
                    "ok",
                ),
                ProbeGateResult::single(
                    ProbeGate::JsonObjectStrict,
                    ProbeVerdict::Fail,
                    ProbeLayer::Model,
                    "strict_json_not_satisfied",
                ),
            ],
        };
        assert_eq!(report.overall_verdict(), ProbeVerdict::Fail);
    }
}
