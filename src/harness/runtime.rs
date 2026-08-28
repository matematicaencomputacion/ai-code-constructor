use crate::harness::action::AgentAction;
use crate::harness::agent::Agent;
use crate::harness::artifact_mutation::commit_artifact_preview;
use crate::harness::constraint::{Constraint, ConstraintDecision};
use crate::harness::context::AgentContext;
use crate::harness::evaluation::{Evaluation, EvaluationVerdict, Evidence};
use crate::harness::observation::AgentObservation;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools;

/// Resultado de un único paso del Harness (Constraint → Tool → Evaluation → Observation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutcome {
    pub action: AgentAction,
    pub permitted: bool,
    pub rejected_reason: Option<String>,
    /// Nombre de la constraint que rechazó, si aplica.
    pub rejected_constraint: Option<String>,
    pub tool_executed: bool,
    pub tool_name: Option<String>,
    pub tool_result: Option<ToolResult>,
    pub evaluation: Evaluation,
    pub observation: AgentObservation,
    pub evidence: Vec<Evidence>,
}

/// Resultado estructurado de una ejecución multi-paso del Harness (`run`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessResult {
    pub actions_executed: Vec<AgentAction>,
    pub rejected_actions: Vec<(AgentAction, String)>,
    pub evaluations: Vec<Evaluation>,
    pub final_context: AgentContext,
    pub completed: bool,
}

/// Orquestador model-agnostic: AgentAction → Constraint → Tool → Evaluation.
pub struct Harness {
    tools: Vec<Box<dyn Tool>>,
    constraints: Vec<Box<dyn Constraint>>,
    max_steps: u32,
}

impl Harness {
    pub fn new(max_steps: u32) -> Self {
        Self {
            tools: Vec::new(),
            constraints: Vec::new(),
            max_steps,
        }
    }

    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn register_constraint(&mut self, constraint: Box<dyn Constraint>) {
        self.constraints.push(constraint);
    }

    /// Ejecuta una sola acción bajo control del Harness.
    pub fn execute_step(&self, action: AgentAction, ctx: &mut AgentContext) -> StepOutcome {
        if let Some((constraint, reason)) = self.reject_detail(&action, ctx) {
            let evidence = vec![
                Evidence::new("constraint", constraint.clone()),
                Evidence::new("reject_reason", reason.clone()),
            ];
            let evaluation =
                Evaluation::fail(format!("acción rechazada: {reason}"), evidence.clone());
            let observation = AgentObservation::ActionRejected {
                action: action.clone(),
                reason: reason.clone(),
                constraint: constraint.clone(),
            };
            ctx.push_observation(observation.clone());
            return StepOutcome {
                action,
                permitted: false,
                rejected_reason: Some(reason),
                rejected_constraint: Some(constraint),
                tool_executed: false,
                tool_name: None,
                tool_result: None,
                evaluation,
                observation,
                evidence,
            };
        }

        match action.clone() {
            AgentAction::NoOp => {
                let evidence = vec![Evidence::new("action", "NoOp")];
                let evaluation = Evaluation::pass("noop aceptado", evidence.clone());
                let observation = AgentObservation::NoOpDone;
                ctx.push_observation(observation.clone());
                StepOutcome {
                    action,
                    permitted: true,
                    rejected_reason: None,
                    rejected_constraint: None,
                    tool_executed: false,
                    tool_name: None,
                    tool_result: None,
                    evaluation,
                    observation,
                    evidence,
                }
            }

            AgentAction::Finish { summary } => {
                let evidence = vec![Evidence::new("summary", summary.clone())];
                let evaluation = Evaluation::pass("ejecución finalizada", evidence.clone());
                let observation = AgentObservation::Finished {
                    summary: summary.clone(),
                };
                ctx.push_observation(observation.clone());
                StepOutcome {
                    action,
                    permitted: true,
                    rejected_reason: None,
                    rejected_constraint: None,
                    tool_executed: false,
                    tool_name: None,
                    tool_result: None,
                    evaluation,
                    observation,
                    evidence,
                }
            }

            AgentAction::Compile { code } => {
                if !code.is_empty() {
                    ctx.update_working_source(&code);
                }
                let source = ctx.working_code().map(str::to_string).unwrap_or(code);
                self.dispatch_named_tool(action, tools::COMPILE, &source, ctx)
            }
            AgentAction::RunTests { filter } => {
                self.dispatch_named_tool(action, tools::RUN_TESTS, &filter, ctx)
            }
            AgentAction::RunClippy => self.dispatch_named_tool(action, tools::RUN_CLIPPY, "", ctx),
            AgentAction::CheckFormat => {
                self.dispatch_named_tool(action, tools::CHECK_FORMAT, "", ctx)
            }
            AgentAction::Validate {
                request,
                code,
                plan_kind,
            } => {
                if let Some(ref snapshot) = code {
                    ctx.update_working_source(snapshot);
                }
                let resolved = code
                    .clone()
                    .or_else(|| ctx.working_code().map(str::to_string));
                let input = tools::encode_validate_input(&request, resolved.as_deref(), &plan_kind);
                self.dispatch_named_tool(action, tools::VALIDATE, &input, ctx)
            }
            AgentAction::RepairDiagnostic { errors } => {
                let input = tools::encode_repair_diagnostic_input(&errors);
                self.dispatch_named_tool(action, tools::REPAIR_DIAGNOSTIC, &input, ctx)
            }
            AgentAction::ApplyCorrection { .. } | AgentAction::ApplyFileOperations { .. } => {
                match action.clone() {
                    AgentAction::ApplyCorrection { corrections } => {
                        let input = tools::encode_correction_input(&corrections);
                        self.commit_mutation_tool(action, tools::APPLY_CORRECTION, &input, ctx)
                    }
                    AgentAction::ApplyFileOperations { operations } => {
                        let input = tools::encode_file_operations_input(&operations);
                        self.commit_mutation_tool(action, tools::APPLY_FILE_OPERATIONS, &input, ctx)
                    }
                    _ => unreachable!("mutation actions only"),
                }
            }
            AgentAction::InvokeTool { tool_name, input } => {
                self.dispatch_named_tool(action, &tool_name, &input, ctx)
            }
        }
    }

    /// Bucle legacy de compatibilidad; el Agent Loop nuevo usa [`crate::harness::AgentLoop`].
    pub fn run(&self, agent: &mut dyn Agent, mut ctx: AgentContext) -> HarnessResult {
        let mut actions_executed = Vec::new();
        let mut rejected_actions = Vec::new();
        let mut evaluations = Vec::new();
        let mut completed = false;

        while ctx.step < self.max_steps {
            ctx.step += 1;
            let action = agent.propose(&ctx);
            let outcome = self.execute_step(action, &mut ctx);
            evaluations.push(outcome.evaluation.clone());

            if !outcome.permitted {
                if let Some(reason) = outcome.rejected_reason.clone() {
                    rejected_actions.push((outcome.action.clone(), reason));
                }
                continue;
            }

            if matches!(outcome.action, AgentAction::Finish { .. }) {
                actions_executed.push(outcome.action);
                completed = true;
                break;
            }

            actions_executed.push(outcome.action);
        }

        if !completed && ctx.step >= self.max_steps {
            evaluations.push(Evaluation::fail(
                "límite de pasos del harness alcanzado",
                vec![Evidence::new("max_steps", self.max_steps.to_string())],
            ));
        }

        HarnessResult {
            actions_executed,
            rejected_actions,
            evaluations,
            final_context: ctx,
            completed,
        }
    }

    fn dispatch_named_tool(
        &self,
        action: AgentAction,
        tool_name: &str,
        input: &str,
        ctx: &mut AgentContext,
    ) -> StepOutcome {
        match self.find_tool(tool_name) {
            Some(tool) => {
                let result = tool.execute(input, ctx);
                let evidence = result.evidence.clone();
                let evaluation = if result.success {
                    Evaluation::pass(format!("tool `{tool_name}` ok"), evidence.clone())
                } else {
                    Evaluation::fail(
                        format!("tool `{tool_name}` falló: {}", result.output),
                        evidence.clone(),
                    )
                };
                let observation = AgentObservation::ToolOutcome {
                    tool_name: tool_name.to_string(),
                    success: result.success,
                    output: result.output.clone(),
                    evidence: evidence.clone(),
                    verdict: evaluation.verdict,
                };
                ctx.push_observation(observation.clone());
                StepOutcome {
                    action,
                    permitted: true,
                    rejected_reason: None,
                    rejected_constraint: None,
                    tool_executed: true,
                    tool_name: Some(tool_name.to_string()),
                    tool_result: Some(result),
                    evaluation,
                    observation,
                    evidence,
                }
            }
            None => {
                let reason = format!("herramienta desconocida: {tool_name}");
                let evidence = vec![Evidence::new("tool", tool_name)];
                let evaluation = Evaluation::fail(reason.clone(), evidence.clone());
                let observation = AgentObservation::UnknownTool {
                    tool_name: tool_name.to_string(),
                };
                ctx.push_observation(observation.clone());
                StepOutcome {
                    action,
                    permitted: true,
                    rejected_reason: None,
                    rejected_constraint: None,
                    tool_executed: false,
                    tool_name: Some(tool_name.to_string()),
                    tool_result: None,
                    evaluation,
                    observation,
                    evidence,
                }
            }
        }
    }

    fn reject_detail(&self, action: &AgentAction, ctx: &AgentContext) -> Option<(String, String)> {
        for constraint in &self.constraints {
            match constraint.check(action, ctx) {
                ConstraintDecision::Allow => {}
                ConstraintDecision::Reject { reason } => {
                    if constraint.name() == "action_policy" {
                        return Some(split_policy_reason(&reason));
                    }
                    return Some((constraint.name().to_string(), reason));
                }
            }
        }
        None
    }

    fn find_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(|tool| tool.as_ref())
    }

    /// Ejecuta una Tool de mutación: preview en la Tool, commit canónico único en el Harness.
    fn commit_mutation_tool(
        &self,
        action: AgentAction,
        tool_name: &str,
        input: &str,
        ctx: &mut AgentContext,
    ) -> StepOutcome {
        let outcome = self.dispatch_named_tool(action, tool_name, input, ctx);
        let Some(tool_result) = outcome.tool_result.as_ref() else {
            return outcome;
        };
        if !tool_result.success {
            return outcome;
        }
        let Some(preview) = tool_result.artifact_preview.clone() else {
            return outcome;
        };
        let Some(artifact) = ctx.working_artifact.as_mut() else {
            return mutation_commit_failed(outcome, "working_artifact ausente para commit");
        };
        match commit_artifact_preview(artifact, preview) {
            Ok(_) => outcome,
            Err(error) => mutation_commit_failed(outcome, &error),
        }
    }
}

fn split_policy_reason(reason: &str) -> (String, String) {
    match reason.split_once(": ") {
        Some((constraint, detail)) => (constraint.to_string(), detail.to_string()),
        None => ("action_policy".to_string(), reason.to_string()),
    }
}

fn mutation_commit_failed(mut outcome: StepOutcome, error: &str) -> StepOutcome {
    let evidence = vec![
        Evidence::new("mutation_commit", "error"),
        Evidence::new("mutation_commit_error", error),
    ];
    let evaluation = Evaluation::fail(
        format!("commit canónico de mutación falló: {error}"),
        evidence.clone(),
    );
    let observation = AgentObservation::ToolOutcome {
        tool_name: outcome
            .tool_name
            .clone()
            .unwrap_or_else(|| "mutation".to_string()),
        success: false,
        output: error.to_string(),
        evidence: evidence.clone(),
        verdict: evaluation.verdict,
    };
    outcome.tool_executed = true;
    outcome.evaluation = evaluation;
    outcome.observation = observation.clone();
    outcome.evidence = evidence;
    if let Some(tool_result) = outcome.tool_result.as_mut() {
        tool_result.success = false;
        tool_result.output = error.to_string();
        tool_result.artifact_preview = None;
    }
    outcome
}

impl HarnessResult {
    pub fn has_pass(&self) -> bool {
        self.evaluations
            .iter()
            .any(|e| matches!(e.verdict, EvaluationVerdict::Pass))
    }

    pub fn has_fail(&self) -> bool {
        self.evaluations
            .iter()
            .any(|e| matches!(e.verdict, EvaluationVerdict::Fail))
    }

    pub fn all_evidence(&self) -> Vec<&Evidence> {
        self.evaluations
            .iter()
            .flat_map(|evaluation| evaluation.evidence.iter())
            .collect()
    }
}
