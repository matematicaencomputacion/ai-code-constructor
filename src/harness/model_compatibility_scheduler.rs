//! Scheduler durable para el Model Compatibility Probe.
//!
//! El checkpoint contiene solo identidad pública, contadores y resultados
//! normalizados. Nunca persiste API keys, prompts, respuestas ni cuerpos HTTP.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::harness::model_compatibility_probe::{
    LiveProbePermit, MODEL_COMPATIBILITY_SUITE_VERSION, ModelCompatibilityProbe,
    ModelCompatibilityReport, ProbeExecutionUnit, ProbeGate, ProbeGateResult, ProbeLayer,
    ProbePauseSignal, ProbeProfile, ProbeTarget, ProbeUnitExecution, ProbeVerdict,
};

const CHECKPOINT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeScheduleState {
    Ready,
    Waiting,
    Completed,
    Blocked,
}

impl ProbeScheduleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSchedulerConfig {
    pub checkpoint_dir: PathBuf,
    pub pacing: Duration,
    pub max_recovery_attempts: u32,
    pub max_single_wait: Duration,
    pub max_cumulative_wait: Duration,
    pub fallback_wait: Duration,
}

impl ProbeSchedulerConfig {
    pub fn in_directory(checkpoint_dir: impl Into<PathBuf>) -> Self {
        Self {
            checkpoint_dir: checkpoint_dir.into(),
            pacing: Duration::from_secs(2),
            max_recovery_attempts: 3,
            max_single_wait: Duration::from_secs(120),
            max_cumulative_wait: Duration::from_secs(300),
            fallback_wait: Duration::from_secs(5),
        }
    }

    fn validate(&self) -> Result<(), ProbeSchedulerError> {
        if self.max_recovery_attempts == 0
            || self.max_single_wait.is_zero()
            || self.max_cumulative_wait.is_zero()
            || self.fallback_wait.is_zero()
        {
            return Err(ProbeSchedulerError::InvalidPolicy);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedPolicy {
    calls_limit: u32,
    active_elapsed_limit_ms: u64,
    pacing_ms: u64,
    max_recovery_attempts: u32,
    max_single_wait_ms: u64,
    max_cumulative_wait_ms: u64,
    fallback_wait_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedPause {
    reason_code: String,
    retry_after_ms: u64,
    http_status: Option<u16>,
    pacing_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProbeCheckpoint {
    schema_version: u32,
    suite_version: String,
    target: ProbeTarget,
    profile: ProbeProfile,
    policy: PersistedPolicy,
    state: ProbeScheduleState,
    next_unit_index: usize,
    completed_gates: Vec<ProbeGateResult>,
    calls_used_total: u32,
    active_elapsed_ms_total: u64,
    recovery_attempts_used: u32,
    cumulative_wait_ms: u64,
    not_before_unix_ms: Option<u64>,
    last_pause: Option<PersistedPause>,
    updated_at_unix_ms: u64,
}

impl ProbeCheckpoint {
    fn new(
        target: ProbeTarget,
        profile: ProbeProfile,
        policy: PersistedPolicy,
        now_ms: u64,
    ) -> Self {
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            suite_version: MODEL_COMPATIBILITY_SUITE_VERSION.to_string(),
            target,
            profile,
            policy,
            state: ProbeScheduleState::Ready,
            next_unit_index: 0,
            completed_gates: Vec::new(),
            calls_used_total: 0,
            active_elapsed_ms_total: 0,
            recovery_attempts_used: 0,
            cumulative_wait_ms: 0,
            not_before_unix_ms: None,
            last_pause: None,
            updated_at_unix_ms: now_ms,
        }
    }

    fn report(&self) -> ModelCompatibilityReport {
        let mut gates = self.completed_gates.clone();
        append_pending_report_results(self, &mut gates);
        ModelCompatibilityReport {
            suite_version: self.suite_version.clone(),
            provider: self.target.provider.clone(),
            model: self.target.model.clone(),
            profile: self.profile,
            calls_used: self.calls_used_total,
            calls_limit: self.policy.calls_limit,
            gates,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeScheduleOutcome {
    pub state: ProbeScheduleState,
    pub next_unit: Option<String>,
    pub not_before_unix_ms: Option<u64>,
    pub recovery_attempts_used: u32,
    pub cumulative_wait_ms: u64,
    pub report: ModelCompatibilityReport,
    pub checkpoint_path: PathBuf,
}

impl ProbeScheduleOutcome {
    fn from_checkpoint(checkpoint: &ProbeCheckpoint, checkpoint_path: PathBuf) -> Self {
        Self {
            state: checkpoint.state,
            next_unit: ProbeExecutionUnit::ORDERED
                .get(checkpoint.next_unit_index)
                .map(|unit| unit.as_str().to_string()),
            not_before_unix_ms: checkpoint.not_before_unix_ms,
            recovery_attempts_used: checkpoint.recovery_attempts_used,
            cumulative_wait_ms: checkpoint.cumulative_wait_ms,
            report: checkpoint.report(),
            checkpoint_path,
        }
    }

    pub fn to_json_value(&self) -> Value {
        json!({
            "scheduler": {
                "state": self.state.as_str(),
                "next_unit": self.next_unit,
                "not_before_unix_ms": self.not_before_unix_ms,
                "recovery_attempts_used": self.recovery_attempts_used,
                "cumulative_wait_ms": self.cumulative_wait_ms,
                "checkpoint_path": self.checkpoint_path.to_string_lossy(),
            },
            "report": self.report.to_json_value(),
        })
    }

    pub fn to_text(&self) -> String {
        format!(
            "MODEL COMPATIBILITY SCHEDULER\nstate={}\nnext_unit={}\nnot_before_unix_ms={}\nrecovery_attempts={}\ncumulative_wait_ms={}\ncheckpoint={}\n\n{}",
            self.state.as_str(),
            self.next_unit.as_deref().unwrap_or("none"),
            self.not_before_unix_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.recovery_attempts_used,
            self.cumulative_wait_ms,
            self.checkpoint_path.display(),
            self.report.to_text(),
        )
    }
}

#[derive(Debug)]
pub enum ProbeSchedulerError {
    InvalidPolicy,
    CheckpointIo(std::io::Error),
    CheckpointJson(serde_json::Error),
    CheckpointMismatch(&'static str),
    CheckpointCorrupt(&'static str),
    ExecutorGateMismatch {
        unit: &'static str,
    },
    ExecutorCallBudgetExceeded {
        unit: &'static str,
        calls_used: u32,
        calls_remaining: u32,
    },
}

impl fmt::Display for ProbeSchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy => write!(formatter, "política de scheduler inválida"),
            Self::CheckpointIo(error) => write!(formatter, "error de checkpoint: {error}"),
            Self::CheckpointJson(error) => write!(formatter, "checkpoint JSON inválido: {error}"),
            Self::CheckpointMismatch(field) => {
                write!(formatter, "checkpoint incompatible en {field}")
            }
            Self::CheckpointCorrupt(reason) => write!(formatter, "checkpoint corrupto: {reason}"),
            Self::ExecutorGateMismatch { unit } => {
                write!(
                    formatter,
                    "executor devolvió gates incompatibles para {unit}"
                )
            }
            Self::ExecutorCallBudgetExceeded {
                unit,
                calls_used,
                calls_remaining,
            } => write!(
                formatter,
                "executor excedió el presupuesto de llamadas para {unit}: usó {calls_used}, quedaban {calls_remaining}"
            ),
        }
    }
}

impl From<std::io::Error> for ProbeSchedulerError {
    fn from(error: std::io::Error) -> Self {
        Self::CheckpointIo(error)
    }
}

impl From<serde_json::Error> for ProbeSchedulerError {
    fn from(error: serde_json::Error) -> Self {
        Self::CheckpointJson(error)
    }
}

pub(crate) trait ProbeSchedulerClock: Send + Sync {
    fn now_unix_ms(&self) -> u64;
    fn delay(&self, duration: Duration);
}

#[derive(Debug, Default)]
struct SystemProbeSchedulerClock;

impl ProbeSchedulerClock for SystemProbeSchedulerClock {
    fn now_unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis()
            .min(u64::MAX as u128) as u64
    }

    fn delay(&self, duration: Duration) {
        if !duration.is_zero() {
            thread::sleep(duration);
        }
    }
}

pub struct ProbeScheduler {
    config: ProbeSchedulerConfig,
    clock: Arc<dyn ProbeSchedulerClock>,
}

impl fmt::Debug for ProbeScheduler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeScheduler")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ProbeScheduler {
    pub fn new(config: ProbeSchedulerConfig) -> Result<Self, ProbeSchedulerError> {
        config.validate()?;
        Ok(Self {
            config,
            clock: Arc::new(SystemProbeSchedulerClock),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_clock(
        config: ProbeSchedulerConfig,
        clock: Arc<dyn ProbeSchedulerClock>,
    ) -> Result<Self, ProbeSchedulerError> {
        config.validate()?;
        Ok(Self { config, clock })
    }

    pub fn run_live(
        &self,
        probe: &ModelCompatibilityProbe,
        permit: &LiveProbePermit,
        api_key: &str,
    ) -> Result<ProbeScheduleOutcome, ProbeSchedulerError> {
        let calls_limit = probe.config.max_calls.min(permit.max_calls());
        self.run_with_executor(
            &probe.target,
            probe.config.profile,
            calls_limit,
            probe.config.max_elapsed,
            |unit, remaining_calls, remaining_active| {
                probe.execute_unit(api_key, unit, remaining_calls, remaining_active)
            },
        )
    }

    pub(crate) fn run_with_executor<F>(
        &self,
        target: &ProbeTarget,
        profile: ProbeProfile,
        calls_limit: u32,
        active_elapsed_limit: Duration,
        mut execute: F,
    ) -> Result<ProbeScheduleOutcome, ProbeSchedulerError>
    where
        F: FnMut(ProbeExecutionUnit, u32, Duration) -> ProbeUnitExecution,
    {
        let path = self.checkpoint_path(target, profile);
        let policy = self.persisted_policy(calls_limit, active_elapsed_limit);
        let now = self.clock.now_unix_ms();
        let mut checkpoint = if path.exists() {
            self.load_checkpoint(&path)?
        } else {
            let checkpoint = ProbeCheckpoint::new(target.clone(), profile, policy.clone(), now);
            self.persist_checkpoint(&path, &checkpoint)?;
            checkpoint
        };
        self.validate_checkpoint(&checkpoint, target, profile, &policy)?;

        if matches!(
            checkpoint.state,
            ProbeScheduleState::Completed | ProbeScheduleState::Blocked
        ) {
            return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
        }
        if checkpoint.state == ProbeScheduleState::Waiting {
            let not_before =
                checkpoint
                    .not_before_unix_ms
                    .ok_or(ProbeSchedulerError::CheckpointCorrupt(
                        "waiting_without_not_before",
                    ))?;
            if now < not_before {
                return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
            }
            checkpoint.state = ProbeScheduleState::Ready;
            checkpoint.not_before_unix_ms = None;
            checkpoint.updated_at_unix_ms = now;
            self.persist_checkpoint(&path, &checkpoint)?;
        }

        loop {
            let Some(unit) = ProbeExecutionUnit::ORDERED
                .get(checkpoint.next_unit_index)
                .copied()
            else {
                checkpoint.state = ProbeScheduleState::Completed;
                checkpoint.not_before_unix_ms = None;
                checkpoint.updated_at_unix_ms = self.clock.now_unix_ms();
                self.persist_checkpoint(&path, &checkpoint)?;
                return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
            };

            if unit.is_external() && checkpoint.calls_used_total >= checkpoint.policy.calls_limit {
                self.block(&mut checkpoint, &path, "call_limit_reached", None, 0)?;
                return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
            }
            if unit.is_external()
                && checkpoint.active_elapsed_ms_total >= checkpoint.policy.active_elapsed_limit_ms
            {
                self.block(
                    &mut checkpoint,
                    &path,
                    "active_elapsed_limit_reached",
                    None,
                    0,
                )?;
                return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
            }

            let remaining_calls = checkpoint
                .policy
                .calls_limit
                .saturating_sub(checkpoint.calls_used_total);
            let remaining_active_ms = checkpoint
                .policy
                .active_elapsed_limit_ms
                .saturating_sub(checkpoint.active_elapsed_ms_total);
            let execution = execute(
                unit,
                remaining_calls.max(1),
                Duration::from_millis(remaining_active_ms.max(1)),
            );
            let expected_gates = gates_for_unit(unit);
            if execution.gates.len() != expected_gates.len()
                || execution
                    .gates
                    .iter()
                    .zip(expected_gates)
                    .any(|(actual, expected)| actual.gate != expected)
            {
                return Err(ProbeSchedulerError::ExecutorGateMismatch {
                    unit: unit.as_str(),
                });
            }
            if execution.calls_used > remaining_calls {
                return Err(ProbeSchedulerError::ExecutorCallBudgetExceeded {
                    unit: unit.as_str(),
                    calls_used: execution.calls_used,
                    calls_remaining: remaining_calls,
                });
            }
            checkpoint.calls_used_total = checkpoint
                .calls_used_total
                .saturating_add(execution.calls_used);
            checkpoint.active_elapsed_ms_total = checkpoint
                .active_elapsed_ms_total
                .saturating_add(duration_ms(execution.active_elapsed));

            if let Some(pause) = execution.pause {
                self.pause_for_transient(&mut checkpoint, &path, pause)?;
                return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
            }

            let exhausted_budget_reason = execution.gates.iter().find_map(|gate| {
                if gate.layer == ProbeLayer::Budget {
                    match gate.reason_code.as_str() {
                        "call_limit_reached" => Some("call_limit_reached"),
                        "wall_clock_limit_reached" => Some("active_elapsed_limit_reached"),
                        _ => None,
                    }
                } else {
                    None
                }
            });
            if let Some(reason_code) = exhausted_budget_reason {
                self.block(&mut checkpoint, &path, reason_code, None, 0)?;
                return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
            }

            let prerequisite_failed = matches!(
                unit,
                ProbeExecutionUnit::Transport | ProbeExecutionUnit::JsonObjectStrict
            ) && execution
                .gates
                .first()
                .is_some_and(|gate| gate.verdict != ProbeVerdict::Pass);
            let failed_layer = execution
                .gates
                .first()
                .map(|gate| gate.layer)
                .unwrap_or(ProbeLayer::Harness);
            checkpoint.completed_gates.extend(execution.gates);
            checkpoint.next_unit_index = checkpoint.next_unit_index.saturating_add(1);
            checkpoint.state = ProbeScheduleState::Ready;
            checkpoint.not_before_unix_ms = None;
            checkpoint.updated_at_unix_ms = self.clock.now_unix_ms();

            if prerequisite_failed {
                append_prerequisite_results(
                    &mut checkpoint.completed_gates,
                    checkpoint.next_unit_index,
                    failed_layer,
                );
                checkpoint.next_unit_index = ProbeExecutionUnit::ORDERED.len();
                checkpoint.state = ProbeScheduleState::Completed;
                self.persist_checkpoint(&path, &checkpoint)?;
                return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
            }

            self.persist_checkpoint(&path, &checkpoint)?;
            let next_is_external = ProbeExecutionUnit::ORDERED
                .get(checkpoint.next_unit_index)
                .is_some_and(|next| next.is_external());
            if unit.is_external() && next_is_external && !self.config.pacing.is_zero() {
                self.apply_pacing(&mut checkpoint, &path)?;
                if checkpoint.state == ProbeScheduleState::Waiting {
                    return Ok(ProbeScheduleOutcome::from_checkpoint(&checkpoint, path));
                }
            }
        }
    }

    fn persisted_policy(&self, calls_limit: u32, active_limit: Duration) -> PersistedPolicy {
        PersistedPolicy {
            calls_limit,
            active_elapsed_limit_ms: duration_ms(active_limit),
            pacing_ms: duration_ms(self.config.pacing),
            max_recovery_attempts: self.config.max_recovery_attempts,
            max_single_wait_ms: duration_ms(self.config.max_single_wait),
            max_cumulative_wait_ms: duration_ms(self.config.max_cumulative_wait),
            fallback_wait_ms: duration_ms(self.config.fallback_wait),
        }
    }

    fn pause_for_transient(
        &self,
        checkpoint: &mut ProbeCheckpoint,
        path: &Path,
        pause: ProbePauseSignal,
    ) -> Result<(), ProbeSchedulerError> {
        let wait_ms = pause
            .retry_after
            .map(duration_ms)
            .unwrap_or(checkpoint.policy.fallback_wait_ms);
        if checkpoint.calls_used_total >= checkpoint.policy.calls_limit {
            return self.block(
                checkpoint,
                path,
                "call_limit_reached",
                pause.http_status,
                wait_ms,
            );
        }
        if checkpoint.active_elapsed_ms_total >= checkpoint.policy.active_elapsed_limit_ms {
            return self.block(
                checkpoint,
                path,
                "active_elapsed_limit_reached",
                pause.http_status,
                wait_ms,
            );
        }

        if wait_ms > checkpoint.policy.max_single_wait_ms {
            return self.block(
                checkpoint,
                path,
                "retry_after_exceeds_single_wait_cap",
                pause.http_status,
                wait_ms,
            );
        }
        if checkpoint.recovery_attempts_used >= checkpoint.policy.max_recovery_attempts {
            return self.block(
                checkpoint,
                path,
                "recovery_attempts_exhausted",
                pause.http_status,
                wait_ms,
            );
        }
        let cumulative = checkpoint.cumulative_wait_ms.saturating_add(wait_ms);
        if cumulative > checkpoint.policy.max_cumulative_wait_ms {
            return self.block(
                checkpoint,
                path,
                "cumulative_wait_exhausted",
                pause.http_status,
                wait_ms,
            );
        }
        let now = self.clock.now_unix_ms();
        checkpoint.recovery_attempts_used = checkpoint.recovery_attempts_used.saturating_add(1);
        checkpoint.cumulative_wait_ms = cumulative;
        checkpoint.state = ProbeScheduleState::Waiting;
        checkpoint.not_before_unix_ms = Some(now.saturating_add(wait_ms));
        checkpoint.last_pause = Some(PersistedPause {
            reason_code: pause.reason_code.to_string(),
            retry_after_ms: wait_ms,
            http_status: pause.http_status,
            pacing_only: false,
        });
        checkpoint.updated_at_unix_ms = now;
        self.persist_checkpoint(path, checkpoint)
    }

    fn apply_pacing(
        &self,
        checkpoint: &mut ProbeCheckpoint,
        path: &Path,
    ) -> Result<(), ProbeSchedulerError> {
        let wait_ms = checkpoint.policy.pacing_ms;
        let now = self.clock.now_unix_ms();
        checkpoint.state = ProbeScheduleState::Waiting;
        checkpoint.not_before_unix_ms = Some(now.saturating_add(wait_ms));
        checkpoint.last_pause = Some(PersistedPause {
            reason_code: "inter_gate_pacing".to_string(),
            retry_after_ms: wait_ms,
            http_status: None,
            pacing_only: true,
        });
        checkpoint.updated_at_unix_ms = now;
        self.persist_checkpoint(path, checkpoint)?;
        self.clock.delay(Duration::from_millis(wait_ms));
        let after = self.clock.now_unix_ms();
        if after >= checkpoint.not_before_unix_ms.unwrap_or(u64::MAX) {
            checkpoint.state = ProbeScheduleState::Ready;
            checkpoint.not_before_unix_ms = None;
            checkpoint.updated_at_unix_ms = after;
            self.persist_checkpoint(path, checkpoint)?;
        }
        Ok(())
    }

    fn block(
        &self,
        checkpoint: &mut ProbeCheckpoint,
        path: &Path,
        reason_code: &'static str,
        http_status: Option<u16>,
        retry_after_ms: u64,
    ) -> Result<(), ProbeSchedulerError> {
        checkpoint.state = ProbeScheduleState::Blocked;
        checkpoint.not_before_unix_ms = None;
        checkpoint.last_pause = Some(PersistedPause {
            reason_code: reason_code.to_string(),
            retry_after_ms,
            http_status,
            pacing_only: false,
        });
        checkpoint.updated_at_unix_ms = self.clock.now_unix_ms();
        self.persist_checkpoint(path, checkpoint)
    }

    fn checkpoint_path(&self, target: &ProbeTarget, profile: ProbeProfile) -> PathBuf {
        let identity_material = format!(
            "{}\0{}\0{}\0{}\0{}",
            MODEL_COMPATIBILITY_SUITE_VERSION,
            target.provider,
            target.model,
            target.base_url,
            profile.as_str(),
        );
        let identity = format!(
            "{}-{}-{}-{:016x}.json",
            sanitize_component(&target.provider),
            sanitize_component(&target.model),
            profile.as_str(),
            stable_hash(identity_material.as_bytes()),
        );
        self.config.checkpoint_dir.join(identity)
    }

    fn validate_checkpoint(
        &self,
        checkpoint: &ProbeCheckpoint,
        target: &ProbeTarget,
        profile: ProbeProfile,
        policy: &PersistedPolicy,
    ) -> Result<(), ProbeSchedulerError> {
        if checkpoint.schema_version != CHECKPOINT_SCHEMA_VERSION {
            return Err(ProbeSchedulerError::CheckpointMismatch("schema_version"));
        }
        if checkpoint.suite_version != MODEL_COMPATIBILITY_SUITE_VERSION {
            return Err(ProbeSchedulerError::CheckpointMismatch("suite_version"));
        }
        if checkpoint.target != *target {
            return Err(ProbeSchedulerError::CheckpointMismatch("target"));
        }
        if checkpoint.profile != profile {
            return Err(ProbeSchedulerError::CheckpointMismatch("profile"));
        }
        if checkpoint.policy != *policy {
            return Err(ProbeSchedulerError::CheckpointMismatch("policy"));
        }
        if checkpoint.next_unit_index > ProbeExecutionUnit::ORDERED.len() {
            return Err(ProbeSchedulerError::CheckpointCorrupt(
                "next_unit_out_of_range",
            ));
        }
        let total_units = ProbeExecutionUnit::ORDERED.len();
        match checkpoint.state {
            ProbeScheduleState::Completed if checkpoint.next_unit_index != total_units => {
                return Err(ProbeSchedulerError::CheckpointCorrupt(
                    "completed_without_terminal_cursor",
                ));
            }
            ProbeScheduleState::Ready
            | ProbeScheduleState::Waiting
            | ProbeScheduleState::Blocked
                if checkpoint.next_unit_index >= total_units =>
            {
                return Err(ProbeSchedulerError::CheckpointCorrupt(
                    "non_completed_with_terminal_cursor",
                ));
            }
            _ => {}
        }
        if checkpoint.state == ProbeScheduleState::Waiting
            && (checkpoint.not_before_unix_ms.is_none() || checkpoint.last_pause.is_none())
        {
            return Err(ProbeSchedulerError::CheckpointCorrupt(
                "waiting_without_pause",
            ));
        }
        if checkpoint.state != ProbeScheduleState::Waiting
            && checkpoint.not_before_unix_ms.is_some()
        {
            return Err(ProbeSchedulerError::CheckpointCorrupt(
                "not_before_without_waiting",
            ));
        }
        if checkpoint.state == ProbeScheduleState::Blocked && checkpoint.last_pause.is_none() {
            return Err(ProbeSchedulerError::CheckpointCorrupt(
                "blocked_without_reason",
            ));
        }
        if checkpoint.calls_used_total > checkpoint.policy.calls_limit {
            return Err(ProbeSchedulerError::CheckpointCorrupt("calls_exceed_limit"));
        }
        if checkpoint.recovery_attempts_used > checkpoint.policy.max_recovery_attempts {
            return Err(ProbeSchedulerError::CheckpointCorrupt(
                "recoveries_exceed_limit",
            ));
        }
        if checkpoint.cumulative_wait_ms > checkpoint.policy.max_cumulative_wait_ms {
            return Err(ProbeSchedulerError::CheckpointCorrupt("wait_exceeds_limit"));
        }
        let expected_gates: Vec<ProbeGate> = ProbeExecutionUnit::ORDERED
            .iter()
            .take(checkpoint.next_unit_index)
            .flat_map(|unit| gates_for_unit(*unit))
            .collect();
        if checkpoint.completed_gates.len() != expected_gates.len()
            || checkpoint
                .completed_gates
                .iter()
                .zip(expected_gates)
                .any(|(actual, expected)| actual.gate != expected)
        {
            return Err(ProbeSchedulerError::CheckpointCorrupt(
                "completed_gates_do_not_match_cursor",
            ));
        }
        Ok(())
    }

    fn load_checkpoint(&self, path: &Path) -> Result<ProbeCheckpoint, ProbeSchedulerError> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn persist_checkpoint(
        &self,
        path: &Path,
        checkpoint: &ProbeCheckpoint,
    ) -> Result<(), ProbeSchedulerError> {
        let parent = path.parent().ok_or(ProbeSchedulerError::CheckpointCorrupt(
            "checkpoint_without_parent",
        ))?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(checkpoint)?;
        let file_name = path.file_name().and_then(|name| name.to_str()).ok_or(
            ProbeSchedulerError::CheckpointCorrupt("checkpoint_without_safe_filename"),
        )?;
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            self.clock.now_unix_ms(),
        ));
        let result = (|| -> Result<(), std::io::Error> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(ProbeSchedulerError::CheckpointIo)
    }
}

fn append_pending_report_results(checkpoint: &ProbeCheckpoint, gates: &mut Vec<ProbeGateResult>) {
    if checkpoint.state == ProbeScheduleState::Completed {
        return;
    }
    let pause = checkpoint.last_pause.as_ref();
    let primary_reason = pause
        .map(|value| value.reason_code.as_str())
        .unwrap_or("scheduler_pending");
    let primary_layer = pause.map(pause_layer).unwrap_or(ProbeLayer::Harness);
    let primary_verdict = match checkpoint.state {
        ProbeScheduleState::Waiting if pause.is_some_and(|value| !value.pacing_only) => {
            ProbeVerdict::Inconclusive
        }
        ProbeScheduleState::Blocked => ProbeVerdict::Blocked,
        ProbeScheduleState::Ready | ProbeScheduleState::Waiting => ProbeVerdict::NotTested,
        ProbeScheduleState::Completed => return,
    };

    for (offset, unit) in ProbeExecutionUnit::ORDERED
        .iter()
        .skip(checkpoint.next_unit_index)
        .enumerate()
    {
        let is_current = offset == 0;
        for gate in gates_for_unit(*unit) {
            gates.push(ProbeGateResult {
                gate,
                verdict: if is_current {
                    primary_verdict
                } else {
                    ProbeVerdict::NotTested
                },
                layer: primary_layer,
                reason_code: if is_current {
                    primary_reason.to_string()
                } else {
                    "prerequisite_not_satisfied".to_string()
                },
                metrics: Vec::new(),
            });
        }
    }
}

fn pause_layer(pause: &PersistedPause) -> ProbeLayer {
    if pause.http_status.is_some()
        || matches!(
            pause.reason_code.as_str(),
            "timeout" | "transport_failure" | "provider_server_error" | "rate_limited"
        )
    {
        ProbeLayer::External
    } else {
        ProbeLayer::Budget
    }
}

fn append_prerequisite_results(
    gates: &mut Vec<ProbeGateResult>,
    start_index: usize,
    layer: ProbeLayer,
) {
    for unit in ProbeExecutionUnit::ORDERED.iter().skip(start_index) {
        for gate in gates_for_unit(*unit) {
            gates.push(ProbeGateResult {
                gate,
                verdict: ProbeVerdict::NotTested,
                layer,
                reason_code: "prerequisite_not_satisfied".to_string(),
                metrics: Vec::new(),
            });
        }
    }
}

pub(crate) fn gates_for_unit(unit: ProbeExecutionUnit) -> Vec<ProbeGate> {
    match unit {
        ProbeExecutionUnit::RetryAfterSynthetic => vec![ProbeGate::RetryAfterSynthetic],
        ProbeExecutionUnit::Transport => vec![ProbeGate::TransportPromptOnly],
        ProbeExecutionUnit::JsonObjectStrict => vec![ProbeGate::JsonObjectStrict],
        ProbeExecutionUnit::ActionValidate => vec![ProbeGate::ActionValidate],
        ProbeExecutionUnit::ActionRepairDiagnostic => vec![ProbeGate::ActionRepairDiagnostic],
        ProbeExecutionUnit::ActionApplyCorrection => vec![ProbeGate::ActionApplyCorrection],
        ProbeExecutionUnit::ActionApplyFileOperations => vec![ProbeGate::ActionApplyFileOperations],
        ProbeExecutionUnit::ActionCompile => vec![ProbeGate::ActionCompile],
        ProbeExecutionUnit::ActionRunTests => vec![ProbeGate::ActionRunTests],
        ProbeExecutionUnit::ActionRunClippy => vec![ProbeGate::ActionRunClippy],
        ProbeExecutionUnit::ActionCheckFormat => vec![ProbeGate::ActionCheckFormat],
        ProbeExecutionUnit::ActionFinish => vec![ProbeGate::ActionFinish],
        ProbeExecutionUnit::RepairBundle => vec![
            ProbeGate::AutonomousRepair,
            ProbeGate::MultiFile,
            ProbeGate::BoundedConvergence,
        ],
        ProbeExecutionUnit::NativeToolCalling => vec![ProbeGate::NativeToolCalling],
    }
}

fn sanitize_component(raw: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_dash = false;
    for character in raw.chars().take(96) {
        let safe = if character.is_ascii_alphanumeric() {
            previous_dash = false;
            Some(character.to_ascii_lowercase())
        } else if !previous_dash {
            previous_dash = true;
            Some('-')
        } else {
            None
        };
        if let Some(safe) = safe {
            sanitized.push(safe);
        }
    }
    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "target".to_string()
    } else {
        sanitized.to_string()
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}
