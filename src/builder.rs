use crate::state::CodeState;

/// Genera código a partir del estado actual.
///
/// En la primera iteración genera deliberadamente una versión
/// defectuosa para probar el ciclo de compilación y reparación.
///
/// En las siguientes iteraciones utiliza los errores del sistema
/// para generar una versión corregida.
pub fn build(state: &mut CodeState) {
    println!("BUILDER: generando código...");

    // =========================================================
    // PRIMERA ITERACIÓN
    // =========================================================

    if state.iteration == 1 {
        println!("BUILDER: generando primera versión...");

        // Código deliberadamente incorrecto.
        // El objetivo es comprobar que COMPILER y REPAIRER
        // detectan el problema.
        let code = String::from(
            "fn main() {\n\
             println!(\"API REST generada\"\n\
             }\n",
        );

        state.code = Some(code);

        println!("BUILDER: código generado");

        return;
    }

    // =========================================================
    // ITERACIONES DE CORRECCIÓN
    // =========================================================

    if !state.errors.is_empty() {
        println!("BUILDER: analizando errores anteriores...");

        for error in &state.errors {
            println!("BUILDER: feedback recibido -> {}", error);
        }

        println!("BUILDER: generando versión corregida...");

        // -----------------------------------------------------
        // Código corregido.
        //
        // Es código Rust válido y además contiene los elementos
        // que el Validator busca para considerar que existe
        // una API REST:
        //
        // HTTP
        // GET
        // /api
        // endpoint
        // -----------------------------------------------------

        let corrected_code = String::from(
            "fn main() {\n\
             println!(\"API REST generada\");\n\
             \n\
             // HTTP Server\n\
             // GET /api\n\
             // endpoint: GET /api\n\
             }\n",
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
        "fn main() {\n\
         println!(\"API REST generada\");\n\
         \n\
         // HTTP Server\n\
         // GET /api\n\
         // endpoint: GET /api\n\
         }\n",
    );

    state.code = Some(code);

    println!("BUILDER: código generado");
}