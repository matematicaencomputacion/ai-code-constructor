use crate::state::CodeState;

/// Analiza los errores encontrados durante la construcción
/// y genera instrucciones de reparación para el Builder.
pub fn repair(state: &mut CodeState) {
    if state.errors.is_empty() {
        println!("REPAIRER: no hay errores para analizar.");
        return;
    }

    println!("REPAIRER: analizando errores...");

    // Eliminamos el feedback anterior.
    state.feedback.clear();

    for error in &state.errors {
        if error.contains("mismatched closing delimiter") {
            state.feedback.push(
                "Revisar los delimitadores del código generado. \
                 Verificar que todas las llaves { } y paréntesis ( ) \
                 estén correctamente balanceados."
                    .to_string(),
            );
        } else if error.contains("unclosed delimiter") {
            state.feedback.push(
                "Existe un delimitador sin cerrar. \
                 Revisar llaves, paréntesis y corchetes."
                    .to_string(),
            );
        } else if error.contains("expected item") {
            state.feedback.push(
                "El código contiene texto fuera de una estructura Rust válida. \
                 Revisar que el código generado contenga únicamente \
                 elementos válidos de Rust."
                    .to_string(),
            );
        } else {
            state.feedback.push(format!(
                "Analizar y corregir el siguiente error de compilación: {}",
                error
            ));
        }
    }

    println!("REPAIRER: diagnóstico generado:");

    for feedback in &state.feedback {
        println!("  - {}", feedback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{BuildPlan, PlanKind};
    use crate::state::CodeState;

    fn state_with_errors(errors: Vec<&str>) -> CodeState {
        CodeState {
            request: "Crear una calculadora".to_string(),
            plan: Some(BuildPlan {
                kind: PlanKind::Calculator,
                steps: vec!["paso".to_string()],
            }),
            code: Some("fn main() {}".to_string()),
            errors: errors.into_iter().map(str::to_string).collect(),
            feedback: Vec::new(),
            iteration: 2,
        }
    }

    #[test]
    fn repairer_does_nothing_when_there_are_no_errors() {
        let mut state = state_with_errors(vec![]);
        state.feedback.push("feedback previo".to_string());

        repair(&mut state);

        assert!(state.errors.is_empty());
        // Sin errores, el Repairer retorna temprano y no limpia el feedback.
        assert_eq!(state.feedback, vec!["feedback previo".to_string()]);
    }

    #[test]
    fn repairer_generates_feedback_for_mismatched_closing_delimiter() {
        let mut state = state_with_errors(vec![
            "error: mismatched closing delimiter: expected `}` found `)`",
        ]);

        repair(&mut state);

        assert_eq!(state.feedback.len(), 1);
        assert!(state.feedback[0].contains("delimitadores"));
        assert!(state.feedback[0].contains("{ }"));
        assert!(state.feedback[0].contains("( )"));
    }

    #[test]
    fn repairer_generates_feedback_for_unclosed_delimiter() {
        let mut state = state_with_errors(vec!["error: this file contains an unclosed delimiter"]);

        repair(&mut state);

        assert_eq!(state.feedback.len(), 1);
        assert!(state.feedback[0].contains("delimitador sin cerrar"));
        assert!(state.feedback[0].contains("llaves"));
    }

    #[test]
    fn repairer_generates_feedback_for_expected_item() {
        let mut state = state_with_errors(vec!["error: expected item, found `hola`"]);

        repair(&mut state);

        assert_eq!(state.feedback.len(), 1);
        assert!(state.feedback[0].contains("estructura Rust válida"));
        assert!(state.feedback[0].contains("elementos válidos de Rust"));
    }

    #[test]
    fn repairer_generates_generic_feedback_for_other_compilation_errors() {
        let error = "Error de compilación: error: cannot find value `x` in this scope";
        let mut state = state_with_errors(vec![error]);

        repair(&mut state);

        assert_eq!(state.feedback.len(), 1);
        assert_eq!(
            state.feedback[0],
            format!(
                "Analizar y corregir el siguiente error de compilación: {}",
                error
            )
        );
    }

    #[test]
    fn repairer_treats_validation_errors_with_generic_compilation_feedback() {
        let error = "El código no contiene la implementación esperada de API REST";
        let mut state = state_with_errors(vec![error]);

        repair(&mut state);

        assert_eq!(state.feedback.len(), 1);
        assert_eq!(
            state.feedback[0],
            format!(
                "Analizar y corregir el siguiente error de compilación: {}",
                error
            )
        );
    }

    #[test]
    fn repairer_clears_previous_feedback_when_processing_errors() {
        let mut state = state_with_errors(vec!["error: unclosed delimiter"]);
        state.feedback.push("feedback obsoleto".to_string());

        repair(&mut state);

        assert_eq!(state.feedback.len(), 1);
        assert!(!state.feedback.iter().any(|f| f.contains("obsoleto")));
        assert!(state.feedback[0].contains("delimitador sin cerrar"));
    }

    #[test]
    fn repairer_generates_one_feedback_entry_per_error() {
        let mut state = state_with_errors(vec![
            "error: unclosed delimiter",
            "error: expected item, found `x`",
        ]);

        repair(&mut state);

        assert_eq!(state.errors.len(), 2);
        assert_eq!(state.feedback.len(), 2);
        assert!(state.feedback[0].contains("delimitador sin cerrar"));
        assert!(state.feedback[1].contains("estructura Rust válida"));
    }

    #[test]
    fn repairer_preserves_errors_request_plan_code_and_iteration() {
        let mut state = state_with_errors(vec!["error: unclosed delimiter"]);
        let request_before = state.request.clone();
        let plan_before = state.plan.clone();
        let code_before = state.code.clone();
        let errors_before = state.errors.clone();
        let iteration_before = state.iteration;

        repair(&mut state);

        assert_eq!(state.request, request_before);
        assert_eq!(
            state.plan.as_ref().map(|p| &p.kind),
            plan_before.as_ref().map(|p| &p.kind)
        );
        assert_eq!(state.code, code_before);
        assert_eq!(state.errors, errors_before);
        assert_eq!(state.iteration, iteration_before);
        assert!(!state.feedback.is_empty());
    }
}
