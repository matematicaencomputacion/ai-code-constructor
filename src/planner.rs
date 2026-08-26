use crate::state::CodeState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanKind {
    Api,
    Calculator,
    Authentication,
    Generic,
}

#[derive(Debug, Clone)]
pub struct BuildPlan {
    pub kind: PlanKind,
    pub steps: Vec<String>,
}

/// Genera un plan de construcción a partir del pedido original.
pub fn plan(state: &mut CodeState) {
    let request = state.request.to_lowercase();

    let (kind, steps) = if request.contains("api") || request.contains("rest") {
        (
            PlanKind::Api,
            vec![
                "Crear servidor HTTP".to_string(),
                "Definir endpoints".to_string(),
                "Implementar handlers".to_string(),
                "Agregar tests".to_string(),
            ],
        )
    } else if request.contains("calculadora") || request.contains("calculator") {
        (
            PlanKind::Calculator,
            vec![
                "Definir las operaciones matemáticas".to_string(),
                "Implementar las funciones de cálculo".to_string(),
                "Implementar la interfaz de entrada".to_string(),
                "Agregar tests".to_string(),
            ],
        )
    } else if request.contains("login")
        || request.contains("autenticación")
        || request.contains("authentication")
    {
        (
            PlanKind::Authentication,
            vec![
                "Definir el modelo de autenticación".to_string(),
                "Implementar validación de credenciales".to_string(),
                "Implementar login".to_string(),
                "Agregar tests".to_string(),
            ],
        )
    } else {
        (
            PlanKind::Generic,
            vec![
                "Analizar los requisitos".to_string(),
                "Diseñar la solución".to_string(),
                "Implementar la funcionalidad principal".to_string(),
                "Agregar tests".to_string(),
            ],
        )
    };

    let plan = BuildPlan { kind, steps };

    println!("PLANNER: plan generado:");

    for (index, step) in plan.steps.iter().enumerate() {
        println!("{}. {}", index + 1, step);
    }

    state.plan = Some(plan);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CodeState;

    fn state_with_request(request: &str) -> CodeState {
        CodeState {
            request: request.to_string(),
            plan: None,
            code: None,
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        }
    }

    #[test]
    fn planner_detects_api_rest() {
        let mut state = state_with_request("Crear una API REST");

        plan(&mut state);

        let build_plan = state.plan.expect("El Planner debe generar un plan");

        assert_eq!(build_plan.kind, PlanKind::Api);
        assert_eq!(build_plan.steps.len(), 4);
    }

    #[test]
    fn planner_detects_calculator() {
        let mut state = state_with_request("Crear una calculadora");

        plan(&mut state);

        let build_plan = state.plan.expect("El Planner debe generar un plan");

        assert_eq!(build_plan.kind, PlanKind::Calculator);
        assert_eq!(build_plan.steps.len(), 4);
    }

    #[test]
    fn planner_detects_authentication() {
        let mut state = state_with_request("Crear un sistema de autenticación");

        plan(&mut state);

        let build_plan = state.plan.expect("El Planner debe generar un plan");

        assert_eq!(build_plan.kind, PlanKind::Authentication);
        assert_eq!(build_plan.steps.len(), 4);
    }

    #[test]
    fn planner_uses_generic_plan_for_unknown_request() {
        let mut state = state_with_request("Crear una aplicación de inventario");

        plan(&mut state);

        let build_plan = state.plan.expect("El Planner debe generar un plan");

        assert_eq!(build_plan.kind, PlanKind::Generic);
        assert_eq!(build_plan.steps.len(), 4);
    }
}
