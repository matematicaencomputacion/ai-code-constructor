use crate::state::CodeState;

/// Valida el código generado según el pedido original.
pub fn validate(state: &mut CodeState) {
    // Limpiamos los errores anteriores del Validator.
    state.errors.clear();

    let code = match &state.code {
        Some(code) => code,
        None => {
            state.errors.push("No se generó ningún código.".to_string());

            println!(
                "VALIDATOR: código inválido. {} error(es) encontrado(s)",
                state.errors.len()
            );

            return;
        }
    };

    // ---------------------------------------------------------
    // VALIDACIÓN BÁSICA
    // ---------------------------------------------------------

    if code.trim().is_empty() {
        state
            .errors
            .push("El código generado está vacío.".to_string());
    }

    // ---------------------------------------------------------
    // VALIDACIÓN DE API REST
    // ---------------------------------------------------------

    if state.request.contains("API REST") {
        let has_server =
            code.contains("HTTP") || code.contains("Server") || code.contains("server");

        let has_endpoint = code.contains("GET")
            || code.contains("POST")
            || code.contains("endpoint")
            || code.contains("/api");

        if !has_server || !has_endpoint {
            state
                .errors
                .push("El código no contiene la implementación esperada de API REST".to_string());
        }
    }

    // ---------------------------------------------------------
    // RESULTADO
    // ---------------------------------------------------------

    if state.errors.is_empty() {
        println!("VALIDATOR: código válido");
    } else {
        println!(
            "VALIDATOR: código inválido. {} error(es) encontrado(s)",
            state.errors.len()
        );

        for error in &state.errors {
            println!("  - {}", error);
        }
    }
}
