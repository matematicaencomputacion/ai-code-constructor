use crate::planner::PlanKind;
use crate::state::CodeState;

/// Genera código a partir del estado actual.
///
/// La primera iteración genera deliberadamente código
/// defectuoso para probar el ciclo de compilación y reparación.
///
/// Las siguientes iteraciones utilizan el feedback generado
/// por el Repairer y el plan generado por el Planner.
pub fn build(state: &mut CodeState) {
    println!("BUILDER: generando código...");
    println!("BUILDER: request -> {}", state.request);

    let plan = match &state.plan {
        Some(plan) => plan,
        None => {
            println!("BUILDER: no hay plan disponible.");
            state.code = None;
            return;
        }
    };

    println!("BUILDER: utilizando plan -> {:?}", plan);

    // =========================================================
    // PRIMERA ITERACIÓN
    // =========================================================

    if state.iteration == 1 {
        println!("BUILDER: generando primera versión...");

        // Código deliberadamente defectuoso para probar
        // el ciclo Compiler -> Repairer -> Builder.
        let code = format!(
            r#"fn main() {{
    println!("Request: {}"
}}
"#,
            state.request
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
    }

    // =========================================================
    // GENERACIÓN BASADA EN EL TIPO DE PLAN
    // =========================================================

    let implementation = match plan.kind {
        PlanKind::Calculator => r#"fn main() {
    let resultado = sumar(2, 3);
    println!("Resultado: {}", resultado);
}

fn sumar(a: i32, b: i32) -> i32 {
    a + b
}
"#
        .to_string(),

        PlanKind::Authentication => r#"fn main() {
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
"#
        .to_string(),

        PlanKind::Api => r#"fn main() {
    crear_servidor();
    definir_endpoints();
    implementar_handlers();
}

fn crear_servidor() {
    println!("Servidor HTTP configurado");
}

fn definir_endpoints() {
    println!("Endpoints definidos");
}

fn implementar_handlers() {
    println!("Handlers implementados");
}
"#
        .to_string(),

        PlanKind::Generic => r#"fn main() {
    analizar_requisitos();
    disenar_solucion();
    implementar_funcionalidad();
}

fn analizar_requisitos() {
    println!("Requisitos analizados");
}

fn disenar_solucion() {
    println!("Solución diseñada");
}

fn implementar_funcionalidad() {
    println!("Funcionalidad principal implementada");
}
"#
        .to_string(),
    };

    state.code = Some(implementation);

    println!("BUILDER: código generado a partir del plan");
}
