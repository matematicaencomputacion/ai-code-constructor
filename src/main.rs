mod builder;
mod compiler;
mod planner;
mod repairer;
mod state;
mod validator;

use state::CodeState;

fn main() {
    let request = std::env::args().skip(1).collect::<Vec<String>>().join(" ");
    let _state = run_constructor(&request);
}

/// Ejecuta el ciclo autónomo completo del Constructor y devuelve el estado final.
fn run_constructor(request: &str) -> CodeState {
    let mut state = CodeState {
        request: request.to_string(),

        plan: None,
        code: None,
        errors: Vec::new(),
        feedback: Vec::new(),
        iteration: 0,
    };

    // =========================================================
    // 1. PLANNER
    // =========================================================

    println!("=== AI-CODE-CONSTRUCTOR ===");
    println!("REQUEST: {}", state.request);

    println!("\nPLANNER: creando plan...");

    planner::plan(&mut state);

    if let Some(plan) = &state.plan {
        println!("PLANNER: plan generado:");
        println!("Tipo: {:?}", plan.kind);

        for (index, step) in plan.steps.iter().enumerate() {
            println!("{}. {}", index + 1, step);
        }
    }

    // =========================================================
    // 2. CICLO AUTÓNOMO
    // =========================================================

    loop {
        state.iteration += 1;

        println!(
            "\n================ ITERACIÓN {} ================",
            state.iteration
        );

        // -----------------------------------------------------
        // LÍMITE DE SEGURIDAD
        // -----------------------------------------------------

        if state.iteration > 6 {
            println!("\nCONSTRUCTOR: límite de iteraciones alcanzado.");
            break;
        }

        // -----------------------------------------------------
        // IMPORTANTE:
        //
        // NO limpiamos state.errors aquí.
        //
        // El Builder necesita recibir el feedback generado
        // por el Repairer de la iteración anterior.
        // -----------------------------------------------------

        // -----------------------------------------------------
        // BUILDER
        // -----------------------------------------------------

        builder::build(&mut state);

        // -----------------------------------------------------
        // COMPILER
        // -----------------------------------------------------

        let compile_result = match &state.code {
            Some(code) => compiler::compile(code),

            None => Err("El Builder no generó ningún código.".to_string()),
        };

        match compile_result {
            // =================================================
            // CÓDIGO COMPILADO
            // =================================================
            Ok(_) => {
                println!("COMPILER: código compilado correctamente");

                // -------------------------------------------------
                // VALIDATOR
                // -------------------------------------------------

                validator::validate(&mut state);

                // -------------------------------------------------
                // ¿VALIDACIÓN CORRECTA?
                // -------------------------------------------------

                if state.errors.is_empty() {
                    println!("\nCONSTRUCTOR: código aprobado.");

                    break;
                }

                // -------------------------------------------------
                // VALIDACIÓN FALLIDA
                // -------------------------------------------------

                println!(
                    "CONSTRUCTOR: se encontraron {} error(es).",
                    state.errors.len()
                );

                // -------------------------------------------------
                // REPAIRER
                // -------------------------------------------------

                repairer::repair(&mut state);

                println!("CONSTRUCTOR: intentando corregir...");
            }

            // =================================================
            // ERROR DE COMPILACIÓN
            // =================================================
            Err(error) => {
                println!("COMPILER: error de compilación:");

                println!("{}", error);

                // -------------------------------------------------
                // GUARDAMOS EL ERROR EN EL ESTADO
                // -------------------------------------------------

                state
                    .errors
                    .push(format!("Error de compilación: {}", error.trim()));

                println!("CONSTRUCTOR: se encontró un error de compilación.");

                // -------------------------------------------------
                // REPAIRER
                // -------------------------------------------------

                repairer::repair(&mut state);

                println!("CONSTRUCTOR: intentando corregir...");
            }
        }
    }

    // =========================================================
    // 3. ESTADO FINAL
    // =========================================================

    println!("\n================ ESTADO FINAL ================");

    println!("{:#?}", state);

    println!("\n================================================");

    state
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use planner::PlanKind;
    use std::sync::Mutex;

    /// El Compiler escribe en rutas fijas bajo /tmp; serializamos estos tests
    /// para evitar carreras entre ejecuciones paralelas de rustc.
    static CONSTRUCTOR_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn constructor_api_rest_repairs_defective_code_and_approves() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear una API REST";
        let state = run_constructor(request);

        assert_eq!(state.request, request);

        let plan = state.plan.expect("El ciclo debe conservar el plan");
        assert_eq!(plan.kind, PlanKind::Api);

        // El ciclo debió pasar por reparación: iteración 1 falla, iteración 2 aprueba.
        assert_eq!(
            state.iteration, 2,
            "El ciclo debe reparar en la primera iteración y aprobar en la segunda"
        );
        assert!(
            !state.feedback.is_empty(),
            "Debe existir feedback del Repairer tras el código defectuoso"
        );
        assert!(
            state.feedback.iter().any(|f| f.contains("delimitadores")),
            "El feedback debe reflejar el error de delimitadores de la primera versión"
        );

        let code = state.code.expect("El ciclo debe dejar código generado");
        assert!(
            !code.contains(&format!("Request: {request}")),
            "El código final no debe ser la versión defectuosa de la primera iteración"
        );
        assert!(code.contains("crear_servidor"));
        assert!(code.contains("definir_endpoints") || code.contains("endpoint"));
        assert!(code.contains("HTTP") || code.contains("Servidor"));

        assert!(
            state.errors.is_empty(),
            "Al aprobar no deben quedar errores: {:?}",
            state.errors
        );
    }

    #[test]
    fn constructor_calculator_cycle_approves_valid_code() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear una calculadora";
        let state = run_constructor(request);

        assert_eq!(state.request, request);

        let plan = state.plan.expect("El ciclo debe conservar el plan");
        assert_eq!(plan.kind, PlanKind::Calculator);

        assert!(
            state.iteration >= 2,
            "Debe atravesar al menos una reparación antes de aprobar"
        );
        assert!(!state.feedback.is_empty());

        let code = state.code.expect("El ciclo debe dejar código generado");
        assert!(code.contains("sumar"));
        assert!(code.contains("a + b"));

        assert!(
            state.errors.is_empty(),
            "No deben quedar errores finales: {:?}",
            state.errors
        );
    }

    #[test]
    fn constructor_authentication_cycle_approves_valid_code() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear un sistema de autenticación";
        let state = run_constructor(request);

        assert_eq!(state.request, request);

        let plan = state.plan.expect("El ciclo debe conservar el plan");
        assert_eq!(plan.kind, PlanKind::Authentication);

        assert!(
            state.iteration >= 2,
            "Debe atravesar al menos una reparación antes de aprobar"
        );
        assert!(!state.feedback.is_empty());

        let code = state.code.expect("El ciclo debe dejar código generado");
        assert!(code.contains("validar_credenciales"));
        assert!(code.contains("Login correcto") || code.contains("Login incorrecto"));

        assert!(
            state.errors.is_empty(),
            "No deben quedar errores finales: {:?}",
            state.errors
        );
    }
}
