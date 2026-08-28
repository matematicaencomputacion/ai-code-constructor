use std::sync::Arc;

use crate::harness::action::AgentAction;
use crate::harness::agent::Agent;
use crate::harness::agent_loop::{AgentLoop, LoopResult};
use crate::harness::artifact::{ArtifactId, RustArtifact};
use crate::harness::context::AgentContext;
use crate::harness::correction_policy::{
    CorrectionPolicy, CorrectionPolicyInput, DeterministicCorrectionPolicy,
};
use crate::harness::observation::AgentObservation;
use crate::harness::runtime::Harness;
use crate::harness::tools::{APPLY_CORRECTION, COMPILE, REPAIR_DIAGNOSTIC, VALIDATE};
use crate::planner::PlanKind;
use crate::state::CodeState;

/// Snapshot explícito de artefactos del Constructor para una sesión del AgentLoop.
///
/// No muta el [`CodeState`] original. El código se comparte con `Arc` para evitar
/// duplicar el mismo buffer cuando Bridge y Agent lo necesitan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorArtifacts {
    pub request: Arc<str>,
    pub plan_kind: Option<PlanKind>,
    pub plan_steps: Arc<[String]>,
    pub code: Option<Arc<str>>,
    pub iteration: u32,
    /// Artifact de dominio derivado del CodeState (sin mutarlo).
    pub artifact: Option<RustArtifact>,
}

impl ConstructorArtifacts {
    pub fn snapshot(state: &CodeState) -> Self {
        let artifact = state.code.as_ref().map(|code| {
            RustArtifact::with_id(
                ArtifactId::new(format!("constructor:{}:{}", state.request, state.iteration)),
                "main.rs",
                code.as_str(),
            )
        });
        Self {
            request: Arc::from(state.request.as_str()),
            plan_kind: state.plan.as_ref().map(|plan| plan.kind),
            plan_steps: state
                .plan
                .as_ref()
                .map(|plan| Arc::from(plan.steps.as_slice()))
                .unwrap_or_else(|| Arc::from([])),
            code: state.code.as_ref().map(|code| Arc::from(code.as_str())),
            iteration: state.iteration,
            artifact,
        }
    }

    pub fn code_str(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn artifact_id(&self) -> Option<&ArtifactId> {
        self.artifact.as_ref().map(RustArtifact::id)
    }
}

/// Sesión bridged: contexto del AgentLoop + artefactos del Constructor.
#[derive(Debug, Clone)]
pub struct BridgedSession {
    pub context: AgentContext,
    pub artifacts: ConstructorArtifacts,
}

/// Resultado de una sesión Bridge → AgentLoop (sin alterar el CodeState original).
#[derive(Debug, Clone)]
pub struct BridgeResult {
    pub artifacts: ConstructorArtifacts,
    pub loop_result: LoopResult,
}

/// Adaptador Constructor → AgentLoop.
///
/// No contiene lógica de reparación ni conoce detalles internos del AgentLoop
/// más allá de construir contexto y delegar en [`AgentLoop::run`].
pub struct ConstructorBridge;

impl ConstructorBridge {
    /// Convierte un [`CodeState`] en una sesión lista para el AgentLoop.
    pub fn session_from_state(state: &CodeState) -> BridgedSession {
        let artifacts = ConstructorArtifacts::snapshot(state);
        let goal = format!("bridge:{}", artifacts.request);
        let mut context = AgentContext::new(goal);
        context.record(format!("request:{}", artifacts.request));
        if let Some(kind) = &artifacts.plan_kind {
            context.record(format!("plan_kind:{kind:?}"));
        }
        if let Some(code) = artifacts.code_str() {
            context.record(format!("code_bytes:{}", code.len()));
        }
        if let Some(artifact) = artifacts.artifact.clone() {
            context.set_working_artifact(artifact);
        }
        BridgedSession { context, artifacts }
    }

    /// Ejecuta el AgentLoop sobre una sesión bridged.
    pub fn run_session(
        harness: &Harness,
        agent: &mut dyn Agent,
        session: BridgedSession,
        max_iterations: u32,
    ) -> BridgeResult {
        let loop_result = AgentLoop::new(max_iterations).run(harness, agent, session.context);
        BridgeResult {
            artifacts: session.artifacts,
            loop_result,
        }
    }
}

/// Introduce un defecto de compilación controlado sin mutar el artefacto original.
///
/// Inyecta un `println!` sin cerrar al inicio de `main`.
pub fn introduce_compile_defect(valid_code: &str) -> String {
    valid_code.replacen(
        "fn main() {\n",
        "fn main() {\n    println!(\"bridge-defect\"\n",
        1,
    )
}

/// Introduce un defecto de validación (plan Api) sin romper necesariamente el parseo Rust.
///
/// Elimina marcadores que el Validator exige para `PlanKind::Api`.
pub fn introduce_validation_defect(valid_api_code: &str) -> String {
    valid_api_code
        .replace("HTTP", "NET")
        .replace("Endpoints", "Routes")
        .replace("endpoint", "route")
        .replace("/api", "/x")
        .replace("GET", "READ")
        .replace("POST", "WRITE")
        .replace("Server", "Host")
        .replace("server", "host")
}

/// MockAgent bridged: Compile(broken) → Observation FAIL → Compile(fixed) → PASS → Finish.
///
/// La decisión de la segunda acción depende causalmente de la Observation.
pub struct BridgedCompileRepairAgent {
    broken_code: Arc<str>,
    fixed_code: Arc<str>,
    pub saw_compile_failure: bool,
}

impl BridgedCompileRepairAgent {
    pub fn new(broken_code: impl Into<Arc<str>>, fixed_code: impl Into<Arc<str>>) -> Self {
        Self {
            broken_code: broken_code.into(),
            fixed_code: fixed_code.into(),
            saw_compile_failure: false,
        }
    }

    pub fn from_valid_code(valid_code: &str) -> Self {
        let fixed: Arc<str> = Arc::from(valid_code);
        let broken: Arc<str> = Arc::from(introduce_compile_defect(valid_code));
        Self::new(broken, fixed)
    }
}

impl Agent for BridgedCompileRepairAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            None => AgentAction::Compile {
                code: self.broken_code.to_string(),
            },
            Some(AgentObservation::ToolOutcome {
                tool_name,
                success: false,
                ..
            }) if tool_name == COMPILE => {
                self.saw_compile_failure = true;
                AgentAction::Compile {
                    code: self.fixed_code.to_string(),
                }
            }
            Some(AgentObservation::ToolOutcome {
                tool_name,
                success: true,
                ..
            }) if tool_name == COMPILE => AgentAction::Finish {
                summary: "bridge compile repaired".to_string(),
            },
            Some(other) => AgentAction::Finish {
                summary: format!("bridge stop: {}", other.summary()),
            },
        }
    }
}

/// MockAgent: Validate(invalid) → RepairDiagnostic → ApplyCorrection → Validate → Compile → Finish.
pub struct BridgedValidateRepairAgent {
    request: Arc<str>,
    plan_kind: String,
    invalid_code: Arc<str>,
    policy: Box<dyn CorrectionPolicy>,
    pub saw_validation_failure: bool,
    pub saw_repair_diagnostic: bool,
    pub saw_apply_correction: bool,
    pub observed_feedback: Vec<String>,
    pub observed_errors: Vec<String>,
    pub proposed_corrections: Vec<crate::harness::Correction>,
    pub corrected_code: Option<String>,
}

impl BridgedValidateRepairAgent {
    pub fn new(
        request: impl Into<Arc<str>>,
        plan_kind: impl Into<String>,
        invalid_code: impl Into<Arc<str>>,
    ) -> Self {
        Self::with_policy(
            request,
            plan_kind,
            invalid_code,
            Box::new(DeterministicCorrectionPolicy::new()),
        )
    }

    pub fn with_policy(
        request: impl Into<Arc<str>>,
        plan_kind: impl Into<String>,
        invalid_code: impl Into<Arc<str>>,
        policy: Box<dyn CorrectionPolicy>,
    ) -> Self {
        Self {
            request: request.into(),
            plan_kind: plan_kind.into(),
            invalid_code: invalid_code.into(),
            policy,
            saw_validation_failure: false,
            saw_repair_diagnostic: false,
            saw_apply_correction: false,
            observed_feedback: Vec::new(),
            observed_errors: Vec::new(),
            proposed_corrections: Vec::new(),
            corrected_code: None,
        }
    }

    pub fn for_api(valid_code: &str) -> Self {
        let invalid: Arc<str> = Arc::from(introduce_validation_defect(valid_code));
        Self::new(Arc::from("Crear una API REST"), "Api", invalid)
    }
}

impl Agent for BridgedValidateRepairAgent {
    fn propose(&mut self, ctx: &AgentContext) -> AgentAction {
        match &ctx.last_observation {
            None => AgentAction::Validate {
                request: self.request.to_string(),
                code: Some(self.invalid_code.to_string()),
                plan_kind: self.plan_kind.clone(),
            },
            Some(
                obs @ AgentObservation::ToolOutcome {
                    tool_name,
                    success: false,
                    ..
                },
            ) if tool_name == VALIDATE => {
                let errors = obs.validator_errors();
                if errors.is_empty() {
                    return AgentAction::Finish {
                        summary: "fail: validation without errors".to_string(),
                    };
                }
                if !obs.repairer_feedback().is_empty() {
                    return AgentAction::Finish {
                        summary: "fail: validation must not include repair feedback".to_string(),
                    };
                }
                self.saw_validation_failure = true;
                self.observed_errors = errors.iter().map(|s| (*s).to_string()).collect();
                AgentAction::RepairDiagnostic {
                    errors: self.observed_errors.clone(),
                }
            }
            Some(
                obs @ AgentObservation::ToolOutcome {
                    tool_name,
                    success: true,
                    ..
                },
            ) if tool_name == REPAIR_DIAGNOSTIC => {
                let feedback = obs.repairer_feedback();
                if feedback.is_empty() {
                    return AgentAction::Finish {
                        summary: "fail: repair diagnostic without feedback".to_string(),
                    };
                }
                self.saw_repair_diagnostic = true;
                self.observed_feedback = feedback.iter().map(|s| (*s).to_string()).collect();
                let input = CorrectionPolicyInput::new(obs, ctx);
                let corrections = match self.policy.propose_corrections(&input) {
                    Ok(items) => items,
                    Err(error) => {
                        return AgentAction::Finish {
                            summary: format!("policy error: {error}"),
                        };
                    }
                };
                self.proposed_corrections = corrections.clone();
                AgentAction::ApplyCorrection { corrections }
            }
            Some(
                obs @ AgentObservation::ToolOutcome {
                    tool_name,
                    success: true,
                    ..
                },
            ) if tool_name == APPLY_CORRECTION => {
                let corrected = obs
                    .corrected_code()
                    .or_else(|| ctx.working_code())
                    .map(str::to_string);
                let Some(code) = corrected else {
                    return AgentAction::Finish {
                        summary: "fail: correction without corrected_code".to_string(),
                    };
                };
                self.saw_apply_correction = true;
                self.corrected_code = Some(code.clone());
                AgentAction::Validate {
                    request: self.request.to_string(),
                    code: Some(code),
                    plan_kind: self.plan_kind.clone(),
                }
            }
            Some(AgentObservation::ToolOutcome {
                tool_name,
                success: true,
                ..
            }) if tool_name == VALIDATE => {
                let code = self
                    .corrected_code
                    .clone()
                    .or_else(|| ctx.working_code().map(str::to_string))
                    .unwrap_or_default();
                AgentAction::Compile { code }
            }
            Some(AgentObservation::ToolOutcome {
                tool_name,
                success: true,
                ..
            }) if tool_name == COMPILE => AgentAction::Finish {
                summary: "bridge validate+compile repaired".to_string(),
            },
            Some(other) => AgentAction::Finish {
                summary: format!("bridge validate stop: {}", other.summary()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder;
    use crate::harness::Evidence;
    use crate::harness::ToolPermissionConstraint;
    use crate::harness::agent_loop::LoopStatus;
    use crate::harness::tools::{
        CompileTool, CorrectionTool, RepairDiagnosticTool, ValidationTool,
    };
    use crate::planner::{self, PlanKind};
    use crate::state::CodeState;

    fn state_for_api_request() -> CodeState {
        CodeState {
            request: "Crear una API REST".to_string(),
            plan: None,
            code: None,
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        }
    }

    fn build_valid_api_state() -> CodeState {
        let mut state = state_for_api_request();
        planner::plan(&mut state);
        state.iteration = 2;
        builder::build(&mut state);
        state
    }

    #[test]
    fn bridge_converts_code_state_into_agent_context() {
        // A
        let state = build_valid_api_state();
        let session = ConstructorBridge::session_from_state(&state);

        assert!(session.context.goal.contains("Crear una API REST"));
        assert!(
            session
                .context
                .observations
                .iter()
                .any(|o| o.starts_with("request:"))
        );
    }

    #[test]
    fn bridge_preserves_request_and_plan() {
        // B
        let state = build_valid_api_state();
        let session = ConstructorBridge::session_from_state(&state);

        assert_eq!(&*session.artifacts.request, "Crear una API REST");
        assert_eq!(session.artifacts.plan_kind, Some(PlanKind::Api));
        assert!(!session.artifacts.plan_steps.is_empty());
        assert_eq!(session.artifacts.iteration, 2);
    }

    #[test]
    fn bridge_exposes_code_for_tools() {
        // C
        let state = build_valid_api_state();
        let session = ConstructorBridge::session_from_state(&state);
        let code = session
            .artifacts
            .code_str()
            .expect("el Builder debe haber generado código");

        assert!(code.contains("crear_servidor"));
        assert!(code.contains("fn main()"));
    }

    #[test]
    fn bridge_end_to_end_compile_fail_observation_repair_pass() {
        // D–I: Compile FAIL → Observation → decisión distinta → Compile PASS → Finish
        let state = build_valid_api_state();
        let original_code = state.code.clone().expect("código válido del Constructor");

        // CodeState original no se muta con el defecto.
        let session = ConstructorBridge::session_from_state(&state);
        assert_eq!(session.artifacts.code_str(), Some(original_code.as_str()));

        let mut agent = BridgedCompileRepairAgent::from_valid_code(&original_code);
        let broken = introduce_compile_defect(&original_code);
        assert_ne!(broken, original_code);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let result = ConstructorBridge::run_session(&harness, &mut agent, session, 5);

        // D: FAIL llegó como Observation
        assert!(agent.saw_compile_failure);
        assert!(result.loop_result.history.observations.iter().any(|o| {
            matches!(
                o,
                AgentObservation::ToolOutcome {
                    tool_name,
                    success: false,
                    ..
                } if tool_name == COMPILE
            )
        }));

        // E: el Agent cambió de decisión (broken → fixed)
        assert!(result.loop_result.iterations >= 3);
        assert_ne!(
            result.loop_result.history.proposed_actions[0],
            result.loop_result.history.proposed_actions[1]
        );
        match (
            &result.loop_result.history.proposed_actions[0],
            &result.loop_result.history.proposed_actions[1],
        ) {
            (AgentAction::Compile { code: first }, AgentAction::Compile { code: second }) => {
                assert_eq!(first, &broken);
                assert_eq!(second, &original_code);
            }
            other => panic!("se esperaban dos Compile causales, got {other:?}"),
        }

        // F/G: segunda acción ejecutada y PASS
        let compile_successes: Vec<bool> = result
            .loop_result
            .history
            .observations
            .iter()
            .filter_map(|o| match o {
                AgentObservation::ToolOutcome {
                    tool_name, success, ..
                } if tool_name == COMPILE => Some(*success),
                _ => None,
            })
            .collect();
        assert_eq!(compile_successes.first().copied(), Some(false));
        assert_eq!(compile_successes.get(1).copied(), Some(true));
        assert_eq!(result.loop_result.history.tool_results.len(), 2);

        // H
        assert_eq!(result.loop_result.status, LoopStatus::Completed);

        // I: historial conserva ambas ejecuciones
        assert!(result.loop_result.history.steps.len() >= 3);
        assert_eq!(
            result
                .loop_result
                .history
                .steps
                .iter()
                .filter(|s| s.tool_executed)
                .count(),
            2
        );

        // El snapshot del Bridge conserva el código original del Constructor.
        assert_eq!(result.artifacts.code_str(), Some(original_code.as_str()));
        assert_eq!(result.artifacts.plan_kind, Some(PlanKind::Api));
    }

    #[test]
    fn bridge_creates_artifact_without_mutating_code_state() {
        // L
        let state = build_valid_api_state();
        let original = state.code.clone();
        let session = ConstructorBridge::session_from_state(&state);
        assert!(session.artifacts.artifact.is_some());
        assert!(session.artifacts.artifact_id().is_some());
        assert_eq!(
            session
                .context
                .working_artifact
                .as_ref()
                .map(|a| a.source()),
            original.as_deref()
        );
        assert_eq!(state.code, original);
        let mut artifact = session.artifacts.artifact.clone().unwrap();
        let id = artifact.id().clone();
        artifact.replace_source("fn main() { /* mutated in harness */ }");
        assert_eq!(artifact.id(), &id);
        assert_eq!(state.code, original);
    }

    #[test]
    fn bridge_does_not_mutate_original_code_state() {
        let state = build_valid_api_state();
        let before = format!("{:?}", state);
        let session = ConstructorBridge::session_from_state(&state);
        let _ = session;
        let after = format!("{:?}", state);
        assert_eq!(before, after);
        assert!(state.errors.is_empty());
        assert!(state.code.is_some());
    }

    #[test]
    fn bridge_session_exposes_validation_capable_artifacts() {
        // A
        let state = build_valid_api_state();
        let session = ConstructorBridge::session_from_state(&state);
        assert_eq!(session.artifacts.plan_kind, Some(PlanKind::Api));
        assert!(session.artifacts.code_str().is_some());
    }

    #[test]
    fn bridge_validation_feedback_causal_loop_completes() {
        // Validate FAIL → RepairDiagnostic → ApplyCorrection → Validate PASS → Compile PASS → Finish
        let state = build_valid_api_state();
        let original_code = state.code.clone().expect("código válido");
        let invalid_code = introduce_validation_defect(&original_code);
        assert_ne!(invalid_code, original_code);

        let session = ConstructorBridge::session_from_state(&state);
        let mut agent = BridgedValidateRepairAgent::for_api(&original_code);

        let mut harness = Harness::new(10);
        harness.register_tool(Box::new(ValidationTool));
        harness.register_tool(Box::new(RepairDiagnosticTool));
        harness.register_tool(Box::new(CorrectionTool));
        harness.register_tool(Box::new(CompileTool));
        harness.register_constraint(Box::new(
            ToolPermissionConstraint::default_constructor_tools(),
        ));

        let result = ConstructorBridge::run_session(&harness, &mut agent, session, 10);

        // ValidationTool solamente valida (errors, sin feedback)
        let first_obs = result
            .loop_result
            .history
            .observations
            .first()
            .expect("debe existir Observation de validate");
        assert!(first_obs.is_validation_outcome());
        assert!(!first_obs.validator_errors().is_empty());
        assert!(first_obs.repairer_feedback().is_empty());

        // RepairDiagnostic en paso separado
        let diagnostic_obs = result
            .loop_result
            .history
            .observations
            .iter()
            .find(|o| o.is_repair_diagnostic_outcome())
            .expect("debe existir Observation de repair_diagnostic");
        assert!(diagnostic_obs.is_repair_diagnostic_outcome());
        assert!(!diagnostic_obs.repairer_feedback().is_empty());
        assert!(diagnostic_obs.validator_errors().is_empty());

        // ApplyCorrection modifica el código
        let correction_obs = result
            .loop_result
            .history
            .observations
            .iter()
            .find(|o| o.is_correction_outcome())
            .expect("debe existir Observation de apply_correction");
        assert!(correction_obs.is_correction_outcome());
        let corrected = correction_obs
            .corrected_code()
            .expect("corrected_code en evidence");
        assert_ne!(corrected, invalid_code.as_str());
        assert_eq!(corrected, original_code.as_str());

        assert!(agent.saw_validation_failure);
        assert!(agent.saw_repair_diagnostic);
        assert!(agent.saw_apply_correction);
        assert!(!agent.observed_feedback.is_empty());
        assert!(!agent.observed_errors.is_empty());
        assert!(!agent.proposed_corrections.is_empty());

        // Secuencia causal en acciones
        assert!(result.loop_result.iterations >= 6);
        assert!(matches!(
            result.loop_result.history.proposed_actions[0],
            AgentAction::Validate { .. }
        ));
        assert!(matches!(
            result.loop_result.history.proposed_actions[1],
            AgentAction::RepairDiagnostic { .. }
        ));
        assert!(matches!(
            result.loop_result.history.proposed_actions[2],
            AgentAction::ApplyCorrection { .. }
        ));
        assert!(matches!(
            result.loop_result.history.proposed_actions[3],
            AgentAction::Validate { code: Some(_), .. }
        ));
        assert!(matches!(
            result.loop_result.history.proposed_actions[4],
            AgentAction::Compile { .. }
        ));

        match (
            &result.loop_result.history.proposed_actions[0],
            &result.loop_result.history.proposed_actions[3],
        ) {
            (
                AgentAction::Validate {
                    code: Some(first), ..
                },
                AgentAction::Validate {
                    code: Some(second), ..
                },
            ) => {
                assert_eq!(first, &invalid_code);
                assert_eq!(second, &original_code);
            }
            other => panic!("se esperaban Validate invalido y Validate corregido, got {other:?}"),
        }

        let validate_successes: Vec<bool> = result
            .loop_result
            .history
            .observations
            .iter()
            .filter_map(|o| match o {
                AgentObservation::ToolOutcome {
                    tool_name, success, ..
                } if tool_name == VALIDATE => Some(*success),
                _ => None,
            })
            .collect();
        assert_eq!(validate_successes.first().copied(), Some(false));
        assert_eq!(validate_successes.get(1).copied(), Some(true));

        assert!(result.loop_result.history.observations.iter().any(|o| {
            matches!(
                o,
                AgentObservation::ToolOutcome {
                    tool_name,
                    success: true,
                    ..
                } if tool_name == COMPILE
            )
        }));

        assert_eq!(result.loop_result.status, LoopStatus::Completed);
        assert!(result.loop_result.history.tool_results.len() >= 5);
        assert_eq!(result.artifacts.code_str(), Some(original_code.as_str()));
    }

    #[test]
    fn bridge_agent_decides_apply_correction_from_feedback() {
        let state = build_valid_api_state();
        let original = state.code.clone().unwrap();
        let invalid = introduce_validation_defect(&original);
        let mut agent = BridgedValidateRepairAgent::for_api(&original);

        let mut ctx = AgentContext::new("repair-feedback");
        ctx.update_working_source(invalid.clone());
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: VALIDATE.to_string(),
            success: false,
            output: "fail".to_string(),
            evidence: vec![Evidence::new(
                "validator_error_0",
                "El código no contiene la implementación esperada de API REST",
            )],
            verdict: crate::harness::EvaluationVerdict::Fail,
        });
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: REPAIR_DIAGNOSTIC.to_string(),
            success: true,
            output: "feedback".to_string(),
            evidence: vec![Evidence::new(
                "repairer_feedback_0",
                "Analizar y corregir el siguiente error: API REST",
            )],
            verdict: crate::harness::EvaluationVerdict::Pass,
        });

        let action = agent.propose(&ctx);
        assert!(matches!(
            action,
            AgentAction::ApplyCorrection { corrections } if !corrections.is_empty()
        ));
        assert!(agent.saw_repair_diagnostic);
        assert!(!agent.proposed_corrections.is_empty());
    }

    #[test]
    fn bridge_agent_requests_repair_diagnostic_after_validation_fail() {
        let state = build_valid_api_state();
        let original = state.code.clone().unwrap();
        let mut agent = BridgedValidateRepairAgent::for_api(&original);

        let mut ctx = AgentContext::new("validation-fail");
        ctx.push_observation(AgentObservation::ToolOutcome {
            tool_name: VALIDATE.to_string(),
            success: false,
            output: "fail".to_string(),
            evidence: vec![Evidence::new(
                "validator_error_0",
                "El código no contiene la implementación esperada de API REST",
            )],
            verdict: crate::harness::EvaluationVerdict::Fail,
        });

        let action = agent.propose(&ctx);
        assert!(matches!(
            action,
            AgentAction::RepairDiagnostic { errors } if !errors.is_empty()
        ));
        assert!(agent.saw_validation_failure);
        assert!(!agent.saw_repair_diagnostic);
    }
}
