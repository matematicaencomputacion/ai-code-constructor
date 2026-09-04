# ai-code-constructor

An experimental architecture for autonomous software construction, where AI agents
operate inside a verifiable engineering harness.

This repository began as a Rust code constructor and evolved into a research
platform for **controlled agent autonomy**: the model proposes actions; the
harness controls their execution.

> **Central principle:** the model is part of the system, not the system.
> Autonomy is designed *around* the model, not delegated to it without bounds.

---

## What problem this project explores

Generating source code is not enough for reliable software construction.

A language model can emit text that *looks* like a program, but software
engineering requires:

- verification against intent,
- structured feedback when something fails,
- bounded actions with observable effects,
- reproducible decision loops,
- separation between *proposal* and *execution*.

**ai-code-constructor** asks whether we can build a system where an AI agent
has enough autonomy to develop non-trivial Rust artifacts, while every
important action remains **observable, restricted, verifiable, and testable**.

---

## Philosophy

The project prioritizes:

| Principle | Meaning here |
|-----------|--------------|
| Systems thinking before tooling | Architecture defines what agents may do |
| Architecture before implementation | Harness contracts precede prompts |
| Contracts before prompts | Capabilities need tests and acceptance criteria |
| Orchestration before execution | AgentLoop and Harness gate every action |
| Infrastructure before interface | ModelClient is transport, not control plane |
| Evidence before trust | Decisions follow Observation, not assumptions |
| Verification before generation | Validate and compile before declaring success |
| Reproducibility before speed | Deterministic mocks and explicit limits in CI |
| Conceptual clarity before optimization | Typed `AgentAction`, not free-form tool calls |
| Controlled autonomy | Allowlists, iteration caps, structured corrections |

---

## Two coexisting layers

The codebase contains **two related but separate execution paths**.

### 1. Constructor cycle (entry point today)

The CLI (`main`) runs the traditional pipeline:

```
Request
   ↓
Planner
   ↓
Builder  →  Compiler  →  Validator
                ↑            ↓
                └── Repairer ←┘
                      ↓
                  CodeState
```

- **`CodeState`** holds request, plan, generated code, errors, feedback, and
  iteration count.
- **`Planner`** derives a `BuildPlan` from keyword rules (API, calculator,
  authentication, generic).
- **`Builder`** generates Rust source per plan. Iteration 1 deliberately
  produces defective code to exercise the repair loop.
- **`Compiler`** invokes `rustc` on temporary files.
- **`Validator`** checks plan-specific structural requirements.
- **`Repairer`** converts compiler/validator errors into textual feedback for
  the Builder.

This path is **implemented and exercised** by `cargo run` and unit tests.
It does **not** use a live LLM.

### 2. Harness layer (experimental, not wired to CLI)

The `src/harness/` module implements agent orchestration **without replacing**
the Constructor:

```
AgentContext + working_code
        ↓
    AgentLoop
        ↓
      Agent  (mock agents | AiAgent)
        ↓
   AgentAction  (typed, serializable)
        ↓
     Harness
        ↓
   Constraints  (e.g. tool allowlist)
        ↓
      Tools
        ↓
   ToolResult → Evidence → Evaluation → AgentObservation
        ↓
   next Agent decision
```

The harness module is compiled (`mod harness` in `main.rs`) but marked
`dead_code` at the entry point: **`run_constructor` is unchanged** and remains
the default runtime behavior.

---

## Harness Engineering (in this project)

**Harness Engineering** here means: every agent capability is mediated by an
orchestrator that can **permit, reject, execute, evaluate, and record** an
action before the agent sees the next observation.

Roles:

| Component | Role |
|-----------|------|
| **Model** | Proposes structured decisions (via `ModelClient`) |
| **Agent** | Converts model output into `AgentAction` |
| **Harness** | Runs Constraint → Tool → Evaluation |
| **Tool** | Performs one bounded capability |
| **Verifier** | Validator / Compiler (via Tools or Constructor modules) |
| **Evidence** | Labeled facts from tool output |
| **Observation** | Structured feedback to the Agent |

```
MODEL     proposes
AGENT     decides (parses + validates)
HARNESS   controls
TOOL      executes
VERIFIER  checks (Validator, Compiler, clippy, fmt, tests)
EVIDENCE  demonstrates
OBSERVATION informs the next decision
```

**The model never executes tools directly.**

---

## Agent Loop

`AgentLoop` is the autonomy kernel:

```
Observe  (AgentContext + last AgentObservation)
   ↓
Decide   (Agent::propose)
   ↓
Act      (Harness::execute_step)
   ↓
Observe  (push AgentObservation)
   ↓
  …
```

Properties implemented today:

- **Explicit iteration limit** (`max_iterations`). When reached →
  `LoopStatus::MaxIterations`. The model cannot override this.
- **Provider-agnostic**: the loop depends only on `Agent`, `Harness`, and
  `AgentContext`.
- **Full history**: `LoopHistory` records proposed actions, rejections, tool
  results, evidence, evaluations, and observations.

Live sessions use `LIVE_AGENT_MAX_ITERATIONS = 12` as a strict cap.

### Goal convergence and adaptive recovery

When an evaluation specification is present, the loop also records measurable
Goal progress. `GoalProgressTracker` compares criterion verdicts, gap size,
state fingerprints, artifact revision, and the state/action pair:

- a larger pass count or smaller gap is progress;
- swapping one passing criterion for another is lateral movement, not progress;
- alternating actions over an unchanged Goal still consumes the stale window;
- historical progress is retained for reporting, but only recent progress can
  suppress a model escalation.

Failure handling is a composed, observable decision:

```
FailureEvidence + recent Goal progress
        ├── plan_recovery (retry / wait / terminal)
        └── plan_routing  (stay / switch / escalate / stop)
                         ↓
             plan_adaptive_recovery
                         ↓
        AdaptiveRecoveryBudget (single session ledger)
                         ↓
              retry | route | stop
```

The common ledger bounds recovery attempts, model switches, and cumulative
wait. Its current defaults are 3 recovery attempts, 2 model switches, and 300
seconds of cumulative wait. A provider `Retry-After` hint has priority over
generic backoff, but it cannot exceed that cumulative wait budget.

Routing planning is side-effect free. The selected route is applied only after
the common budget authorizes it. The product live entrypoint likewise uses
`AgentLoop` as the single retry authority; `RetryingModelClient` remains an
explicit compatibility/standalone wrapper rather than being stacked by default.

---

## Contracts (direction of travel)

The project is evolving toward **explicit contracts** for each capability.

Conceptually, a contract spans:

```
Input
  ↓
Action
  ↓
Permission   (Constraint)
  ↓
Execution    (Tool)
  ↓
Evidence
  ↓
Evaluation
  ↓
Observation
```

A feature is not considered complete because it "works once". It needs:

- a contract (inputs, allowed actions, expected evidence),
- implementation,
- tests demonstrating causality,
- acceptance criteria.

Example flow **demonstrated by tests** (mock agents and `MockModelClient`):

```
Validation FAIL
    ↓ Observation
RepairDiagnostic
    ↓ Observation
ApplyCorrection
    ↓ Observation
Validation PASS
    ↓ Observation
Compile PASS
    ↓ Observation
Finish
```

See `harness::bridge` tests, `harness::ai_agent::ai_agent_e2e_loop_with_mock_model_client`,
and `harness::live_session::live_session_with_mock_model_client_completes_flow`.

Formal contract documents (`docs/contracts/`) do **not** exist yet.

---

## AgentAction

Agents may only emit **typed, enumerable actions** (`AgentAction`):

| Action | Tool (if any) | Purpose |
|--------|---------------|---------|
| `Validate` | `validate` | Run real `Validator` on code + plan |
| `RepairDiagnostic` | `repair_diagnostic` | Run real `Repairer` for feedback only |
| `ApplyCorrection` | `apply_correction` | Structured edits to session code |
| `Compile` | `compile` | Compile a code fragment via `rustc` |
| `RunTests` | `run_tests` | `cargo test` with filter |
| `RunClippy` | `run_clippy` | `cargo clippy -- -D warnings` |
| `CheckFormat` | `check_format` | `cargo fmt --check` |
| `InvokeTool` | named tool | Controlled indirection |
| `Finish` | — | End session |
| `NoOp` | — | No operation |

`AiAgent` parses model JSON, validates schema, and maps to `AgentAction`.
Invalid model output → `Finish` with error; **no tool runs**.

---

## Tools and Constraints

Tools are **registered capabilities**, not agent privileges.

```
Agent
  ↓
AgentAction
  ↓
Harness
  ↓
Constraint          ← ToolPermissionConstraint (allowlist)
  ↓
Tool
  ↓
ToolResult
  ↓
Evidence
```

`ToolPermissionConstraint::default_constructor_tools()` allows:

`compile`, `validate`, `repair_diagnostic`, `apply_correction`, `run_tests`,
`run_clippy`, `check_format`.

Any other tool name → action **rejected**; the tool does not run.

`ValidationTool` and `RepairDiagnosticTool` wrap the existing Constructor
`Validator` and `Repairer`. They produce evidence labels such as
`validator_error_*` and `repairer_feedback_*`.

---

## Correction (architectural shift)

**Before (conceptually):** agent returns a full corrected program.

**Now (implemented):**

```
Agent → ApplyCorrection { corrections: [Correction] }
              ↓
        CorrectionTool
              ↓
     AgentContext::working_code
```

Corrections are **atomic structured operations**:

- `ReplaceText { search, replacement }`
- `InsertText { position, text }`
- `RemoveText { start, end }`

Target is restricted to `CorrectionTarget::SessionCode` (sandbox minimum).

This matters because:

- edits are diff-like and auditable,
- full-file rewrites are discouraged,
- the harness owns `working_code`, not the model,
- the same Tool and Harness work for mock and live agents.

---

## CorrectionPolicy

Repair **strategy** is separated from the Agent.

- Trait: `CorrectionPolicy`
- Default: `DeterministicCorrectionPolicy` (rule-based, testable)

Agents such as `BridgedValidateRepairAgent` delegate correction planning to the
policy. An AI-driven policy could be added later **without changing** Harness,
AgentLoop, or CorrectionTool.

---

## ModelClient and AiAgent

`AiAgent` does not know the model provider.

```
AiAgent
   ↓
ModelClient (trait)
   ├── MockModelClient      (deterministic, CI-safe)
   └── OpenAICompatibleModelClient
```

`RetryingModelClient` remains available for standalone or legacy callers.
The default live session passes the raw compatible client to `AgentLoop`,
which owns retry, `Retry-After`, and the shared budget.

- **`ModelRequest`** carries goal, step, working code, serialized observations,
  evidence, and a versioned system prompt (`SYSTEM_PROMPT_V1`).
- **`OpenAICompatibleModelClient`** handles HTTP transport, auth, and response
  extraction only — no tool execution, no harness logic.
- Configuration via environment: `MODEL_BASE_URL`, `MODEL_API_KEY`, `MODEL_NAME`,
  `MODEL_TIMEOUT_MS`.

Swapping models should not require changing Harness architecture.

---

## ConstructorBridge

`ConstructorBridge` connects the Constructor world to the Harness world:

- `ConstructorArtifacts::snapshot(&CodeState)` — read-only snapshot (`Arc` shared
  data; does not mutate original state).
- `session_from_state` → `BridgedSession` with `AgentContext`.
- `run_session` → delegates to `AgentLoop`.

This enables future flows: **Constructor produces an artifact → Agent refines it
inside the harness** without corrupting the original `CodeState`.

---

## RustArtifact (controlled artifact)

`RustArtifact` (`harness/artifact.rs`) is the **working product** domain object (contract v2):

- stable `ArtifactId` (independent of source content),
- logical multi-file tree: `ArtifactPath` → source (`src/…`, `tests/…`),
- **primary** file for single-file compatibility (`source()` / `replace_source` / `working_code()`),
- `name` (label), `language`, `contract_version`, `revision`,
- optional `specification_id` for in-memory Specification → Artifact traceability,
- `AgentContext.working_artifact` is canonical; `working_code()` returns the **primary** source,
- `replace_source` updates only the primary and preserves sibling files,
- `ArtifactMaterialization` writes every file under an ephemeral Cargo crate (RAII), never the host workspace,
- `CompileTool` / Test / Clippy / Fmt run on that materialized crate (`cargo check` / `test` / `clippy` / `fmt`);
  ValidationTool still uses the **primary** buffer (compat).
- `Correction` may target an existing `ArtifactPath`; legacy corrections without path still use **primary**.
- Model JSON corrections may include optional `"path"` per operation; omitted path edits the primary file.

---

## Live Agent Session (experimental)

`run_live_agent_session` composes:

```
OpenAICompatibleModelClient
  → AiAgent
  → Harness (validate / repair / correct / compile tools)
  → AgentLoop
      └── progress + failure classification
          + adaptive recovery/routing budget
```

First use case implemented: validate and compile an existing Rust artifact with
a **real model choosing actions** (not a hardcoded sequence in `AiAgent`).

`LiveSessionConfig::from_specification` resolves Specification → `plan_specification` →
Builder Initial Artifact (single- or multi-file) without manual `working_code`.
Legacy constructors (`validate_and_compile_artifact`, `with_artifact`, etc.) remain available.

Manual test (excluded from CI):

```bash
export MODEL_BASE_URL=...
export MODEL_API_KEY=...
export MODEL_NAME=...
cargo test manual_live_agent_session -- --ignored --nocapture
```

Autonomous compile repair (broken multi-file helper → real model repair):

```bash
export MODEL_BASE_URL=...
export MODEL_API_KEY=...
export MODEL_NAME=...
cargo run -- live-repair-smoke
# or:
cargo test manual_live_autonomous_repair_session -- --ignored --nocapture
```

Without `MODEL_*` credentials, `cargo run -- live-repair-smoke` prints setup instructions (CI-safe).

---

## Security (what is actually implemented)

Do not overstate protections. Today the sandbox is **minimal and explicit**:

| Mechanism | What it does |
|-----------|--------------|
| No direct tool access from model | Model output → parse → Harness only |
| `ToolPermissionConstraint` | Allowlist of tool names |
| `CorrectionTarget::SessionCode` | Corrections only on harness working code |
| Typed `AgentAction` | No arbitrary shell or path fields |
| Iteration limits | Constructor (3) and live session (12) |
| Secret redaction | `redact_secrets` in public model errors |
| System prompt rules | Instructs model; enforcement is architectural |

**Not implemented:** full filesystem sandbox, network isolation, syscall filtering,
arbitrary-path write prevention beyond the correction target design, or a shell
tool.

Some product/session harnesses **do** register tools that invoke `cargo` or
`rustc` (compile, clippy, fmt, tests). `ToolPermissionConstraint` limits which
registered capability may be selected; it is not a process sandbox and does
not isolate the filesystem, network, syscalls, or inherited environment.
Generated-code execution is therefore not a certified security boundary.

---

## Testing as architecture

Tests are not a footnote. They **demonstrate contracts and causality**.

The suite currently runs 612 tests (608 passing; 4 manual/live tests ignored) and covers:

- harness step execution and rejection,
- tool evidence shape,
- constraint allowlist,
- correction operations and policy,
- model response parsing and invalid-response safety,
- AgentLoop termination and max iterations,
- ConstructorBridge snapshot immutability,
- E2E mock flows: validate → repair → correct → validate → compile → finish,
- HTTP client behavior with local mock server,
- retry policy for transient model errors,
- state/action convergence, lateral-state detection, and bounded NonProgress,
- structured failure classification and signal-aware `Retry-After`,
- common-budget recovery plus multi-model capability escalation,
- retry observability (`last` ≠ `total` ≠ AgentLoop `iteration_count`; `None` ≠ zero).

CI (`.github/workflows/ci.yml`): `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo check`, `cargo test` — no network, no live LLM.

**Retry observability (causal):** `last_retry_count` and
`ModelRetryObservability::per_call()` describe an explicitly injected client
wrapper. `LiveSessionTrace::total_retries` includes coordinator retries and,
when present, wrapper retries. AgentLoop iterations remain a separate measure.

---

## Current Status

### Implemented

- Constructor pipeline: Planner, Builder, Compiler, Validator, Repairer, `CodeState`
- CLI entry via `run_constructor` / `cargo run -- "<request>"`
- CLI `export`: `export_artifact` writes a minimal Cargo package (`Cargo.toml` + `src/`) from the in-memory catalog (`demo` / `api` / `calculator` / `authentication`); accepts `--out <dir>` and `--force`
- Harness core: AgentLoop, Harness, Agent trait, Context, Observation, Evidence, Evaluation
- Goal → Gap → RecommendedAction → Model → Tools → Evidence guidance
- `GoalProgressTracker` with repeated state/action and lateral-change detection
- Structured failure classification and terminal `FailureReport`
- `AdaptiveRecoveryBudget` for attempts, model switches, and cumulative wait
- Pure recovery/routing composition with observable `AdaptiveRecoveryDecision`
- Optional multi-model routing with bounded capability escalation and no revisits
- Typed `AgentAction` and Constraint framework
- Tools: compile, validate, repair_diagnostic, apply_correction, run_tests, clippy, fmt
- Structured `Correction` + `CorrectionTool`
- `CorrectionPolicy` + deterministic implementation
- `ModelClient`, `MockModelClient`, `AiAgent`, JSON decision parsing
- `OpenAICompatibleModelClient` + opt-in `RetryingModelClient`
- `ModelRetryObservability` (handle causal): `last` / `total` / `per_call` — ortogonal a `AgentLoop` iterations
- LiveSession reports coordinator retries plus optional client-wrapper retries
- `ConstructionObservability.model_retry_count`: `Some(total)` con handle inyectado; `None` sin fuente causal (no inventa 0)
- `ConstructorBridge` + artifact snapshot from `CodeState`
- Live session scaffolding + versioned system prompt
- `RustArtifact` with revision tracking and canonical AgentContext integration
- `Specification` contract: goal, requirements, acceptance criteria, structural validation
- Deterministic `plan_specification`: `Specification` → `BuildPlan` via `SpecificationBuildPlan` (traceable WHAT → HOW)
- Evidence-based `EvaluationEngine`: AcceptanceCriterion + Evidence → PASS / FAIL / InsufficientEvidence (deterministic, no LLM)
- Explicit `CriterionKind` on `AcceptanceCriterion` (semantics live in the contract, not in the ID)
- Evidence-based evaluation for Validate / Compile / RunTests / Clippy / CheckFormat (`tool` + scoped `exit_status`)
- AiAgent / ModelDecision surface for `run_tests` / `run_clippy` / `check_format` (same decision chain)
- Evaluation → Observation bridge (`observation_from_*`) so Agents decide from typed verification results
- AgentLoop evaluation cycle: Tool → Evidence → EvaluationEngine → Observation → Agent (optional via `evaluation_specification`)
- `RustArtifact` as first-class working product in `AgentContext` / live sessions / ConstructorBridge
- ActionPolicy / ActionConstraints: permission ≠ action validity (Artifact / Repair / Correction / Finish)
- LiveSession uses `ActionPolicy::default_session_policy()` by default (injectable)
- Autonomous construction session: Specification → Plan → Artifact → AgentLoop → ConstructionResult
- Deterministic Initial Artifact from `builder::initial_artifact_definition_for_kind(PlanKind)` (caller override optional)
- `PlanKind::Authentication` produces `src/main.rs` + `src/auth.rs`; other kinds remain single-file
- Autonomous construction observability (`ConstructionObservability` derived from LoopResult / Evaluation)
- Artifact-scoped quality tools: Test/Clippy/Fmt materialize `RustArtifact` into an ephemeral Cargo crate (RAII), never the host workspace
- Multi-file `RustArtifact` (`ArtifactPath` + primary compat) + multi-file materialization
- `CompileTool` via materialized crate (`cargo check`), same isolation as quality tools
- Live quality demo wiring (`LiveSessionConfig::quality_verification_artifact`)
- LiveSession can start from `LiveSessionConfig::from_specification` (Specification → Plan → Builder → Initial Artifact, including multi-file Authentication)
- Artifacts can evolve structurally via `apply_file_operations` (`create_file`, `delete_file`, `rename_file`) on the canonical `RustArtifact`
- Mutation tools preview changes (`ToolResult.artifact_preview`); the Harness performs a single canonical commit (`commit_artifact_preview`). Correction batches are atomic (+1 `revision` per successful batch, same as file operations)
- CI quality gate

### Experimental

- Full harness stack (not default CLI path)
- `AiAgent` with live model (`manual_live_agent_session`, ignored in CI)
- Live validate-and-compile session
- Live quality-verification session (`manual_live_quality_agent_session`, ignored in CI)
- `AutonomousConstructionSession` (Specification-driven, not the CLI Constructor)
- Bridged agents (`BridgedValidateRepairAgent`, etc.) for demonstration
- Prompt-driven decisions (provider-agnostic prompt v1)

### Next (reasonable architectural units)

- Wire configured model catalogs into the product LiveSession entrypoint
- Attach `UnitCompletionRecord` to terminal session/construction outcomes
- Extend the common ledger to explicit model-call, tool-call, and wall-clock caps
- Extend multi-file initial artifacts to additional PlanKinds when justified

### Long-term vision (not implemented)

- Specification-driven construction from full PRD / multi-file workspaces
- Artifact graphs and richer workspaces
- Filesystem / shell / Git tools under harness control
- AI-based CorrectionPolicy
- Persistent memory, multi-agent coordination, MCP, RAG
- Provider catalog discovery and streaming

---

## Roadmap (conceptual evolution)

```
Constructor
   ↓
Harness
   ↓
Agent Loop
   ↓
AiAgent
   ↓
Controlled Artifact  ← RustArtifact (started)
   ↓
Specification
   ↓
Evaluation
   ↓
Autonomous Software Construction   (research goal, not current product)
```

---

## Running the project

**Constructor (default):**

```bash
cargo run -- "Crear una API REST"
cargo run -- "Crear una calculadora"
cargo run -- "Crear un sistema de autenticación"
```

**Export artifact (Oleada 1):**

`export_artifact` materializes a `RustArtifact` as a minimal Cargo package
(`Cargo.toml` + `src/`). The catalog is in-memory (`demo`, `api`,
`calculator`, `authentication`); there is no persistent artifact store yet.
The resulting directory is compilable with `cargo build` in its own directory.

```bash
cargo run -- export demo --out /tmp/exported-demo
cargo run -- export demo /tmp/exported-demo
cargo run -- export --force --artifact-id authentication --out /tmp/exported-auth
```

`--force` allows writing into a non-empty directory. Paths inside the artifact
are still resolved with `ArtifactPath::resolve_under` (no traversal).

**Quality checks:**

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
cargo check
```

**Dependencies:** Rust stable; `ureq` para HTTP, `serde`/`serde_json` para checkpoints y `httpdate` para `Retry-After` RFC 9110.

## Model compatibility probe

`ModelCompatibilityProbe` certifica de forma acotada transporte, gramática de
decisiones y comportamiento causal del Harness sin guardar prompts, respuestas
crudas o credenciales. No certifica todavía la ejecución real de código generado.

**Preflight sin red ni secretos:**

```bash
cargo run -- model-compatibility-probe --dry-run --profile smoke --max-calls 32 --json
```

**Ejecución live explícita:**

```bash
export NVIDIA_API_KEY='configure-locally'
cargo run -- model-compatibility-probe \
  --profile smoke \
  --max-calls 32 \
  --checkpoint-dir .model-probe-state \
  --pacing-ms 2000 \
  --max-recoveries 3 \
  --max-retry-after-ms 120000 \
  --max-cumulative-wait-ms 300000 \
  --ack-live \
  --json
```

Por defecto recorre `moonshotai/kimi-k3`, `deepseek-ai/deepseek-v4-pro-0813`, `nvidia/nemotron-3.5-lightning-30b-a3b` y `nvidia/nemotron-3-ultra-550b-a55b`. `--model ID` puede repetirse para seleccionar un subconjunto.

La base URL live debe usar HTTPS; HTTP se rechaza antes de adjuntar o enviar `NVIDIA_API_KEY`.

La suite evalúa transporte prompt-only, JSON estricto, todas las acciones
válidas, reparación autónoma, edición multiarchivo, convergencia acotada y
el contrato de `429/Retry-After`. Los gates individuales de acciones solo validan
el JSON producido por el modelo; no entregan esas acciones al Harness ni ejecutan herramientas.

El `repair_bundle` live usa un Harness exclusivo del probe. `Validate`,
`RepairDiagnostic`, `ApplyCorrection` y `ApplyFileOperations` operan sobre el
artefacto en memoria, mientras que `Compile` se resuelve con
`ProbeCompileTool`, un verificador sintético que no materializa archivos ni
inicia procesos. `RunTests`, `RunClippy` y `CheckFormat` no están
registrados en ese Harness y cualquier intento de usarlos dentro del bundle es
rechazado. Un `pass` demuestra la secuencia causal sobre la fixture sintética;
la ejecución real mediante `cargo` o `rustc` permanece fuera de alcance hasta
incorporar un `SandboxRunner`.

El scheduler persiste un checkpoint atómico por target y perfil. Un `429`,
timeout, `5xx` o fallo de transporte elegible deja el estado en `waiting` solo
si quedan presupuestos suficientes para reintentarlo; si se agotó el límite de
llamadas, recoveries, tiempo activo o espera, queda `blocked`. En ambos casos
conserva el primer gate pendiente y no repite gates ya confirmados.

`Retry-After` admite delta-seconds y HTTP-date según RFC 9110. Una espera superior a `--max-retry-after-ms`, o que exceda `--max-cumulative-wait-ms`, deja el checkpoint en `blocked` en vez de truncar o insistir. El gate sintético nunca fuerza un rate limit real.

El adaptador actual consume acciones JSON en `choices[0].message.content`; por eso tool-calling nativo se informa como `not_tested` y no afecta el veredicto general.

---

## Repository layout (high level)

```
src/
  main.rs           # CLI → run_constructor
  planner.rs        # PlanKind + planning rules
  builder.rs        # Code generation per plan
  compiler.rs       # rustc on temp files
  validator.rs      # Plan-aware validation
  repairer.rs       # Error → feedback
  state.rs          # CodeState
  harness/          # Agent orchestration layer
    agent_loop.rs
    runtime.rs      # Harness
    action.rs
    agent.rs
    ai_agent.rs
    model.rs
    openai_compatible_client.rs
    retrying_model_client.rs
    bridge.rs
    live_session.rs
    artifact.rs
    correction.rs
    correction_policy.rs
    tools/
```

Las decisiones de arquitectura y contratos activos se documentan en `docs/` y `docs/adr/`.

---

## Vision

> Can we build a system where an AI has enough autonomy to develop complex
> software, while every important action remains observable, restricted,
> verifiable, and reproducible?

**ai-code-constructor** is an experiment in answering that question — one
contract, one tool, and one test at a time.

---

## License

Not specified in repository metadata at time of writing.
