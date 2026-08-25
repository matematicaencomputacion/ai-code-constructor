use crate::state::CodeState;

/// Genera un plan de construcción a partir del pedido original.
pub fn plan(state: &mut CodeState) {
    let request = state.request.to_lowercase();

    let plan = if request.contains("api") || request.contains("rest") {
        "1. Crear servidor HTTP
2. Definir endpoints
3. Implementar handlers
4. Agregar tests"
    } else if request.contains("calculadora") || request.contains("calculator") {
        "1. Definir las operaciones matemáticas
2. Implementar las funciones de cálculo
3. Implementar la interfaz de entrada
4. Agregar tests"
    } else if request.contains("login")
        || request.contains("autenticación")
        || request.contains("authentication")
    {
        "1. Definir el modelo de autenticación
2. Implementar validación de credenciales
3. Implementar login
4. Agregar tests"
    } else {
        "1. Analizar los requisitos
2. Diseñar la solución
3. Implementar la funcionalidad principal
4. Agregar tests"
    };

    state.plan = Some(plan.to_string());

    println!("PLANNER: plan generado:");
    println!("{}", plan);
}
