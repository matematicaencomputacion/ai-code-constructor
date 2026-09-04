mod builder;
#[cfg(test)]
mod builder_multi_file_tests;
mod compiler;
#[allow(dead_code)] // capa nueva; aún no cableada al ciclo Constructor
mod harness;
mod planner;
mod repairer;
mod state;
mod validator;

use state::CodeState;

/// Máximo de intentos del ciclo autónomo Builder → Compiler → Validator → Repairer.
const MAX_ITERATIONS: u32 = 3;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("model-compatibility-probe") {
        match harness::run_model_compatibility_probe_cli(args.iter().skip(1).cloned()) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("model-compatibility-probe: {error}");
                eprintln!("{}", harness::probe_cli_usage());
                std::process::exit(1);
            }
        }
        return;
    }

    if args.first().map(String::as_str) == Some("live-repair-smoke") {
        match harness::run_live_repair_smoke_harness() {
            Ok(harness::LiveRepairSmokeOutcome::BlockedWithInstructions) => {}
            Ok(harness::LiveRepairSmokeOutcome::LiveSessionCompleted(_)) => {}
            Err(error) => {
                eprintln!("live-repair-smoke: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    if args.first().map(String::as_str) == Some("export") {
        match harness::run_export_cli(args.iter().skip(1).cloned()) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("export: {error}");
                eprintln!("{}", harness::export_cli_usage());
                std::process::exit(1);
            }
        }
        return;
    }

    let request = args.join(" ");
    let _state = run_constructor(&request);
}

/// Ejecuta el ciclo autónomo completo del Constructor y devuelve el estado final.
fn run_constructor(request: &str) -> CodeState {
    run_constructor_with_limit(request, MAX_ITERATIONS)
}

/// Igual que [`run_constructor`], pero permite fijar el tope de iteraciones
/// (útil para tests del límite sin alterar los módulos de construcción).
fn run_constructor_with_limit(request: &str, max_iterations: u32) -> CodeState {
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

                if reached_iteration_limit(&mut state, max_iterations) {
                    break;
                }
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

                if reached_iteration_limit(&mut state, max_iterations) {
                    break;
                }
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

/// Si se agotaron los intentos sin aprobación, deja evidencia en `state.errors`.
fn reached_iteration_limit(state: &mut CodeState, max_iterations: u32) -> bool {
    if state.iteration < max_iterations {
        return false;
    }

    println!("\nCONSTRUCTOR: límite de iteraciones alcanzado.");
    state.errors.push(format!(
        "Límite de iteraciones alcanzado (máximo: {}).",
        max_iterations
    ));
    true
}

#[cfg(test)]
fn hit_iteration_limit(state: &CodeState) -> bool {
    state
        .errors
        .iter()
        .any(|error| error.contains("Límite de iteraciones alcanzado"))
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

        let plan = state
            .plan
            .as_ref()
            .expect("El ciclo debe conservar el plan");
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

        let code = state
            .code
            .as_ref()
            .expect("El ciclo debe dejar código generado");
        // Defecto iteración 1: println sin `);`. Tras feedback debe estar cerrado.
        let corrected = format!(r#"println!("Request: {request}");"#);
        let defective = format!(r#"println!("Request: {request}""#) + "\n";
        assert!(
            code.contains(&corrected),
            "El código final debe corregir el println del request"
        );
        assert!(
            !code.contains(&defective),
            "No debe quedar el println defectuoso sin cerrar"
        );
        assert!(code.contains("crear_servidor"));
        assert!(code.contains("definir_endpoints") || code.contains("endpoint"));
        assert!(code.contains("HTTP") || code.contains("Servidor"));

        assert!(
            state.errors.is_empty(),
            "Al aprobar no deben quedar errores: {:?}",
            state.errors
        );
        assert!(
            !hit_iteration_limit(&state),
            "Una construcción aprobada no debe reportar límite de iteraciones"
        );
        assert!(state.iteration < MAX_ITERATIONS);
    }

    #[test]
    fn constructor_calculator_cycle_approves_valid_code() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear una calculadora";
        let state = run_constructor(request);

        assert_eq!(state.request, request);

        let plan = state
            .plan
            .as_ref()
            .expect("El ciclo debe conservar el plan");
        assert_eq!(plan.kind, PlanKind::Calculator);

        assert!(
            state.iteration >= 2,
            "Debe atravesar al menos una reparación antes de aprobar"
        );
        assert!(!state.feedback.is_empty());

        let code = state
            .code
            .as_ref()
            .expect("El ciclo debe dejar código generado");
        assert!(code.contains("sumar"));
        assert!(code.contains("a + b"));

        assert!(
            state.errors.is_empty(),
            "No deben quedar errores finales: {:?}",
            state.errors
        );
        assert!(!hit_iteration_limit(&state));
    }

    #[test]
    fn constructor_authentication_cycle_approves_valid_code() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear un sistema de autenticación";
        let state = run_constructor(request);

        assert_eq!(state.request, request);

        let plan = state
            .plan
            .as_ref()
            .expect("El ciclo debe conservar el plan");
        assert_eq!(plan.kind, PlanKind::Authentication);

        assert!(
            state.iteration >= 2,
            "Debe atravesar al menos una reparación antes de aprobar"
        );
        assert!(!state.feedback.is_empty());

        let code = state
            .code
            .as_ref()
            .expect("El ciclo debe dejar código generado");
        assert!(code.contains("validar_credenciales"));
        assert!(code.contains("Login correcto") || code.contains("Login incorrecto"));

        assert!(
            state.errors.is_empty(),
            "No deben quedar errores finales: {:?}",
            state.errors
        );
        assert!(!hit_iteration_limit(&state));
    }

    #[test]
    fn constructor_stops_at_iteration_limit_without_approval() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Con tope 1 solo corre la versión defectuosa deliberada; no puede aprobar.
        let max_iterations = 1;
        let request = "Crear una calculadora";
        let state = run_constructor_with_limit(request, max_iterations);

        assert_eq!(state.iteration, max_iterations);
        assert!(
            state.iteration <= MAX_ITERATIONS,
            "El ciclo no debe superar MAX_ITERATIONS"
        );
        assert!(
            hit_iteration_limit(&state),
            "Debe quedar evidencia explícita del límite en state.errors: {:?}",
            state.errors
        );
        assert!(
            state
                .errors
                .iter()
                .any(|e| e.contains("Error de compilación")),
            "Deben conservarse los errores de la última iteración fallida"
        );
        assert!(
            !state.feedback.is_empty(),
            "El Repairer debe haber generado feedback antes del corte"
        );

        let code = state
            .code
            .as_ref()
            .expect("Debe conservarse el código generado");
        assert!(code.contains(&format!("Request: {request}")));

        let plan = state.plan.as_ref().expect("Debe conservarse el plan");
        assert_eq!(plan.kind, PlanKind::Calculator);
        assert_eq!(state.request, request);
    }

    #[test]
    fn constructor_never_exceeds_configured_iteration_limit() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let max_iterations = 2;
        let state = run_constructor_with_limit("Crear una API REST", max_iterations);

        // Con tope 2 el flujo actual aprueba exactamente en la segunda iteración.
        assert!(state.iteration <= max_iterations);
        assert!(state.errors.is_empty());
        assert!(!hit_iteration_limit(&state));
        assert_eq!(state.plan.as_ref().map(|p| p.kind), Some(PlanKind::Api));
    }

    #[test]
    fn constructor_preserves_state_fields_when_hitting_iteration_limit() {
        let _guard = CONSTRUCTOR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear un sistema de autenticación";
        let state = run_constructor_with_limit(request, 1);

        assert_eq!(state.request, request);
        assert!(state.plan.is_some());
        assert!(state.code.is_some());
        assert!(!state.errors.is_empty());
        assert!(!state.feedback.is_empty());
        assert_eq!(state.iteration, 1);
        assert!(hit_iteration_limit(&state));
        assert_eq!(
            state.plan.as_ref().map(|p| p.kind),
            Some(PlanKind::Authentication)
        );
    }
}
