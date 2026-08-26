use crate::state::CodeState;

#[derive(Debug, Clone)]
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
