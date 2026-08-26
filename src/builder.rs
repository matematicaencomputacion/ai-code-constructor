use crate::state::CodeState;

/// Genera código a partir del estado actual.
///
/// La primera iteración genera deliberadamente código
/// defectuoso para probar el ciclo de compilación y reparación.
///
/// Las siguientes iteraciones utilizan el feedback generado
/// por el Repairer y el plan generado por el Planner.
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
        // El request sigue siendo información real del estado.
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

        let plan_comments = plan
            .lines()
            .map(|line| format!("// {}", line))
            .collect::<Vec<String>>()
            .join("\n");

        let corrected_code = format!(
            r#"fn main() {{
    println!("Request: {}");

    // Plan utilizado:
    {}

    // Implementación basada en el plan.
}}
"#,
            state.request, plan_comments
        );

        state.code = Some(corrected_code);

        println!("BUILDER: código corregido generado");
        return;
    }

    // =========================================================
    // FALLBACK
    // =========================================================

    println!("BUILDER: generando versión estándar...");

    let plan = state.plan.as_deref().unwrap_or("Sin plan disponible");

    let plan_comments = plan
        .lines()
        .map(|line| format!("// {}", line))
        .collect::<Vec<String>>()
        .join("\n");

    let code = format!(
        r#"fn main() {{
    println!("Request: {}");

    // Plan utilizado:
    {}

    // Implementación basada en el plan.
}}
"#,
        state.request, plan_comments
    );

    state.code = Some(code);

    println!("BUILDER: código generado");
}
