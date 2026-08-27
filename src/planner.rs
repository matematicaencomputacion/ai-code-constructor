use crate::state::CodeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Api,
    Calculator,
    Authentication,
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub kind: PlanKind,
    pub steps: Vec<String>,
}

/// Clasifica la estrategia de construcción a partir de texto (goal/requirements).
pub fn classify_plan_kind(text: &str) -> PlanKind {
    let request = text.to_lowercase();

    if request.contains("api") || contains_word(&request, "rest") {
        PlanKind::Api
    } else if request.contains("calculadora") || contains_word(&request, "calculator") {
        PlanKind::Calculator
    } else if request.contains("login")
        || request.contains("autenticación")
        || contains_word(&request, "authentication")
    {
        PlanKind::Authentication
    } else {
        PlanKind::Generic
    }
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == needle)
}

/// Steps base deterministas para un [`PlanKind`] (HOW, no WHAT).
pub fn build_steps_for_kind(kind: PlanKind) -> Vec<String> {
    match kind {
        PlanKind::Api => vec![
            "Crear servidor HTTP".to_string(),
            "Definir endpoints".to_string(),
            "Implementar handlers".to_string(),
            "Agregar tests".to_string(),
        ],
        PlanKind::Calculator => vec![
            "Definir las operaciones matemáticas".to_string(),
            "Implementar las funciones de cálculo".to_string(),
            "Implementar la interfaz de entrada".to_string(),
            "Agregar tests".to_string(),
        ],
        PlanKind::Authentication => vec![
            "Definir el modelo de autenticación".to_string(),
            "Implementar validación de credenciales".to_string(),
            "Implementar login".to_string(),
            "Agregar tests".to_string(),
        ],
        PlanKind::Generic => vec![
            "Analizar los requisitos".to_string(),
            "Diseñar la solución".to_string(),
            "Implementar la funcionalidad principal".to_string(),
            "Agregar tests".to_string(),
        ],
    }
}

/// Construye un [`BuildPlan`] a partir del goal (API de compatibilidad / delegación).
pub fn plan_from_goal(goal: &str) -> BuildPlan {
    let kind = classify_plan_kind(goal);
    BuildPlan {
        kind,
        steps: build_steps_for_kind(kind),
    }
}

/// Genera un plan de construcción a partir del pedido original en [`CodeState`].
///
/// API preservada del Constructor: delega internamente en [`plan_from_goal`].
pub fn plan(state: &mut CodeState) {
    let build_plan = plan_from_goal(&state.request);

    println!("PLANNER: plan generado:");

    for (index, step) in build_plan.steps.iter().enumerate() {
        println!("{}. {}", index + 1, step);
    }

    state.plan = Some(build_plan);
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

    #[test]
    fn planner_classifies_rest_without_matching_resta_substring() {
        assert_eq!(
            classify_plan_kind("Debe soportar suma y resta"),
            PlanKind::Generic
        );
        assert_eq!(classify_plan_kind("Crear una API REST"), PlanKind::Api);
    }

    #[test]
    fn plan_from_goal_matches_legacy_plan_behavior() {
        let mut state = state_with_request("Crear una API REST");
        plan(&mut state);
        let legacy = state.plan.expect("plan");

        let direct = plan_from_goal("Crear una API REST");
        assert_eq!(legacy, direct);
    }
}
