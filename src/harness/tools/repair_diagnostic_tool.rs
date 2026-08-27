use crate::harness::context::AgentContext;
use crate::harness::evaluation::Evidence;
use crate::harness::tool::{Tool, ToolResult};
use crate::harness::tools::REPAIR_DIAGNOSTIC;
use crate::planner::{BuildPlan, PlanKind};
use crate::repairer;
use crate::state::CodeState;

/// Separador interno para serializar la lista de errores.
const ERROR_SEP: &str = "\n<<<ACC_ERR>>>\n";

/// Codifica errores para [`crate::harness::AgentAction::RepairDiagnostic`].
pub fn encode_repair_diagnostic_input(errors: &[String]) -> String {
    errors.join(ERROR_SEP)
}

fn decode_repair_diagnostic_input(input: &str) -> Vec<String> {
    if input.trim().is_empty() {
        return Vec::new();
    }
    input.split(ERROR_SEP).map(str::to_string).collect()
}

/// Adaptador exclusivo de [`repairer::repair`]: errors → feedback diagnóstico.
///
/// No ejecuta Validator ni Compiler. No modifica `code`, `plan`, `request` ni `iteration`.
pub struct RepairDiagnosticTool;

impl Tool for RepairDiagnosticTool {
    fn name(&self) -> &str {
        REPAIR_DIAGNOSTIC
    }

    fn execute(&self, input: &str, _ctx: &AgentContext) -> ToolResult {
        let errors = decode_repair_diagnostic_input(input);

        if errors.is_empty() {
            return ToolResult {
                success: false,
                output: "no hay errores para diagnosticar".to_string(),
                evidence: vec![
                    Evidence::new("tool", REPAIR_DIAGNOSTIC),
                    Evidence::new("diagnostic_status", "error"),
                    Evidence::new("feedback_count", "0"),
                ],
            };
        }

        let request = "harness-repair-diagnostic".to_string();
        let plan = Some(BuildPlan {
            kind: PlanKind::Generic,
            steps: vec!["diagnostic".to_string()],
        });
        let code = Some("fn main() {}".to_string());
        let iteration = 7_u32;

        let mut state = CodeState {
            request: request.clone(),
            plan: plan.clone(),
            code: code.clone(),
            errors,
            feedback: Vec::new(),
            iteration,
        };

        repairer::repair(&mut state);

        debug_assert_eq!(state.request, request);
        if let (Some(after), Some(before)) = (&state.plan, &plan) {
            debug_assert_eq!(after.kind, before.kind);
            debug_assert_eq!(after.steps, before.steps);
        } else {
            debug_assert_eq!(state.plan.is_some(), plan.is_some());
        }
        debug_assert_eq!(state.code, code);
        debug_assert_eq!(state.iteration, iteration);

        let success = !state.feedback.is_empty();
        let output = if success {
            state.feedback.join(" | ")
        } else {
            "Repairer no generó feedback".to_string()
        };

        let mut evidence = vec![
            Evidence::new("tool", REPAIR_DIAGNOSTIC),
            Evidence::new("diagnostic_status", if success { "ok" } else { "error" }),
            Evidence::new("feedback_count", state.feedback.len().to_string()),
            Evidence::new("input_error_count", state.errors.len().to_string()),
        ];

        for (index, feedback) in state.feedback.iter().enumerate() {
            evidence.push(Evidence::new(
                format!("repairer_feedback_{index}"),
                feedback,
            ));
        }

        ToolResult {
            success,
            output,
            evidence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::AgentContext;

    #[test]
    fn repair_diagnostic_tool_generates_feedback_without_mutating_fields() {
        let tool = RepairDiagnosticTool;
        let errors =
            vec!["El código no contiene la implementación esperada de API REST".to_string()];
        let input = encode_repair_diagnostic_input(&errors);
        let result = tool.execute(&input, &AgentContext::new("diag"));

        assert!(result.success);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.label.starts_with("repairer_feedback_"))
        );
        assert!(
            !result
                .evidence
                .iter()
                .any(|e| e.label.starts_with("validator_error_"))
        );
    }

    #[test]
    fn repair_diagnostic_tool_does_not_emit_validation_evidence() {
        let tool = RepairDiagnosticTool;
        let input = encode_repair_diagnostic_input(&["error de prueba".to_string()]);
        let result = tool.execute(&input, &AgentContext::new("diag"));

        assert!(!result.evidence.iter().any(|e| e.label == "validate_status"));
    }
}
