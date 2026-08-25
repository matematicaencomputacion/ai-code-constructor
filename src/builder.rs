use crate::state::CodeState;

/// Genera código a partir del estado actual.
///
/// La primera iteración genera deliberadamente código
/// defectuoso para probar el ciclo de compilación y reparación.
///
/// Las siguientes iteraciones utilizan el feedback generado
/// por el Repairer para producir una versión corregida.
pub fn build(state: &mut CodeState) {
    println!("BUILDER: generando código...");
    println!("BUILDER: request -> {}", state.request);

    if let Some(plan) = &state.plan {
        println!("BUILDER: utilizando plan -> {}", plan);
    }

    // =========================================================
    // PRIMERA ITERACIÓN
    // =========================================================

    if state.iteration == 1 {
        println!("BUILDER: generando primera versión...");

        // Generamos deliberadamente código incorrecto.
        // La diferencia es que ahora el código incorpora
        // información real del request.
        let code = format!(
            r#"fn main() {{
    println!("Request: {}"
}}
"#,
            state.request
        );

        state.code = Some(code);

        println!("BUILDER: código generado");
        return;
    }

    // =========================================================
    // ITERACIONES DE CORRECCIÓN
    // =========================================================

    if !state.feedback.is_empty() {
        println!("BUILDER: analizando feedback anterior...");

        for feedback in &state.feedback {
            println!("BUILDER: feedback recibido -> {}", feedback);
        }

        println!("BUILDER: generando versión corregida...");

        let plan = state.plan.as_deref().unwrap_or("Sin plan disponible");

        let corrected_code = format!(
            r#"fn main() {{
    println!("Request: {}");

    // Plan utilizado:
    // {}

    // HTTP Server
    // GET /api
    // endpoint: GET /api
}}
"#,
            state.request,
            plan.replace('\n', "\n// ")
        );

        state.code = Some(corrected_code);

        println!("BUILDER: código corregido generado");
        return;
    }

    // =========================================================
    // FALLBACK
    // =========================================================

    println!("BUILDER: generando versión estándar...");

    let code = format!(
        r#"fn main() {{
    println!("Request: {}");

    // HTTP Server
    // GET /api
    // endpoint: GET /api
}}
"#,
        state.request
    );

    state.code = Some(code);

    println!("BUILDER: código generado");
}
