mod builder;
mod compiler;
mod planner;
mod repairer;
mod state;
mod validator;

use state::CodeState;

fn main() {
    let mut state = CodeState {
        request: std::env::args().skip(1).collect::<Vec<String>>().join(" "),

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
        println!("{}", plan);
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
}
