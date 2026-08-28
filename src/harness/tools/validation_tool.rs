use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::VALIDATE;
use crate::planner::{BuildPlan, PlanKind};
use crate::state::CodeState;
use crate::validator;

/// Separador interno para serializar entradas de validación.
const SEP: &str = "\n<<<ACC_SEP>>>\n";

/// Codifica los campos de [`crate::harness::AgentAction::Validate`] para ValidationTool.
pub fn encode_validate_input(request: &str, code: Option<&str>, plan_kind: &str) -> String {
    let code_field = code.unwrap_or("");
    format!("{plan_kind}{SEP}{request}{SEP}{code_field}")
}

fn decode_validate_input(input: &str) -> Result<(PlanKind, String, Option<String>), String> {
    let parts: Vec<&str> = input.splitn(3, SEP).collect();
    if parts.len() != 3 {
        return Err(
            "entrada de validate inválida: se esperaba plan_kind, request y code".to_string(),
        );
    }

    let plan_kind = parse_plan_kind(parts[0])?;
    let request = parts[1].to_string();
    let code = if parts[2].is_empty() {
        None
    } else {
        Some(parts[2].to_string())
    };

    Ok((plan_kind, request, code))
}

fn parse_plan_kind(raw: &str) -> Result<PlanKind, String> {
    match raw.trim() {
        "Api" => Ok(PlanKind::Api),
        "Calculator" => Ok(PlanKind::Calculator),
        "Authentication" => Ok(PlanKind::Authentication),
        "Generic" => Ok(PlanKind::Generic),
        other => Err(format!("PlanKind desconocido: {other}")),
    }
}

/// Adaptador exclusivo de [`validator::validate`].
///
/// Solo valida y expone errors en Evidence. No ejecuta Repairer ni genera feedback.
pub struct ValidationTool;

impl Tool for ValidationTool {
    fn name(&self) -> &str {
        VALIDATE
    }

    fn execute(&self, input: &str, ctx: &AgentContext) -> ToolResult {
        let (plan_kind, request, code) = match decode_validate_input(input) {
            Ok(parsed) => parsed,
            Err(error) => {
                return ToolResult::failure(
                    error.clone(),
                    vec![
                        Evidence::new("tool", VALIDATE),
                        Evidence::new("parse_error", error),
                    ],
                );
            }
        };

        let code = match code {
            Some(value) if !value.is_empty() => Some(value),
            _ => ctx.working_code().map(str::to_string),
        };

        let mut state = CodeState {
            request,
            plan: Some(BuildPlan {
                kind: plan_kind,
                steps: vec!["harness-validate".to_string()],
            }),
            code,
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        };

        validator::validate(&mut state);

        let success = state.errors.is_empty();
        let output = if success {
            "validación exitosa".to_string()
        } else {
            state.errors.join(" | ")
        };

        let mut evidence = vec![
            Evidence::new("tool", VALIDATE),
            Evidence::new("validate_status", if success { "ok" } else { "error" }),
            Evidence::new("error_count", state.errors.len().to_string()),
        ];

        for (index, error) in state.errors.iter().enumerate() {
            evidence.push(Evidence::new(format!("validator_error_{index}"), error));
        }
        ctx.append_artifact_evidence(&mut evidence);

        if success {
            ToolResult::success(output, evidence)
        } else {
            ToolResult::failure(output, evidence)
        }
    }
}
