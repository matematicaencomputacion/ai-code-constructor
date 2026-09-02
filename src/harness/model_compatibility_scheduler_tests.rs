use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::model_compatibility_probe::{
    ProbeExecutionUnit, ProbeGate, ProbeGateResult, ProbeLayer, ProbePauseSignal, ProbeProfile,
    ProbeTarget, ProbeUnitExecution, ProbeVerdict,
};
use super::model_compatibility_scheduler::{
    ProbeScheduleState, ProbeScheduler, ProbeSchedulerClock, ProbeSchedulerConfig,
    ProbeSchedulerError, gates_for_unit,
};

static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct FakeClock {
    now_ms: AtomicU64,
    waits: Mutex<Vec<Duration>>,
    advance_on_delay: bool,
}

impl FakeClock {
    fn new(now_ms: u64, advance_on_delay: bool) -> Self {
        Self {
            now_ms: AtomicU64::new(now_ms),
            waits: Mutex::new(Vec::new()),
            advance_on_delay,
        }
    }

    fn advance(&self, duration: Duration) {
        self.now_ms
            .fetch_add(duration.as_millis() as u64, Ordering::SeqCst);
    }
}

impl ProbeSchedulerClock for FakeClock {
    fn now_unix_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn delay(&self, duration: Duration) {
        self.waits.lock().expect("waits").push(duration);
        if self.advance_on_delay {
            self.advance(duration);
        }
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let id = TEST_DIRECTORY_ID.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "ai-code-constructor-probe-scheduler-{}-{id}",
            std::process::id(),
        ));
        fs::create_dir_all(&path).expect("test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target() -> ProbeTarget {
    ProbeTarget::nvidia(
        "nvidia/model/../../unsafe",
        "https://integrate.api.nvidia.com/v1",
    )
    .expect("target")
}

fn execution(
    unit: ProbeExecutionUnit,
    verdict: ProbeVerdict,
    calls: u32,
    pause: Option<ProbePauseSignal>,
) -> ProbeUnitExecution {
    let gates = gates_for_unit(unit)
        .into_iter()
        .map(|gate| ProbeGateResult {
            gate,
            verdict,
            layer: if verdict == ProbeVerdict::Pass {
                ProbeLayer::Model
            } else {
                ProbeLayer::External
            },
            reason_code: if verdict == ProbeVerdict::Pass {
                "test_pass".to_string()
            } else {
                "rate_limited".to_string()
            },
            metrics: Vec::new(),
        })
        .collect();
    ProbeUnitExecution {
        gates,
        calls_used: calls,
        active_elapsed: Duration::from_millis(u64::from(calls)),
        pause,
    }
}

#[test]
fn empty_executor_gates_fail_closed_without_poisoning_checkpoint() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(100, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");

    let error = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                let mut invalid = execution(unit, ProbeVerdict::Pass, 0, None);
                invalid.gates.clear();
                invalid
            },
        )
        .expect_err("empty gates must fail closed");
    assert!(matches!(
        error,
        ProbeSchedulerError::ExecutorGateMismatch {
            unit: "retry_after_synthetic"
        }
    ));

    let completed = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| execution(unit, ProbeVerdict::Pass, 0, None),
        )
        .expect("checkpoint remains valid");
    assert_eq!(completed.state, ProbeScheduleState::Completed);
}

#[test]
fn out_of_order_executor_gates_do_not_advance_repair_bundle() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(110, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");
    let mut executions = BTreeMap::<String, u32>::new();

    let error = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                *executions.entry(unit.as_str().to_string()).or_default() += 1;
                let mut result = execution(unit, ProbeVerdict::Pass, 0, None);
                if unit == ProbeExecutionUnit::RepairBundle {
                    result.gates.swap(0, 1);
                }
                result
            },
        )
        .expect_err("out-of-order gates must fail closed");
    assert!(matches!(
        error,
        ProbeSchedulerError::ExecutorGateMismatch {
            unit: "repair_bundle"
        }
    ));

    let completed = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                *executions.entry(unit.as_str().to_string()).or_default() += 1;
                execution(unit, ProbeVerdict::Pass, 0, None)
            },
        )
        .expect("resume from repair bundle");
    assert_eq!(completed.state, ProbeScheduleState::Completed);
    assert_eq!(executions.get("transport_prompt_only"), Some(&1));
    assert_eq!(executions.get("json_object_strict"), Some(&1));
    assert_eq!(executions.get("repair_bundle"), Some(&2));
}

#[test]
fn executor_call_overshoot_fails_closed_without_poisoning_checkpoint() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(120, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");

    let error = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            2,
            Duration::from_secs(180),
            |unit, _, _| {
                execution(
                    unit,
                    ProbeVerdict::Pass,
                    if unit == ProbeExecutionUnit::Transport {
                        3
                    } else {
                        0
                    },
                    None,
                )
            },
        )
        .expect_err("call overshoot must fail closed");
    assert!(matches!(
        error,
        ProbeSchedulerError::ExecutorCallBudgetExceeded {
            unit: "transport_prompt_only",
            calls_used: 3,
            calls_remaining: 2,
        }
    ));

    let completed = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            2,
            Duration::from_secs(180),
            |unit, _, _| {
                execution(
                    unit,
                    ProbeVerdict::Pass,
                    u32::from(unit == ProbeExecutionUnit::Transport),
                    None,
                )
            },
        )
        .expect("checkpoint remains valid after overshoot");
    assert_eq!(completed.state, ProbeScheduleState::Completed);
    assert_eq!(completed.report.calls_used, 1);
}

#[test]
fn resumes_same_action_without_repeating_transport_or_json() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(1_000, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::from_millis(10);
    let scheduler = ProbeScheduler::with_clock(config, clock.clone()).expect("scheduler");
    let mut counts = BTreeMap::<String, u32>::new();
    let mut action_rate_limited = false;

    let first = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                *counts.entry(unit.as_str().to_string()).or_default() += 1;
                if unit == ProbeExecutionUnit::ActionValidate && !action_rate_limited {
                    action_rate_limited = true;
                    return execution(
                        unit,
                        ProbeVerdict::Inconclusive,
                        1,
                        Some(ProbePauseSignal {
                            reason_code: "rate_limited",
                            retry_after: Some(Duration::from_secs(7)),
                            http_status: Some(429),
                        }),
                    );
                }
                execution(
                    unit,
                    ProbeVerdict::Pass,
                    u32::from(unit.is_external()),
                    None,
                )
            },
        )
        .expect("first run");
    assert_eq!(first.state, ProbeScheduleState::Waiting);
    assert_eq!(first.next_unit.as_deref(), Some("action_validate"));
    assert_eq!(counts.get("transport_prompt_only"), Some(&1));
    assert_eq!(counts.get("json_object_strict"), Some(&1));
    assert_eq!(first.report.overall_verdict(), ProbeVerdict::Inconclusive);

    let executions_before_early_resume: u32 = counts.values().sum();
    let early = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                *counts.entry(unit.as_str().to_string()).or_default() += 1;
                execution(
                    unit,
                    ProbeVerdict::Pass,
                    u32::from(unit.is_external()),
                    None,
                )
            },
        )
        .expect("early resume");
    assert_eq!(early.state, ProbeScheduleState::Waiting);
    assert_eq!(early.report.overall_verdict(), ProbeVerdict::Inconclusive);
    assert_eq!(counts.values().sum::<u32>(), executions_before_early_resume);

    clock.advance(Duration::from_secs(7));
    let completed = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                *counts.entry(unit.as_str().to_string()).or_default() += 1;
                execution(
                    unit,
                    ProbeVerdict::Pass,
                    u32::from(unit.is_external()),
                    None,
                )
            },
        )
        .expect("completed resume");
    assert_eq!(completed.state, ProbeScheduleState::Completed);
    assert_eq!(counts.get("transport_prompt_only"), Some(&1));
    assert_eq!(counts.get("json_object_strict"), Some(&1));
    assert_eq!(counts.get("action_validate"), Some(&2));
    assert_eq!(completed.report.calls_used, 13);
}

#[test]
fn retry_after_above_cap_blocks_without_advancing_unit() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(10, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    config.max_single_wait = Duration::from_secs(10);
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");

    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                if unit == ProbeExecutionUnit::Transport {
                    execution(
                        unit,
                        ProbeVerdict::Inconclusive,
                        1,
                        Some(ProbePauseSignal {
                            reason_code: "rate_limited",
                            retry_after: Some(Duration::from_secs(11)),
                            http_status: Some(429),
                        }),
                    )
                } else {
                    execution(unit, ProbeVerdict::Pass, 0, None)
                }
            },
        )
        .expect("blocked outcome");
    assert_eq!(outcome.state, ProbeScheduleState::Blocked);
    assert_eq!(outcome.next_unit.as_deref(), Some("transport_prompt_only"));
    assert_eq!(outcome.recovery_attempts_used, 0);
    assert_eq!(outcome.report.overall_verdict(), ProbeVerdict::Blocked);
}

#[test]
fn exhausted_call_budget_blocks_without_scheduling_wait() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(20, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");

    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            1,
            Duration::from_secs(180),
            |unit, _, _| {
                if unit == ProbeExecutionUnit::Transport {
                    execution(
                        unit,
                        ProbeVerdict::Inconclusive,
                        1,
                        Some(ProbePauseSignal {
                            reason_code: "rate_limited",
                            retry_after: Some(Duration::from_secs(1)),
                            http_status: Some(429),
                        }),
                    )
                } else {
                    execution(unit, ProbeVerdict::Pass, 0, None)
                }
            },
        )
        .expect("call budget block");

    assert_eq!(outcome.state, ProbeScheduleState::Blocked);
    assert_eq!(outcome.next_unit.as_deref(), Some("transport_prompt_only"));
    assert_eq!(outcome.recovery_attempts_used, 0);
    assert_eq!(outcome.cumulative_wait_ms, 0);
    assert_eq!(outcome.report.calls_used, 1);
    assert_eq!(outcome.report.overall_verdict(), ProbeVerdict::Blocked);
}

#[test]
fn exact_call_budget_cannot_advance_a_partial_external_unit() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(25, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");
    let mut json_executed = false;

    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Certify,
            2,
            Duration::from_secs(180),
            |unit, remaining_calls, _| {
                if unit == ProbeExecutionUnit::Transport {
                    assert_eq!(remaining_calls, 2);
                    let mut partial = execution(unit, ProbeVerdict::Inconclusive, 2, None);
                    partial.gates[0].layer = ProbeLayer::Budget;
                    partial.gates[0].reason_code = "call_limit_reached".to_string();
                    return partial;
                }
                if unit == ProbeExecutionUnit::JsonObjectStrict {
                    json_executed = true;
                }
                execution(unit, ProbeVerdict::Pass, 0, None)
            },
        )
        .expect("partial unit must block at its cursor");

    assert_eq!(outcome.state, ProbeScheduleState::Blocked);
    assert_eq!(outcome.next_unit.as_deref(), Some("transport_prompt_only"));
    assert_eq!(outcome.report.calls_used, 2);
    assert_eq!(outcome.report.overall_verdict(), ProbeVerdict::Blocked);
    assert!(!json_executed);
}

#[test]
fn exact_call_budget_cannot_complete_after_not_tested_repair_bundle() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(30, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");
    let mut native_tool_calling_executed = false;

    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            12,
            Duration::from_secs(180),
            |unit, remaining_calls, _| {
                if unit == ProbeExecutionUnit::RepairBundle {
                    assert_eq!(remaining_calls, 1);
                    let mut partial = execution(unit, ProbeVerdict::NotTested, 1, None);
                    for gate in &mut partial.gates {
                        gate.layer = ProbeLayer::Budget;
                        gate.reason_code = "call_limit_reached".to_string();
                    }
                    return partial;
                }
                if unit == ProbeExecutionUnit::NativeToolCalling {
                    native_tool_calling_executed = true;
                }
                execution(
                    unit,
                    ProbeVerdict::Pass,
                    u32::from(unit.is_external()),
                    None,
                )
            },
        )
        .expect("partial final external unit must block");

    assert_eq!(outcome.state, ProbeScheduleState::Blocked);
    assert_eq!(outcome.next_unit.as_deref(), Some("repair_bundle"));
    assert_eq!(outcome.report.calls_used, 12);
    assert_eq!(outcome.report.overall_verdict(), ProbeVerdict::Blocked);
    assert!(!native_tool_calling_executed);
}

#[test]
fn checkpoint_is_safe_atomic_and_policy_mismatch_is_rejected() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(50, false));
    let scheduler =
        ProbeScheduler::with_clock(ProbeSchedulerConfig::in_directory(&directory.0), clock)
            .expect("scheduler");
    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                execution(
                    unit,
                    ProbeVerdict::Inconclusive,
                    1,
                    Some(ProbePauseSignal {
                        reason_code: "timeout",
                        retry_after: None,
                        http_status: None,
                    }),
                )
            },
        )
        .expect("waiting outcome");
    let json = fs::read_to_string(&outcome.checkpoint_path).expect("checkpoint");
    assert!(!json.contains("nvapi-"));
    assert!(!json.contains("Bearer"));
    assert!(!json.contains("system_prompt"));
    assert!(
        !outcome
            .checkpoint_path
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .contains("..")
    );
    assert_eq!(
        fs::read_dir(&directory.0)
            .expect("directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
            .count(),
        0
    );

    let mismatch = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            31,
            Duration::from_secs(180),
            |_, _, _| panic!("mismatch must not execute"),
        )
        .expect_err("policy mismatch");
    assert!(matches!(
        mismatch,
        ProbeSchedulerError::CheckpointMismatch("policy")
    ));
}

#[test]
fn semantically_corrupt_completed_checkpoint_is_rejected() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(60, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");
    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| execution(unit, ProbeVerdict::Pass, 0, None),
        )
        .expect("completed checkpoint");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&outcome.checkpoint_path).expect("checkpoint bytes"))
            .expect("checkpoint json");
    value["completed_gates"]
        .as_array_mut()
        .expect("gates array")
        .truncate(1);
    fs::write(
        &outcome.checkpoint_path,
        serde_json::to_vec_pretty(&value).expect("serialized corrupt checkpoint"),
    )
    .expect("write corrupt checkpoint");

    let error = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |_, _, _| panic!("corrupt checkpoint must not execute"),
        )
        .expect_err("semantic corruption");

    assert!(matches!(
        error,
        ProbeSchedulerError::CheckpointCorrupt("completed_gates_do_not_match_cursor")
    ));
}

#[test]
fn sanitized_model_names_cannot_collide_on_checkpoint_path() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(70, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");
    let run = |model: &str| {
        let target =
            ProbeTarget::nvidia(model, "https://integrate.api.nvidia.com/v1").expect("target");
        scheduler
            .run_with_executor(
                &target,
                ProbeProfile::Smoke,
                32,
                Duration::from_secs(180),
                |unit, _, _| {
                    if unit == ProbeExecutionUnit::Transport {
                        execution(
                            unit,
                            ProbeVerdict::Inconclusive,
                            1,
                            Some(ProbePauseSignal {
                                reason_code: "timeout",
                                retry_after: None,
                                http_status: None,
                            }),
                        )
                    } else {
                        execution(unit, ProbeVerdict::Pass, 0, None)
                    }
                },
            )
            .expect("waiting checkpoint")
    };

    let slash = run("nvidia/foo/bar");
    let dash = run("nvidia/foo-bar");

    assert_ne!(slash.checkpoint_path, dash.checkpoint_path);
}

#[test]
fn failed_transport_closes_dependents_as_not_tested() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(100, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");
    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            32,
            Duration::from_secs(180),
            |unit, _, _| {
                if unit == ProbeExecutionUnit::Transport {
                    let mut result = execution(unit, ProbeVerdict::Fail, 1, None);
                    result.gates[0].layer = ProbeLayer::Model;
                    result
                } else {
                    execution(unit, ProbeVerdict::Pass, 0, None)
                }
            },
        )
        .expect("terminal report");
    assert_eq!(outcome.state, ProbeScheduleState::Completed);
    assert_eq!(outcome.report.overall_verdict(), ProbeVerdict::Fail);
    assert!(outcome.report.gates.iter().any(|gate| {
        gate.gate == ProbeGate::JsonObjectStrict && gate.verdict == ProbeVerdict::NotTested
    }));
}
#[test]
fn recovery_and_call_budgets_survive_process_resumes() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(500, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    config.max_recovery_attempts = 2;
    config.fallback_wait = Duration::from_secs(1);
    let scheduler = ProbeScheduler::with_clock(config, clock.clone()).expect("scheduler");
    let mut transport_calls = 0_u32;

    for expected_state in [
        ProbeScheduleState::Waiting,
        ProbeScheduleState::Waiting,
        ProbeScheduleState::Blocked,
    ] {
        let outcome = scheduler
            .run_with_executor(
                &target(),
                ProbeProfile::Smoke,
                8,
                Duration::from_secs(180),
                |unit, _, _| {
                    if unit == ProbeExecutionUnit::Transport {
                        transport_calls += 1;
                        execution(
                            unit,
                            ProbeVerdict::Inconclusive,
                            1,
                            Some(ProbePauseSignal {
                                reason_code: "timeout",
                                retry_after: None,
                                http_status: None,
                            }),
                        )
                    } else {
                        execution(unit, ProbeVerdict::Pass, 0, None)
                    }
                },
            )
            .expect("resumable recovery");
        assert_eq!(outcome.state, expected_state);
        if expected_state == ProbeScheduleState::Waiting {
            clock.advance(Duration::from_secs(1));
        } else {
            assert_eq!(outcome.recovery_attempts_used, 2);
            assert_eq!(outcome.cumulative_wait_ms, 2_000);
            assert_eq!(outcome.report.calls_used, 3);
        }
    }

    assert_eq!(transport_calls, 3);
}
#[test]
fn exhausted_active_budget_does_not_schedule_retry_wait() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(900, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");

    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            8,
            Duration::from_secs(180),
            |unit, _, _| {
                let mut result = if unit == ProbeExecutionUnit::Transport {
                    execution(
                        unit,
                        ProbeVerdict::Inconclusive,
                        1,
                        Some(ProbePauseSignal {
                            reason_code: "timeout",
                            retry_after: None,
                            http_status: None,
                        }),
                    )
                } else {
                    execution(unit, ProbeVerdict::Pass, 0, None)
                };
                if unit == ProbeExecutionUnit::Transport {
                    result.active_elapsed = Duration::from_secs(181);
                }
                result
            },
        )
        .expect("budget block");

    assert_eq!(outcome.state, ProbeScheduleState::Blocked);
    assert_eq!(outcome.next_unit.as_deref(), Some("transport_prompt_only"));
    assert_eq!(outcome.recovery_attempts_used, 0);
    assert_eq!(outcome.cumulative_wait_ms, 0);
    assert_eq!(outcome.report.calls_used, 1);
}

#[test]
fn partial_wall_clock_budget_result_does_not_advance_cursor() {
    let directory = TestDirectory::new();
    let clock = Arc::new(FakeClock::new(950, true));
    let mut config = ProbeSchedulerConfig::in_directory(&directory.0);
    config.pacing = Duration::ZERO;
    let scheduler = ProbeScheduler::with_clock(config, clock).expect("scheduler");

    let outcome = scheduler
        .run_with_executor(
            &target(),
            ProbeProfile::Smoke,
            8,
            Duration::from_secs(180),
            |unit, _, _| {
                let mut result = if unit == ProbeExecutionUnit::Transport {
                    execution(unit, ProbeVerdict::Inconclusive, 1, None)
                } else {
                    execution(unit, ProbeVerdict::Pass, 0, None)
                };
                if unit == ProbeExecutionUnit::Transport {
                    result.gates[0].layer = ProbeLayer::Budget;
                    result.gates[0].reason_code = "wall_clock_limit_reached".to_string();
                    result.active_elapsed = Duration::from_secs(180);
                }
                result
            },
        )
        .expect("partial active budget must block at its cursor");

    assert_eq!(outcome.state, ProbeScheduleState::Blocked);
    assert_eq!(outcome.next_unit.as_deref(), Some("transport_prompt_only"));
    assert_eq!(outcome.recovery_attempts_used, 0);
}
