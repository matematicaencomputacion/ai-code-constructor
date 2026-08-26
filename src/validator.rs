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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CodeState;

    fn state_with_code(request: &str, code: Option<&str>) -> CodeState {
        CodeState {
            request: request.to_string(),
            plan: None,
            code: code.map(str::to_string),
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        }
    }

    #[test]
    fn validator_accepts_non_empty_code_for_generic_request() {
        let mut state = state_with_code(
            "Crear una calculadora",
            Some("fn main() {\n    println!(\"ok\");\n}\n"),
        );

        validate(&mut state);

        assert!(state.errors.is_empty());
    }

    #[test]
    fn validator_rejects_when_code_is_none() {
        let mut state = state_with_code("Crear una calculadora", None);

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert_eq!(state.errors[0], "No se generó ningún código.");
    }

    #[test]
    fn validator_rejects_empty_code() {
        let mut state = state_with_code("Crear una calculadora", Some("   \n\t  "));

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert_eq!(state.errors[0], "El código generado está vacío.");
    }

    #[test]
    fn validator_clears_previous_errors_before_validating() {
        let mut state = state_with_code(
            "Crear una calculadora",
            Some("fn main() {\n    println!(\"ok\");\n}\n"),
        );
        state.errors.push("error previo".to_string());

        validate(&mut state);

        assert!(state.errors.is_empty());
    }

    #[test]
    fn validator_accepts_api_rest_code_with_server_and_endpoint() {
        let mut state = state_with_code(
            "Crear una API REST",
            Some(
                r#"fn main() {
    println!("Servidor HTTP");
    println!("endpoint /api");
}
"#,
            ),
        );

        validate(&mut state);

        assert!(state.errors.is_empty());
    }

    #[test]
    fn validator_rejects_api_rest_code_without_expected_implementation() {
        let mut state = state_with_code(
            "Crear una API REST",
            Some("fn main() {\n    println!(\"hola\");\n}\n"),
        );

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert_eq!(
            state.errors[0],
            "El código no contiene la implementación esperada de API REST"
        );
    }

    #[test]
    fn validator_does_not_apply_api_rest_rules_when_request_lacks_exact_phrase() {
        let mut state = state_with_code(
            "Crear una api rest",
            Some("fn main() {\n    println!(\"hola\");\n}\n"),
        );

        validate(&mut state);

        assert!(state.errors.is_empty());
    }

    #[test]
    fn validator_reports_empty_and_api_rest_errors_together() {
        let mut state = state_with_code("Crear una API REST", Some(""));

        validate(&mut state);

        assert_eq!(state.errors.len(), 2);
        assert_eq!(state.errors[0], "El código generado está vacío.");
        assert_eq!(
            state.errors[1],
            "El código no contiene la implementación esperada de API REST"
        );
    }
}
