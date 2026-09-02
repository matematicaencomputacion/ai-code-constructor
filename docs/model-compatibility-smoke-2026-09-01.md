# NVIDIA model compatibility smoke - 2026-09-01

## Scope

The `model-compatibility-v1` smoke profile was executed against four NVIDIA
Inference API model IDs. Only synthetic prompts and fixtures were sent. No
repository source, API key, raw prompt, raw response, or HTTP body is retained
in this report.

> **Evidence status:** all live observations in this document predate the
> in-memory `RepairBundle` hardening. None of those runs reached
> `repair_bundle`, so they neither exercised an unsafe generated-code process
> nor validate the current no-process boundary. They remain useful only as
> historical transport, adapter, rate-limit and scheduler evidence. The current
> boundary is covered by deterministic tests and has not been re-run live.

## Results

| Model | Calls | Transport | Strict JSON | Actions / repair / multi-file / convergence | Overall |
| --- | ---: | --- | --- | --- | --- |
| `moonshotai/kimi-k3` | 3 | Pass | Pass | Inconclusive: first action received `429`; remaining gates not tested | Inconclusive |
| `deepseek-ai/deepseek-v4-pro-0813` | 1 | Inconclusive: timeout at 120 s | Not tested | Not tested | Inconclusive |
| `nvidia/nemotron-3.5-lightning-30b-a3b` | 1 | Inconclusive: timeout at 120 s | Not tested | Not tested | Inconclusive |
| `nvidia/nemotron-3-ultra-550b-a55b` | 1 | Inconclusive: provider 5xx | Not tested | Not tested | Inconclusive |

The synthetic `429/Retry-After` adaptive-recovery gate passed. Native tool
calling remains `not_tested` because the current adapter intentionally consumes
structured action JSON from `choices[0].message.content` and does not expose
`tool_calls`.

## Findings

1. None of the four models can be certified or rejected from this run. External
   availability prevented the full suite from completing.
2. Within this limited run, Kimi produced the most positive evidence: it
   demonstrated compatible transport and strict JSON behavior before rate
   limiting. This is not a current ranking or a certification.
3. A live `429` originally allowed later independent gates to continue. The
   probe now opens an external circuit after the first rate limit, timeout, or
   transient provider failure, preventing quota waste.
4. Per-call timeout alone did not bound the total suite duration. The probe now
   has a shared wall-clock budget: 180 seconds per model for `smoke` and 600
   seconds for `certify`. Every call uses the smaller of its configured timeout
   and the remaining model budget.
5. `.env` is ignored by Git and was restricted to owner-only permissions. The
   local runner read a named NVIDIA key without executing the dotenv file or
   printing its value. This was credential hygiene, not isolation against
   environment inheritance by child processes.

## Resumable scheduler v2 validation

The accepted scheduler unit is implemented as `model-compatibility-v2`. Its
checkpoint is versioned, written atomically, keyed by safe target identity and
profile, and excluded from Git. It persists completed gates, the pending unit,
calls, active elapsed time, recovery attempts, cumulative wait and
`not_before_unix_ms`. It never stores credentials, prompts, responses or HTTP
bodies.

Deterministic evidence covers:

- a first run with transport pass, JSON pass and `429` on the first action;
- an early resume that performs zero model calls;
- a later resume that retries only the pending action and completes;
- persistence and exhaustion of call, recovery and cumulative-wait budgets;
- blocking when `Retry-After` exceeds its cap;
- immediate blocking when a timeout has already exhausted active time;
- checkpoint policy mismatch, path sanitization and absence of sensitive fields;
- both delta-seconds and HTTP-date forms of `Retry-After` from RFC 9110.

Before the `RepairBundle` hardening, a bounded live run against
`moonshotai/kimi-k3` added independent historical adapter evidence:

| Observation | Result |
| --- | --- |
| Calls before pause | 12 |
| Transport / strict JSON | Pass / Pass |
| Action gates | 7 pass; `check_format` and `finish` returned invalid action responses |
| Pending unit | `repair_bundle` |
| External interruption | Timeout after the active budget reached 180.146 s |
| First terminal state | `waiting`, fallback 5 s |
| Resume | 0.21 s, calls remained 12, no completed gate was replayed |
| Final state | `blocked` because the persisted active-time budget was exhausted |

That historical live run exposed one edge case: a transient failure had already
consumed the full active-time budget. The current implementation closes it with
deterministic regression evidence: new runs block immediately instead of
scheduling a recovery wait that cannot execute. The historical run itself is
not evidence of the hardened `RepairBundle` boundary.

## Recommended next unit

Add checkpoint lifecycle and single-writer coordination before running multiple
models concurrently: immutable `run_id` generations, an explicit non-destructive
`new-run`/archive operation, status/list commands and an advisory lease. This
prevents a blocked historical checkpoint or two local processes from becoming
ambiguous authority over the same target.
