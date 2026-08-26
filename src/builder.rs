use crate::planner::PlanKind;
use crate::state::CodeState;

/// Genera código a partir del estado actual.
///
/// La primera iteración genera deliberadamente código
/// defectuoso para probar el ciclo de compilación y reparación.
///
/// Las siguientes iteraciones combinan el plan del Planner
/// con el feedback del Repairer para producir `state.code`.
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

        println!("BUILDER: generando versión corregida con feedback...");
    }

    // =========================================================
    // GENERACIÓN BASADA EN EL TIPO DE PLAN
    // =========================================================

    let base_implementation = match plan.kind {
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

    let implementation = apply_feedback(base_implementation, &state.feedback);

    state.code = Some(implementation);

    println!("BUILDER: código generado a partir del plan y el feedback");
}

/// Aplica el feedback del Repairer sobre la implementación base del plan.
///
/// El efecto es determinista y depende del *contenido* de cada mensaje:
/// distintos tipos de feedback producen transformaciones distintas.
fn apply_feedback(base_implementation: String, feedback: &[String]) -> String {
    if feedback.is_empty() {
        return base_implementation;
    }

    let has_delimiter_feedback = feedback
        .iter()
        .any(|item| item.contains("delimitadores") || item.contains("delimitador sin cerrar"));

    let has_structure_feedback = feedback
        .iter()
        .any(|item| item.contains("estructura Rust válida"));

    let mut implementation = base_implementation;

    if has_delimiter_feedback {
        implementation = inject_main_call(implementation, "asegurar_delimitadores_balanceados();");
        implementation.push_str(
            r#"
fn asegurar_delimitadores_balanceados() {
    let _llaves = ('{', '}');
    let _parens = ('(', ')');
    println!("Delimitadores balanceados tras feedback");
}
"#,
        );
    }

    if has_structure_feedback {
        implementation = inject_main_call(implementation, "validar_estructura_rust();");
        implementation.push_str(
            r#"
fn validar_estructura_rust() {
    println!("Estructura Rust validada tras feedback");
}
"#,
        );
    }

    // Feedback genérico: efecto distinto al de delimitadores/estructura,
    // para que el contenido (no solo la presencia) determine el código.
    if !has_delimiter_feedback && !has_structure_feedback {
        implementation = inject_main_call(implementation, "aplicar_feedback_generico();");
        implementation.push_str(
            r#"
fn aplicar_feedback_generico() {
    println!("Feedback genérico aplicado");
}
"#,
        );
    }

    implementation
}

fn inject_main_call(implementation: String, call: &str) -> String {
    implementation.replacen("fn main() {\n", &format!("fn main() {{\n    {call}\n"), 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{BuildPlan, PlanKind};
    use crate::state::CodeState;

    fn state_with_plan(kind: PlanKind, request: &str, iteration: u32) -> CodeState {
        CodeState {
            request: request.to_string(),
            plan: Some(BuildPlan {
                kind,
                steps: vec!["paso".to_string()],
            }),
            code: None,
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration,
        }
    }

    fn braces_are_balanced(code: &str) -> bool {
        let opens = code.chars().filter(|c| *c == '{').count();
        let closes = code.chars().filter(|c| *c == '}').count();
        opens == closes
    }

    #[test]
    fn builder_first_iteration_generates_deliberately_broken_code() {
        let request = "Crear una calculadora";
        let mut state = state_with_plan(PlanKind::Calculator, request, 1);
        state
            .feedback
            .push("Revisar los delimitadores...".to_string());

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(code.contains(request));
        assert!(code.contains("fn main()"));
        // Código incompleto a propósito (falta cierre de println)
        assert!(!code.contains(r#"println!("Request: {}");"#));
        // iteration == 1 ignora el feedback y no aplica correcciones
        assert!(!code.contains("asegurar_delimitadores_balanceados"));
    }

    #[test]
    fn builder_generates_api_code() {
        let mut state = state_with_plan(PlanKind::Api, "Crear una API REST", 2);

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(code.contains("crear_servidor"));
        assert!(code.contains("definir_endpoints"));
        assert!(code.contains("implementar_handlers"));
        assert!(code.contains("Servidor HTTP"));
        assert!(code.contains("Endpoints"));
        assert!(code.contains("Handlers"));
    }

    #[test]
    fn builder_generates_calculator_code() {
        let mut state = state_with_plan(PlanKind::Calculator, "Crear una calculadora", 2);

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(code.contains("sumar"));
        assert!(code.contains("Resultado"));
        assert!(code.contains("a + b"));
    }

    #[test]
    fn builder_generates_authentication_code() {
        let mut state = state_with_plan(PlanKind::Authentication, "Crear un sistema de login", 2);

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(code.contains("validar_credenciales"));
        assert!(code.contains("Login correcto"));
        assert!(code.contains("Login incorrecto"));
    }

    #[test]
    fn builder_generates_generic_code() {
        let mut state = state_with_plan(PlanKind::Generic, "Crear una app de inventario", 2);

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(code.contains("analizar_requisitos"));
        assert!(code.contains("disenar_solucion"));
        assert!(code.contains("implementar_funcionalidad"));
    }

    #[test]
    fn builder_iteration_two_without_feedback_keeps_base_plan_code() {
        let mut state = state_with_plan(PlanKind::Calculator, "Crear una calculadora", 2);

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(code.contains("sumar"));
        assert!(code.contains("a + b"));
        assert!(!code.contains("asegurar_delimitadores_balanceados"));
        assert!(!code.contains("aplicar_feedback_generico"));
        assert!(!code.contains("validar_estructura_rust"));
    }

    #[test]
    fn builder_delimiter_feedback_produces_balanced_corrected_code() {
        let mut state = state_with_plan(PlanKind::Api, "Crear una API REST", 2);
        state.feedback.push(
            "Revisar los delimitadores del código generado. \
             Verificar que todas las llaves { } y paréntesis ( ) \
             estén correctamente balanceados."
                .to_string(),
        );

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(braces_are_balanced(&code));
        assert!(code.contains("asegurar_delimitadores_balanceados"));
        assert!(code.contains("Delimitadores balanceados tras feedback"));
        assert!(code.contains("crear_servidor"));
        assert!(!code.contains(r#"println!("Request:"#));
    }

    #[test]
    fn builder_delimiter_feedback_changes_code_versus_no_feedback() {
        let mut without_feedback =
            state_with_plan(PlanKind::Calculator, "Crear una calculadora", 2);
        build(&mut without_feedback);
        let code_without = without_feedback
            .code
            .expect("Debe generar código sin feedback");

        let mut with_feedback = state_with_plan(PlanKind::Calculator, "Crear una calculadora", 2);
        with_feedback.feedback.push(
            "Existe un delimitador sin cerrar. \
             Revisar llaves, paréntesis y corchetes."
                .to_string(),
        );
        build(&mut with_feedback);
        let code_with = with_feedback
            .code
            .expect("Debe generar código con feedback");

        assert_ne!(code_with, code_without);
        assert!(code_with.contains("asegurar_delimitadores_balanceados"));
        assert!(!code_without.contains("asegurar_delimitadores_balanceados"));
        assert!(code_with.contains("sumar"));
        assert!(braces_are_balanced(&code_with));
    }

    #[test]
    fn builder_consumes_feedback_content_not_only_presence() {
        let mut delimiter_state = state_with_plan(PlanKind::Generic, "app", 2);
        delimiter_state
            .feedback
            .push("Revisar los delimitadores del código generado.".to_string());
        build(&mut delimiter_state);
        let delimiter_code = delimiter_state
            .code
            .expect("código con feedback delimitadores");

        let mut generic_state = state_with_plan(PlanKind::Generic, "app", 2);
        generic_state.feedback.push(
            "Analizar y corregir el siguiente error de compilación: cannot find value `x`"
                .to_string(),
        );
        build(&mut generic_state);
        let generic_code = generic_state.code.expect("código con feedback genérico");

        let mut structure_state = state_with_plan(PlanKind::Generic, "app", 2);
        structure_state
            .feedback
            .push("El código contiene texto fuera de una estructura Rust válida.".to_string());
        build(&mut structure_state);
        let structure_code = structure_state
            .code
            .expect("código con feedback de estructura");

        assert_ne!(delimiter_code, generic_code);
        assert_ne!(delimiter_code, structure_code);
        assert_ne!(generic_code, structure_code);

        assert!(delimiter_code.contains("asegurar_delimitadores_balanceados"));
        assert!(!delimiter_code.contains("aplicar_feedback_generico"));
        assert!(!delimiter_code.contains("validar_estructura_rust"));

        assert!(generic_code.contains("aplicar_feedback_generico"));
        assert!(!generic_code.contains("asegurar_delimitadores_balanceados"));

        assert!(structure_code.contains("validar_estructura_rust"));
        assert!(!structure_code.contains("asegurar_delimitadores_balanceados"));
    }
}
