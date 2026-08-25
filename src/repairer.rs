use crate::state::CodeState;

/// Analiza los errores encontrados durante la construcción
/// y genera instrucciones de reparación para el Builder.
pub fn repair(state: &mut CodeState) {
    if state.errors.is_empty() {
        println!("REPAIRER: no hay errores para analizar.");
        return;
    }

    println!("REPAIRER: analizando errores...");

    let mut feedback = Vec::new();

    for error in &state.errors {
        if error.contains("mismatched closing delimiter") {
            feedback.push(
                "Revisar los delimitadores del código generado. \
                 Verificar que todas las llaves { } y paréntesis ( ) \
                 estén correctamente balanceados."
                    .to_string(),
            );
        } else if error.contains("unclosed delimiter") {
            feedback.push(
                "Existe un delimitador sin cerrar. \
                 Revisar llaves, paréntesis y corchetes."
                    .to_string(),
            );
        } else if error.contains("expected item") {
            feedback.push(
                "El código contiene texto fuera de una estructura Rust válida. \
                 Revisar que el código generado contenga únicamente \
                 elementos válidos de Rust."
                    .to_string(),
            );
        } else if error.contains("Error de compilación") {
            feedback.push(format!(
                "Analizar el siguiente error de compilación y corregirlo: {}",
                error
            ));
        } else {
            feedback.push(format!(
                "Revisar y corregir el siguiente error: {}",
                error
            ));
        }
    }

    println!("REPAIRER: diagnóstico generado:");

    for item in &feedback {
        println!("  - {}", item);
    }

    // Reemplazamos los errores originales por instrucciones
    // de reparación más útiles para el Builder.
    state.errors = feedback;
}