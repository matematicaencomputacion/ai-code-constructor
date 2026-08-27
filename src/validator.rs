use crate::planner::PlanKind;
use crate::state::CodeState;

/// Valida el código generado según el tipo de plan producido por el Planner.
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
    // VALIDACIÓN SEGÚN EL PLAN
    // ---------------------------------------------------------

    if let Some(plan) = &state.plan {
        match plan.kind {
            PlanKind::Api => {
                let has_server =
                    code.contains("HTTP") || code.contains("Server") || code.contains("server");

                let has_endpoint = code.contains("GET")
                    || code.contains("POST")
                    || code.contains("endpoint")
                    || code.contains("/api");

                if !has_server || !has_endpoint {
                    state.errors.push(
                        "El código no contiene la implementación esperada de API REST".to_string(),
                    );
                }
            }

            PlanKind::Calculator => {
                let has_calculation = code.contains("sumar") || code.contains("a + b");

                if !has_calculation {
                    state.errors.push(
                        "El código no contiene la implementación esperada de calculadora"
                            .to_string(),
                    );
                }
            }

            PlanKind::Authentication => {
                let has_credentials = code.contains("validar_credenciales");

                let has_login =
                    code.contains("Login correcto") || code.contains("Login incorrecto");

                if !has_credentials || !has_login {
                    state.errors.push(
                        "El código no contiene la implementación esperada de autenticación"
                            .to_string(),
                    );
                }
            }

            PlanKind::Generic => {
                // Para planes genéricos, por ahora alcanza con
                // verificar que exista código no vacío.
            }
        }
    } else {
        state
            .errors
            .push("No existe un plan para validar el código.".to_string());
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
    use crate::planner::{BuildPlan, PlanKind};
    use crate::state::CodeState;

    fn state_with_plan(kind: PlanKind, request: &str, code: Option<&str>) -> CodeState {
        CodeState {
            request: request.to_string(),
            plan: Some(BuildPlan {
                kind,
                steps: vec!["paso".to_string()],
            }),
            code: code.map(str::to_string),
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        }
    }

    #[test]
    fn validator_accepts_valid_api_plan() {
        let mut state = state_with_plan(
            PlanKind::Api,
            "cualquier pedido",
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
    fn validator_rejects_invalid_api_plan() {
        let mut state = state_with_plan(
            PlanKind::Api,
            "cualquier pedido",
            Some("fn main() { println!(\"hola\"); }"),
        );

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert!(state.errors[0].contains("API REST"));
    }

    #[test]
    fn validator_accepts_valid_calculator_plan() {
        let mut state = state_with_plan(
            PlanKind::Calculator,
            "cualquier pedido",
            Some(
                r#"fn main() {
    let resultado = sumar(2, 3);
    println!("{}", resultado);
}

fn sumar(a: i32, b: i32) -> i32 {
    a + b
}
"#,
            ),
        );

        validate(&mut state);

        assert!(state.errors.is_empty());
    }

    #[test]
    fn validator_rejects_invalid_calculator_plan() {
        let mut state = state_with_plan(
            PlanKind::Calculator,
            "cualquier pedido",
            Some("fn main() { println!(\"hola\"); }"),
        );

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert!(state.errors[0].contains("calculadora"));
    }

    #[test]
    fn validator_accepts_valid_authentication_plan() {
        let mut state = state_with_plan(
            PlanKind::Authentication,
            "cualquier pedido",
            Some(
                r#"fn main() {
    let usuario_valido = validar_credenciales("usuario", "password");

    if usuario_valido {
        println!("Login correcto");
    } else {
        println!("Login incorrecto");
    }
}

fn validar_credenciales(usuario: &str, password: &str) -> bool {
    !usuario.is_empty() && !password.is_empty()
}
"#,
            ),
        );

        validate(&mut state);

        assert!(state.errors.is_empty());
    }

    #[test]
    fn validator_rejects_invalid_authentication_plan() {
        let mut state = state_with_plan(
            PlanKind::Authentication,
            "cualquier pedido",
            Some("fn main() { println!(\"hola\"); }"),
        );

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert!(state.errors[0].contains("autenticación"));
    }

    #[test]
    fn validator_accepts_generic_plan_with_non_empty_code() {
        let mut state = state_with_plan(
            PlanKind::Generic,
            "cualquier pedido",
            Some("fn main() { println!(\"hola\"); }"),
        );

        validate(&mut state);

        assert!(state.errors.is_empty());
    }

    #[test]
    fn validator_rejects_missing_plan() {
        let mut state = CodeState {
            request: "Crear una API REST".to_string(),
            plan: None,
            code: Some("fn main() {}".to_string()),
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        };

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert_eq!(state.errors[0], "No existe un plan para validar el código.");
    }

    #[test]
    fn validator_rejects_missing_code_before_plan_validation() {
        let mut state = state_with_plan(PlanKind::Api, "cualquier pedido", None);

        validate(&mut state);

        assert_eq!(state.errors.len(), 1);
        assert_eq!(state.errors[0], "No se generó ningún código.");
    }

    #[test]
    fn validator_clears_previous_errors() {
        let mut state =
            state_with_plan(PlanKind::Generic, "cualquier pedido", Some("fn main() {}"));

        state.errors.push("error previo".to_string());

        validate(&mut state);

        assert!(state.errors.is_empty());
    }
}
