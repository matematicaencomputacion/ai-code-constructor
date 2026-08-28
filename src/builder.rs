use crate::planner::PlanKind;
use crate::state::CodeState;

/// Definición determinista de archivos iniciales para un [`PlanKind`].
///
/// Fuente canónica del Builder para AutonomousConstruction / [`RustArtifact`].
/// `initial_source_for_kind` expone solo el contenido del **primary**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialArtifactDefinition {
    pub primary_path: &'static str,
    files: Vec<(&'static str, String)>,
}

impl InitialArtifactDefinition {
    pub fn primary_source(&self) -> &str {
        self.files
            .iter()
            .find(|(path, _)| *path == self.primary_path)
            .map(|(_, source)| source.as_str())
            .unwrap_or("")
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn files(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.files
            .iter()
            .map(|(path, source)| (*path, source.as_str()))
    }
}

/// Archivos iniciales deterministas por [`PlanKind`] (primary + siblings).
pub fn initial_artifact_definition_for_kind(kind: PlanKind) -> InitialArtifactDefinition {
    match kind {
        PlanKind::Authentication => InitialArtifactDefinition {
            primary_path: "src/main.rs",
            files: vec![
                ("src/main.rs", authentication_main_module()),
                ("src/auth.rs", authentication_auth_module()),
            ],
        },
        other => {
            let source = base_implementation_for(other);
            InitialArtifactDefinition {
                primary_path: "src/main.rs",
                files: vec![("src/main.rs", source)],
            }
        }
    }
}

fn authentication_main_module() -> String {
    r#"mod auth;

fn main() {
    let usuario_valido = auth::validar_credenciales("usuario", "password");

    if usuario_valido {
        println!("Login correcto");
    } else {
        println!("Login incorrecto");
    }
}
"#
    .to_string()
}

fn authentication_auth_module() -> String {
    r#"pub fn validar_credenciales(usuario: &str, password: &str) -> bool {
    !usuario.is_empty() && !password.is_empty()
}
"#
    .to_string()
}

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
        // Defecto: println! sin cerrar el llamado (falta `);`).
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
    // La base depende exclusivamente de `plan.kind`. El feedback solo
    // transforma esa base; nunca concatena plantillas de otros planes.

    let kind = plan.kind;
    let base_implementation = base_implementation_for(kind);

    let implementation = apply_feedback(base_implementation, &state.feedback, &state.request);

    state.code = Some(implementation);

    println!("BUILDER: código generado a partir del plan y el feedback");
}

fn base_implementation_for(kind: PlanKind) -> String {
    match kind {
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
    }
}

/// Source determinista del Artifact inicial a partir de [`PlanKind`].
///
/// No usa AI ni [`CodeState`]. Devuelve el contenido del archivo **primary**
/// de [`initial_artifact_definition_for_kind`].
pub fn initial_source_for_kind(kind: PlanKind) -> String {
    initial_artifact_definition_for_kind(kind)
        .primary_source()
        .to_string()
}

/// Defecto deliberado de la iteración 1: `println!("Request: …"` sin `);`.
#[cfg(test)]
fn defective_request_println(request: &str) -> String {
    // Tras el cierre de la cadena viene un salto de línea, no `);`.
    format!(r#"println!("Request: {request}""#) + "\n"
}

/// Misma sentencia con los delimitadores correctamente cerrados.
fn corrected_request_println(request: &str) -> String {
    format!(r#"println!("Request: {request}");"#)
}

fn has_delimiter_feedback(feedback: &[String]) -> bool {
    feedback
        .iter()
        .any(|item| item.contains("delimitadores") || item.contains("delimitador sin cerrar"))
}

/// Aplica el feedback del Repairer sobre la implementación base del plan.
///
/// Para feedback de delimitadores, corrige el defecto concreto de la iteración 1:
/// cierra el `println!("Request: …")` que quedó sin `);`.
fn apply_feedback(base_implementation: String, feedback: &[String], request: &str) -> String {
    if feedback.is_empty() {
        return base_implementation;
    }

    let has_structure_feedback = feedback
        .iter()
        .any(|item| item.contains("estructura Rust válida"));

    let mut implementation = base_implementation;

    if has_delimiter_feedback(feedback) {
        // Reparación real: reintroducir el println del request YA cerrado.
        let fixed = corrected_request_println(request);
        implementation = inject_main_statement(implementation, &fixed);
    }

    if has_structure_feedback {
        implementation = inject_main_statement(implementation, "validar_estructura_rust();");
        implementation.push_str(
            r#"
fn validar_estructura_rust() {
    println!("Estructura Rust validada tras feedback");
}
"#,
        );
    }

    // Feedback genérico: efecto distinto al de delimitadores/estructura.
    if !has_delimiter_feedback(feedback) && !has_structure_feedback {
        implementation = inject_main_statement(implementation, "aplicar_feedback_generico();");
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

fn inject_main_statement(implementation: String, statement: &str) -> String {
    implementation.replacen(
        "fn main() {\n",
        &format!("fn main() {{\n    {statement}\n"),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler;
    use crate::planner::{BuildPlan, PlanKind};
    use crate::repairer;
    use crate::state::CodeState;
    use std::sync::Mutex;

    static COMPILE_LOCK: Mutex<()> = Mutex::new(());

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

    fn contains_defective_request_println(code: &str, request: &str) -> bool {
        code.contains(&defective_request_println(request))
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
        assert!(contains_defective_request_println(&code, request));
        // iteration == 1 ignora el feedback y no aplica la corrección
        assert!(!code.contains(&corrected_request_println(request)));
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
        assert!(!code.contains("Request:"));
        assert!(!code.contains("aplicar_feedback_generico"));
        assert!(!code.contains("validar_estructura_rust"));
    }

    #[test]
    fn builder_delimiter_feedback_produces_balanced_corrected_code() {
        let request = "Crear una API REST";
        let mut state = state_with_plan(PlanKind::Api, request, 2);
        state.feedback.push(
            "Revisar los delimitadores del código generado. \
             Verificar que todas las llaves { } y paréntesis ( ) \
             estén correctamente balanceados."
                .to_string(),
        );

        build(&mut state);

        let code = state.code.expect("El Builder debe generar código");
        assert!(braces_are_balanced(&code));
        assert!(code.contains(&corrected_request_println(request)));
        assert!(!contains_defective_request_println(&code, request));
        assert!(code.contains("crear_servidor"));
    }

    #[test]
    fn builder_delimiter_feedback_changes_code_versus_no_feedback() {
        let request = "Crear una calculadora";
        let mut without_feedback = state_with_plan(PlanKind::Calculator, request, 2);
        build(&mut without_feedback);
        let code_without = without_feedback
            .code
            .expect("Debe generar código sin feedback");

        let mut with_feedback = state_with_plan(PlanKind::Calculator, request, 2);
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
        assert!(code_with.contains(&corrected_request_println(request)));
        assert!(!code_without.contains(&corrected_request_println(request)));
        assert!(code_with.contains("sumar"));
        assert!(braces_are_balanced(&code_with));
    }

    #[test]
    fn builder_consumes_feedback_content_not_only_presence() {
        let request = "app";
        let mut delimiter_state = state_with_plan(PlanKind::Generic, request, 2);
        delimiter_state
            .feedback
            .push("Revisar los delimitadores del código generado.".to_string());
        build(&mut delimiter_state);
        let delimiter_code = delimiter_state
            .code
            .expect("código con feedback delimitadores");

        let mut generic_state = state_with_plan(PlanKind::Generic, request, 2);
        generic_state.feedback.push(
            "Analizar y corregir el siguiente error de compilación: cannot find value `x`"
                .to_string(),
        );
        build(&mut generic_state);
        let generic_code = generic_state.code.expect("código con feedback genérico");

        let mut structure_state = state_with_plan(PlanKind::Generic, request, 2);
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

        assert!(delimiter_code.contains(&corrected_request_println(request)));
        assert!(!delimiter_code.contains("aplicar_feedback_generico"));
        assert!(!delimiter_code.contains("validar_estructura_rust"));

        assert!(generic_code.contains("aplicar_feedback_generico"));
        assert!(!generic_code.contains(&corrected_request_println(request)));

        assert!(structure_code.contains("validar_estructura_rust"));
        assert!(!structure_code.contains(&corrected_request_println(request)));
    }

    #[test]
    fn delimiter_repair_chain_fixes_concrete_compile_defect() {
        let _guard = COMPILE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear una API REST";

        // A. iteration == 1 produce código que no compila
        let mut state = state_with_plan(PlanKind::Api, request, 1);
        build(&mut state);
        let broken = state.code.clone().expect("Debe existir código defectuoso");
        assert!(contains_defective_request_println(&broken, request));

        let compile_error =
            compiler::compile(&broken).expect_err("A: el código defectuoso debe fallar");

        // B. el fallo es el error concreto de delimitadores
        assert!(
            compile_error.contains("mismatched closing delimiter")
                || compile_error.contains("unclosed delimiter"),
            "B: error inesperado: {compile_error}"
        );

        // C. Repairer convierte ese error en feedback concreto
        state
            .errors
            .push(format!("Error de compilación: {}", compile_error.trim()));
        repairer::repair(&mut state);
        assert!(
            state
                .feedback
                .iter()
                .any(|f| f.contains("delimitadores") || f.contains("delimitador")),
            "C: feedback inesperado: {:?}",
            state.feedback
        );

        // D/E. Builder consume el feedback y elimina el defecto
        state.iteration = 2;
        build(&mut state);
        let fixed = state
            .code
            .as_ref()
            .expect("D: debe generar código corregido");
        assert!(
            fixed.contains(&corrected_request_println(request)),
            "D: debe incluir el println corregido"
        );
        assert!(
            !contains_defective_request_println(fixed, request),
            "E: no debe conservar el defecto original"
        );
        assert!(fixed.contains("crear_servidor"));
        assert!(fixed.contains("definir_endpoints"));
        assert!(fixed.contains("implementar_handlers"));

        // El código Api no debe mezclar fragmentos de otros planes
        assert!(
            !fixed.contains("validar_credenciales"),
            "no debe contener Authentication"
        );
        assert!(
            !fixed.contains("password.is_empty()"),
            "no debe contener fragmento residual de Authentication"
        );
        assert!(
            !fixed.contains("rio.is_empty()"),
            "no debe contener cola residual de Authentication"
        );
        assert!(!fixed.contains("fn sumar"), "no debe contener Calculator");
        assert!(
            !fixed.contains("analizar_requisitos"),
            "no debe contener Generic"
        );
        assert!(
            braces_are_balanced(fixed),
            "no debe haber llaves de cierre extra"
        );

        // F. el código corregido compila
        compiler::compile(fixed).expect("F: el código corregido debe compilar");
    }

    #[test]
    fn api_delimiter_repair_stays_within_api_plan_and_compiles() {
        let _guard = COMPILE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let request = "Crear una API REST";
        let mut state = state_with_plan(PlanKind::Api, request, 2);
        state.feedback.push(
            "Revisar los delimitadores del código generado. \
             Verificar que todas las llaves { } y paréntesis ( ) \
             estén correctamente balanceados."
                .to_string(),
        );

        build(&mut state);

        let code = state.code.expect("debe generar código Api corregido");
        assert!(code.contains(&corrected_request_println(request)));
        assert!(!contains_defective_request_println(&code, request));
        assert!(code.contains("crear_servidor"));
        assert!(code.contains("Servidor HTTP"));
        assert!(!code.contains("validar_credenciales"));
        assert!(!code.contains("password.is_empty()"));
        assert!(!code.contains("rio.is_empty()"));
        assert!(!code.contains("fn sumar"));
        assert!(!code.contains("analizar_requisitos"));
        assert!(braces_are_balanced(&code));
        compiler::compile(&code).expect("Api + feedback de delimitadores debe compilar");
    }

    #[test]
    fn initial_source_for_kind_is_deterministic() {
        for kind in [
            PlanKind::Api,
            PlanKind::Calculator,
            PlanKind::Authentication,
            PlanKind::Generic,
        ] {
            let first = initial_source_for_kind(kind);
            let second = initial_source_for_kind(kind);
            assert_eq!(first, second);
            assert!(!first.trim().is_empty());
            assert!(first.contains("fn main()"));
        }
    }

    #[test]
    fn initial_source_covers_all_plan_kinds() {
        let api = initial_source_for_kind(PlanKind::Api);
        assert!(api.contains("crear_servidor"));
        assert!(api.contains("Servidor HTTP"));

        let calc = initial_source_for_kind(PlanKind::Calculator);
        assert!(calc.contains("sumar"));

        let auth = initial_source_for_kind(PlanKind::Authentication);
        assert!(auth.contains("mod auth"));
        assert!(auth.contains("auth::validar_credenciales"));

        let auth_def = initial_artifact_definition_for_kind(PlanKind::Authentication);
        assert_eq!(auth_def.file_count(), 2);
        assert!(
            auth_def
                .files()
                .any(|(path, source)| path == "src/auth.rs"
                    && source.contains("validar_credenciales"))
        );

        let generic = initial_source_for_kind(PlanKind::Generic);
        assert!(generic.contains("analizar_requisitos"));
    }

    #[test]
    fn initial_artifact_definition_is_deterministic() {
        for kind in [
            PlanKind::Api,
            PlanKind::Calculator,
            PlanKind::Authentication,
            PlanKind::Generic,
        ] {
            let first = initial_artifact_definition_for_kind(kind);
            let second = initial_artifact_definition_for_kind(kind);
            assert_eq!(first, second);
        }
    }

    #[test]
    fn authentication_is_multi_file_other_kinds_single_file() {
        assert_eq!(
            initial_artifact_definition_for_kind(PlanKind::Calculator).file_count(),
            1
        );
        assert_eq!(
            initial_artifact_definition_for_kind(PlanKind::Api).file_count(),
            1
        );
        assert_eq!(
            initial_artifact_definition_for_kind(PlanKind::Generic).file_count(),
            1
        );
        assert_eq!(
            initial_artifact_definition_for_kind(PlanKind::Authentication).file_count(),
            2
        );
    }

    #[test]
    fn initial_source_matches_builder_correction_base() {
        let mut state = state_with_plan(PlanKind::Api, "Crear una API REST", 2);
        build(&mut state);
        assert_eq!(
            state.code.as_deref(),
            Some(initial_source_for_kind(PlanKind::Api).as_str())
        );
    }
}
