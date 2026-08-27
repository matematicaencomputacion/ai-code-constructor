use crate::harness::context::AgentContext;
use crate::harness::correction::Correction;
use crate::harness::observation::AgentObservation;
use crate::harness::tools::REPAIR_DIAGNOSTIC;

/// Entrada para una [`CorrectionPolicy`]: observación actual + contexto acumulado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrectionPolicyInput<'a> {
    pub observation: &'a AgentObservation,
    pub context: &'a AgentContext,
}

impl<'a> CorrectionPolicyInput<'a> {
    pub fn new(observation: &'a AgentObservation, context: &'a AgentContext) -> Self {
        Self {
            observation,
            context,
        }
    }

    pub fn working_code(&self) -> Option<&str> {
        self.context.working_code()
    }

    pub fn repairer_feedback(&self) -> Vec<&str> {
        self.observation.repairer_feedback()
    }

    /// Errores del último outcome fallido de ValidationTool en el historial.
    pub fn last_validation_errors(&self) -> Vec<String> {
        self.context
            .observation_history
            .iter()
            .rev()
            .find_map(|observation| {
                if observation.is_validation_outcome() && observation.is_failure() {
                    Some(
                        observation
                            .validator_errors()
                            .into_iter()
                            .map(str::to_string)
                            .collect(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }
}

/// Error controlado cuando la policy no puede proponer correcciones seguras.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrectionPolicyError {
    InsufficientInformation(String),
    UnsupportedObservation(String),
}

impl std::fmt::Display for CorrectionPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientInformation(message) => {
                write!(f, "información insuficiente: {message}")
            }
            Self::UnsupportedObservation(message) => {
                write!(f, "observación no soportada: {message}")
            }
        }
    }
}

/// Contrato desacoplado: Observation + contexto → correcciones estructuradas.
///
/// No modifica código, no ejecuta Tools ni componentes del Constructor.
pub trait CorrectionPolicy: Send + Sync {
    fn propose_corrections(
        &self,
        input: &CorrectionPolicyInput<'_>,
    ) -> Result<Vec<Correction>, CorrectionPolicyError>;
}

/// Policy determinista para tests: interpreta diagnóstico y propone operaciones
/// estructuradas a partir de la Observation y el historial disponible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeterministicCorrectionPolicy;

impl DeterministicCorrectionPolicy {
    pub fn new() -> Self {
        Self
    }
}

impl CorrectionPolicy for DeterministicCorrectionPolicy {
    fn propose_corrections(
        &self,
        input: &CorrectionPolicyInput<'_>,
    ) -> Result<Vec<Correction>, CorrectionPolicyError> {
        let AgentObservation::ToolOutcome {
            tool_name,
            success: true,
            ..
        } = input.observation
        else {
            return Err(CorrectionPolicyError::UnsupportedObservation(
                "se esperaba ToolOutcome exitoso".to_string(),
            ));
        };

        if tool_name != REPAIR_DIAGNOSTIC {
            return Err(CorrectionPolicyError::UnsupportedObservation(format!(
                "se esperaba outcome de {REPAIR_DIAGNOSTIC}, recibido {tool_name}"
            )));
        }

        let feedback = input.repairer_feedback();
        if feedback.is_empty() {
            return Err(CorrectionPolicyError::InsufficientInformation(
                "RepairDiagnostic sin feedback".to_string(),
            ));
        }

        let working_code = input.working_code().ok_or_else(|| {
            CorrectionPolicyError::InsufficientInformation(
                "working_code ausente en AgentContext".to_string(),
            )
        })?;

        if working_code.trim().is_empty() {
            return Err(CorrectionPolicyError::InsufficientInformation(
                "working_code vacío".to_string(),
            ));
        }

        let errors = input.last_validation_errors();
        if errors.is_empty() {
            return Err(CorrectionPolicyError::InsufficientInformation(
                "no hay errores de validación previos en el historial".to_string(),
            ));
        }

        let mut corrections = Vec::new();
        for error in &errors {
            corrections.extend(infer_corrections_for_error(error, &feedback, working_code)?);
        }

        if corrections.is_empty() {
            return Err(CorrectionPolicyError::InsufficientInformation(
                "no se derivaron correcciones seguras desde la observación".to_string(),
            ));
        }

        Ok(dedupe_corrections(corrections))
    }
}

/// Infiere correcciones condicionadas al error, feedback y código observables.
fn infer_corrections_for_error(
    error: &str,
    feedback: &[&str],
    working_code: &str,
) -> Result<Vec<Correction>, CorrectionPolicyError> {
    if error.contains("API REST") {
        return infer_missing_marker_corrections(
            working_code,
            error,
            &[
                ("HTTP", "NET"),
                ("Endpoints", "Routes"),
                ("endpoint", "route"),
                ("/api", "/x"),
                ("GET", "READ"),
                ("POST", "WRITE"),
                ("Server", "Host"),
                ("server", "host"),
            ],
        );
    }

    if error.contains("calculadora")
        && !working_code.contains("sumar")
        && !working_code.contains("a + b")
    {
        return Ok(vec![Correction::insert_session_text(
            working_code.len(),
            "\nfn sumar(a: i32, b: i32) -> i32 { a + b }\n",
        )]);
    }

    if error.contains("autenticación") {
        let mut corrections = Vec::new();
        if !working_code.contains("validar_credenciales") {
            corrections.push(Correction::insert_session_text(
                working_code.len(),
                "\nfn validar_credenciales(usuario: &str, password: &str) -> bool { !usuario.is_empty() && !password.is_empty() }\n",
            ));
        }
        if !working_code.contains("Login correcto")
            && !working_code.contains("Login incorrecto")
            && let Some(main_pos) = working_code.find("fn main()")
        {
            corrections.push(Correction::insert_session_text(
                main_pos,
                "// login-check\n",
            ));
        }
        if corrections.is_empty() {
            return Err(CorrectionPolicyError::InsufficientInformation(
                "autenticación: no se identificaron inserciones seguras".to_string(),
            ));
        }
        return Ok(corrections);
    }

    if feedback
        .iter()
        .any(|item| item.contains("delimitador") || item.contains("delimiter"))
        && working_code.contains("println!(\"")
        && working_code.contains('\n')
        && !working_code.contains("\");")
        && let Some(pos) = working_code.rfind('\n')
    {
        return Ok(vec![Correction::insert_session_text(pos, "\");")]);
    }

    Err(CorrectionPolicyError::InsufficientInformation(format!(
        "sin reglas aplicables para el error: {error}"
    )))
}

/// Propone ReplaceText solo cuando el marcador requerido falta y el sustituto
/// observable está presente en el código de trabajo.
fn infer_missing_marker_corrections(
    working_code: &str,
    error: &str,
    required_to_substitute: &[(&str, &str)],
) -> Result<Vec<Correction>, CorrectionPolicyError> {
    let mut corrections = Vec::new();

    for (required, substitute) in required_to_substitute {
        if working_code.contains(required) {
            continue;
        }
        if working_code.contains(substitute) {
            corrections.push(Correction::replace_session_text(*substitute, *required));
        }
    }

    if corrections.is_empty() {
        return Err(CorrectionPolicyError::InsufficientInformation(format!(
            "error `{error}` sin sustitutos observables en working_code"
        )));
    }

    Ok(corrections)
}

fn dedupe_corrections(corrections: Vec<Correction>) -> Vec<Correction> {
    let mut unique = Vec::new();
    for correction in corrections {
        if !unique.contains(&correction) {
            unique.push(correction);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::EvaluationVerdict;
    use crate::harness::Evidence;
    use crate::harness::tools::VALIDATE;

    fn repair_diagnostic_observation() -> AgentObservation {
        AgentObservation::ToolOutcome {
            tool_name: REPAIR_DIAGNOSTIC.to_string(),
            success: true,
            output: "feedback".to_string(),
            evidence: vec![Evidence::new(
                "repairer_feedback_0",
                "Analizar y corregir el siguiente error: API REST",
            )],
            verdict: EvaluationVerdict::Pass,
        }
    }

    fn validation_fail_context(invalid_code: &str) -> AgentContext {
        let mut ctx = AgentContext::new("policy-test");
        ctx.update_working_source(invalid_code);
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: VALIDATE.to_string(),
            success: false,
            output: "fail".to_string(),
            evidence: vec![Evidence::new(
                "validator_error_0",
                "El código no contiene la implementación esperada de API REST",
            )],
            verdict: EvaluationVerdict::Fail,
        });
        ctx
    }

    #[test]
    fn correction_policy_trait_is_object_safe() {
        let _: Box<dyn CorrectionPolicy> = Box::new(DeterministicCorrectionPolicy::new());
    }

    #[test]
    fn deterministic_policy_consumes_real_observation() {
        let policy = DeterministicCorrectionPolicy::new();
        let invalid = "Servidor NET con Routes en /x";
        let ctx = validation_fail_context(invalid);
        let obs = repair_diagnostic_observation();
        let input = CorrectionPolicyInput::new(&obs, &ctx);

        let corrections = policy.propose_corrections(&input).expect("corrections");
        assert!(!corrections.is_empty());
        assert!(corrections.iter().all(|c| {
            matches!(
                c.operation,
                CorrectionOperation::ReplaceText { .. }
                    | CorrectionOperation::InsertText { .. }
                    | CorrectionOperation::RemoveText { .. }
            )
        }));
    }

    #[test]
    fn deterministic_policy_does_not_return_full_code() {
        let policy = DeterministicCorrectionPolicy::new();
        let ctx = validation_fail_context("Servidor NET");
        let obs = repair_diagnostic_observation();
        let input = CorrectionPolicyInput::new(&obs, &ctx);

        let corrections = policy.propose_corrections(&input).expect("corrections");
        for correction in &corrections {
            match &correction.operation {
                CorrectionOperation::ReplaceText {
                    search,
                    replacement,
                } => {
                    assert!(search.len() < 20);
                    assert!(replacement.len() < 20);
                }
                CorrectionOperation::InsertText { text, .. } => assert!(text.len() < 200),
                CorrectionOperation::RemoveText { .. } => {}
            }
        }
    }

    #[test]
    fn insufficient_observation_returns_controlled_error() {
        let policy = DeterministicCorrectionPolicy::new();
        let ctx = AgentContext::new("empty");
        let obs = AgentObservation::NoOpDone;
        let input = CorrectionPolicyInput::new(&obs, &ctx);

        let err = policy.propose_corrections(&input).unwrap_err();
        assert!(matches!(
            err,
            CorrectionPolicyError::UnsupportedObservation(_)
        ));
    }

    #[test]
    fn missing_working_code_returns_controlled_error() {
        let policy = DeterministicCorrectionPolicy::new();
        let mut ctx = AgentContext::new("no-code");
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: VALIDATE.to_string(),
            success: false,
            output: "fail".to_string(),
            evidence: vec![Evidence::new("validator_error_0", "error")],
            verdict: EvaluationVerdict::Fail,
        });
        let obs = repair_diagnostic_observation();
        let input = CorrectionPolicyInput::new(&obs, &ctx);

        let err = policy.propose_corrections(&input).unwrap_err();
        assert!(matches!(
            err,
            CorrectionPolicyError::InsufficientInformation(_)
        ));
    }

    #[test]
    fn policy_can_propose_multiple_corrections() {
        let policy = DeterministicCorrectionPolicy::new();
        let invalid = "Servidor NET\nRoutes en /x\nREAD WRITE Host host route";
        let ctx = validation_fail_context(invalid);
        let obs = repair_diagnostic_observation();
        let input = CorrectionPolicyInput::new(&obs, &ctx);

        let corrections = policy.propose_corrections(&input).expect("corrections");
        assert!(corrections.len() > 1);
    }

    use crate::harness::correction::CorrectionOperation;
}
