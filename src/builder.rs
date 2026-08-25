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

    // =========================================================
    // PRIMERA ITERACIÓN
    // =========================================================

    if state.iteration == 1 {
        println!("BUILDER: generando primera versión...");

        // Código deliberadamente incorrecto.
        // Falta cerrar correctamente el println!.
        let code = String::from(
            r#"fn main() {
    println!("API REST generada"
}
"#,
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

        // Código Rust válido.
        //
        // También contiene los elementos que el Validator
        // utiliza para verificar la intención de API REST.
        let corrected_code = String::from(
            r#"fn main() {
    println!("API REST generada");

    // HTTP Server
    // GET /api
    // endpoint: GET /api
}
"#,
        );

        state.code = Some(corrected_code);

        println!("BUILDER: código corregido generado");

        return;
    }

    // =========================================================
    // FALLBACK
    // =========================================================

    println!("BUILDER: generando versión estándar...");

    let code = String::from(
        r#"fn main() {
    println!("API REST generada");

    // HTTP Server
    // GET /api
    // endpoint: GET /api
}
"#,
    );

    state.code = Some(code);

    println!("BUILDER: código generado");
}
