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
