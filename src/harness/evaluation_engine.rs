//! Motor determinista de evaluación: Evidence → Evaluation (sin ejecutar Tools).
//!
//! "Generation proposes. Execution produces evidence. Evaluation determines satisfaction."

use crate::harness::criterion::CriterionKind;
use crate::harness::evaluation::{EvaluationVerdict, Evidence};
use crate::harness::specification::{
    AcceptanceCriterion, AcceptanceCriterionId, Specification, SpecificationId,
};
use crate::harness::tools::{CHECK_FORMAT, COMPILE, RUN_CLIPPY, RUN_TESTS, VALIDATE};

/// Evaluación de un único [`AcceptanceCriterion`] contra evidencia existente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionEvaluation {
    pub criterion_id: AcceptanceCriterionId,
    pub kind: CriterionKind,
    pub verdict: EvaluationVerdict,
    pub message: String,
    pub evidence_used: Vec<Evidence>,
}

/// Estado agregado de evaluar todos los criterios de una Specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecificationEvaluationStatus {
    Pass,
    Fail,
    InsufficientEvidence,
}

/// Evaluación agregada de una Specification completa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecificationEvaluation {
    pub specification_id: SpecificationId,
    pub status: SpecificationEvaluationStatus,
    pub criteria: Vec<CriterionEvaluation>,
    pub message: String,
}

/// Motor mínimo de evaluación basado en evidencia (sin LLM, shell ni Tools).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvaluationEngine;

impl EvaluationEngine {
    pub fn new() -> Self {
        Self
    }

    /// Evalúa un criterio contra evidencia preexistente usando [`AcceptanceCriterion::kind`].
    pub fn evaluate_criterion(
        &self,
        criterion: &AcceptanceCriterion,
        evidence: &[Evidence],
    ) -> CriterionEvaluation {
        let (verdict, message, evidence_used) = match criterion.kind {
            CriterionKind::Compile => {
                let (verdict, message, evidence_used) =
                    evaluate_tool_status(evidence, COMPILE, "compile_status", "ok", "compilación");
                (
                    verdict,
                    message,
                    attach_compile_diagnostics(evidence, verdict, evidence_used),
                )
            }
            CriterionKind::Validate => {
                evaluate_tool_status(evidence, VALIDATE, "validate_status", "ok", "validación")
            }
            CriterionKind::RunTests => evaluate_exit_status(evidence, RUN_TESTS, "tests"),
            CriterionKind::Clippy => evaluate_exit_status(evidence, RUN_CLIPPY, "clippy"),
            CriterionKind::CheckFormat => evaluate_exit_status(evidence, CHECK_FORMAT, "formato"),
            CriterionKind::Unknown => (
                EvaluationVerdict::InsufficientEvidence,
                format!(
                    "criterio {} (kind=unknown) no tiene semántica evaluable determinista",
                    criterion.id.as_str()
                ),
                relevant_evidence(evidence, &[]),
            ),
        };

        CriterionEvaluation {
            criterion_id: criterion.id.clone(),
            kind: criterion.kind,
            verdict,
            message,
            evidence_used,
        }
    }

    /// Evalúa todos los acceptance criteria de una Specification.
    pub fn evaluate_specification(
        &self,
        specification: &Specification,
        evidence: &[Evidence],
    ) -> SpecificationEvaluation {
        if specification.acceptance_criteria.is_empty() {
            return SpecificationEvaluation {
                specification_id: specification.id.clone(),
                status: SpecificationEvaluationStatus::InsufficientEvidence,
                criteria: Vec::new(),
                message: "specification sin acceptance criteria evaluables".to_string(),
            };
        }

        let criteria = specification
            .acceptance_criteria
            .iter()
            .map(|criterion| self.evaluate_criterion(criterion, evidence))
            .collect::<Vec<_>>();

        let status = aggregate_status(&criteria);
        let message = aggregate_message(status, &criteria);

        SpecificationEvaluation {
            specification_id: specification.id.clone(),
            status,
            criteria,
            message,
        }
    }
}

fn evaluate_tool_status(
    evidence: &[Evidence],
    tool_name: &str,
    status_label: &str,
    pass_detail: &str,
    domain: &str,
) -> (EvaluationVerdict, String, Vec<Evidence>) {
    if !tool_present(evidence, tool_name) {
        return (
            EvaluationVerdict::InsufficientEvidence,
            format!("sin evidencia de tool {tool_name} para evaluar {domain}"),
            Vec::new(),
        );
    }

    let used = relevant_evidence(evidence, &[tool_name, status_label]);
    // Último status gana: el historial acumula intentos fallidos previos.
    let status = evidence
        .iter()
        .rev()
        .find(|item| item.label == status_label)
        .map(|item| item.detail.as_str());

    match status {
        Some(detail) if detail == pass_detail => (
            EvaluationVerdict::Pass,
            format!("{domain} satisfactoria según {status_label}={pass_detail}"),
            used,
        ),
        Some(detail) => (
            EvaluationVerdict::Fail,
            format!("{domain} fallida según {status_label}={detail}"),
            used,
        ),
        None => (
            EvaluationVerdict::InsufficientEvidence,
            format!("tool {tool_name} presente pero falta {status_label}"),
            used,
        ),
    }
}

fn evaluate_exit_status(
    evidence: &[Evidence],
    tool_name: &str,
    domain: &str,
) -> (EvaluationVerdict, String, Vec<Evidence>) {
    if !tool_present(evidence, tool_name) {
        return (
            EvaluationVerdict::InsufficientEvidence,
            format!("sin evidencia de tool {tool_name} para evaluar {domain}"),
            Vec::new(),
        );
    }

    let used = relevant_evidence(evidence, &["exit_status"]);
    // Correlación causal: último exit_status perteneciente a una ejecución de esta Tool.
    let exit = latest_exit_status_for_tool(evidence, tool_name);

    match exit {
        Some("0") => (
            EvaluationVerdict::Pass,
            format!("{domain} satisfactorio según exit_status=0"),
            used,
        ),
        Some(detail) => (
            EvaluationVerdict::Fail,
            format!("{domain} fallido según exit_status={detail}"),
            used,
        ),
        None => (
            EvaluationVerdict::InsufficientEvidence,
            format!("tool {tool_name} presente pero falta exit_status"),
            used,
        ),
    }
}

/// Último `exit_status` emitido dentro de una ejecución `tool=<tool_name>`.
///
/// Recorre Evidence en orden: al ver `tool`, activa/desactiva el scope; solo
/// cuenta `exit_status` mientras el scope de `tool_name` está activo.
fn latest_exit_status_for_tool<'a>(evidence: &'a [Evidence], tool_name: &str) -> Option<&'a str> {
    let mut active = false;
    let mut last: Option<&str> = None;
    for item in evidence {
        if item.label == "tool" {
            active = item.detail == tool_name;
            continue;
        }
        if active && item.label == "exit_status" {
            last = Some(item.detail.as_str());
        }
    }
    last
}

fn tool_present(evidence: &[Evidence], tool_name: &str) -> bool {
    evidence
        .iter()
        .any(|item| item.label == "tool" && item.detail == tool_name)
}

const COMPILE_DIAGNOSTIC_LABELS: &[&str] =
    &["compiler_stderr", "spawn_error", "materialization_error"];

fn attach_compile_diagnostics(
    evidence: &[Evidence],
    verdict: EvaluationVerdict,
    mut evidence_used: Vec<Evidence>,
) -> Vec<Evidence> {
    if verdict != EvaluationVerdict::Fail {
        return evidence_used;
    }
    for label in COMPILE_DIAGNOSTIC_LABELS {
        if evidence_used.iter().any(|item| item.label == *label) {
            continue;
        }
        if let Some(item) = latest_evidence_label(evidence, label) {
            evidence_used.push(item);
        }
    }
    evidence_used
}

fn latest_evidence_label(evidence: &[Evidence], label: &str) -> Option<Evidence> {
    evidence
        .iter()
        .rev()
        .find(|item| item.label == label)
        .cloned()
}

fn relevant_evidence(evidence: &[Evidence], labels: &[&str]) -> Vec<Evidence> {
    if labels.is_empty() {
        return evidence.to_vec();
    }
    evidence
        .iter()
        .filter(|item| {
            item.label == "tool"
                || item.label == "artifact_id"
                || labels.iter().any(|label| item.label == *label)
        })
        .cloned()
        .collect()
}

fn aggregate_status(criteria: &[CriterionEvaluation]) -> SpecificationEvaluationStatus {
    if criteria
        .iter()
        .any(|item| item.verdict == EvaluationVerdict::Fail)
    {
        return SpecificationEvaluationStatus::Fail;
    }
    if criteria
        .iter()
        .any(|item| item.verdict == EvaluationVerdict::InsufficientEvidence)
    {
        return SpecificationEvaluationStatus::InsufficientEvidence;
    }
    if criteria
        .iter()
        .all(|item| item.verdict == EvaluationVerdict::Pass)
    {
        return SpecificationEvaluationStatus::Pass;
    }
    SpecificationEvaluationStatus::InsufficientEvidence
}

fn aggregate_message(
    status: SpecificationEvaluationStatus,
    criteria: &[CriterionEvaluation],
) -> String {
    match status {
        SpecificationEvaluationStatus::Pass => {
            format!("todos los {} criterios PASS", criteria.len())
        }
        SpecificationEvaluationStatus::Fail => {
            let failed = criteria
                .iter()
                .filter(|item| item.verdict == EvaluationVerdict::Fail)
                .map(|item| item.criterion_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("criterios FAIL: {failed}")
        }
        SpecificationEvaluationStatus::InsufficientEvidence => {
            let missing = criteria
                .iter()
                .filter(|item| item.verdict == EvaluationVerdict::InsufficientEvidence)
                .map(|item| item.criterion_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("evidencia insuficiente para: {missing}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::artifact::RustArtifact;
    use crate::harness::context::AgentContext;
    use crate::harness::specification::Requirement;
    use crate::harness::tool::{Tool, ToolResult};
    use crate::harness::tools::CompileTool;
    use crate::harness::tools::REPAIR_DIAGNOSTIC;
    use crate::harness::tools::ValidationTool;
    use crate::harness::tools::encode_validate_input;

    fn compile_pass_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
        ]
    }

    fn compile_fail_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "error"),
            Evidence::new("compiler_stderr", "error"),
        ]
    }

    fn validate_pass_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", VALIDATE),
            Evidence::new("validate_status", "ok"),
        ]
    }

    fn validate_fail_evidence() -> Vec<Evidence> {
        vec![
            Evidence::new("tool", VALIDATE),
            Evidence::new("validate_status", "error"),
            Evidence::new("validator_error_0", "missing api marker"),
        ]
    }

    fn sample_specification() -> Specification {
        Specification::new("spec-api-001", "Crear una API REST")
            .with_requirements(vec![Requirement::new(
                "req-compile",
                "El código debe compilar",
            )])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new(
                    "ac-compile-001",
                    "El código compila",
                    CriterionKind::Compile,
                )
                .satisfying([crate::harness::RequirementId::new("req-compile")]),
                AcceptanceCriterion::new(
                    "ac-validate-001",
                    "El código cumple validación",
                    CriterionKind::Validate,
                )
                .satisfying([crate::harness::RequirementId::new("req-compile")]),
            ])
    }

    #[test]
    fn evidence_can_represent_tool_outcome() {
        let evidence = compile_pass_evidence();
        assert!(tool_present(&evidence, COMPILE));
    }

    #[test]
    fn evaluation_identifies_acceptance_criterion_id() {
        let engine = EvaluationEngine::new();
        let criterion =
            AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile);
        let evaluation = engine.evaluate_criterion(&criterion, &compile_pass_evidence());
        assert_eq!(evaluation.criterion_id.as_str(), "ac-compile-001");
    }

    #[test]
    fn evidence_and_evaluation_are_distinct_concepts() {
        let evidence = compile_pass_evidence();
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile),
            &evidence,
        );
        assert_ne!(evidence[0].label, evaluation.message);
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn compile_pass_evidence_satisfies_compile_criterion() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile),
            &compile_pass_evidence(),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn compile_fail_evidence_produces_fail_evaluation() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile),
            &compile_fail_evidence(),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn compile_fail_evaluation_preserves_compiler_stderr_in_evidence_used() {
        let engine = EvaluationEngine::new();
        let stderr = "error[E0425]: cannot find value `broken` in this scope";
        let evidence = vec![
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "error"),
            Evidence::new("compiler_stderr", stderr),
        ];
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile),
            &evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
        assert!(
            evaluation
                .evidence_used
                .iter()
                .any(|item| item.label == "compiler_stderr" && item.detail.contains("broken")),
            "compiler_stderr debe fluir a evidence_used del criterio Compile"
        );
    }

    #[test]
    fn missing_evidence_does_not_produce_pass() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile),
            &[],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::InsufficientEvidence);
    }

    #[test]
    fn validation_pass_evidence_satisfies_validation_criterion() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-validate-001", "valida", CriterionKind::Validate),
            &validate_pass_evidence(),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn validation_fail_evidence_produces_fail_evaluation() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-validate-001", "valida", CriterionKind::Validate),
            &validate_fail_evidence(),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn multiple_criteria_evaluate_independently() {
        let engine = EvaluationEngine::new();
        let mut evidence = compile_pass_evidence();
        evidence.extend(validate_fail_evidence());

        let compile = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile),
            &evidence,
        );
        let validate = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-validate-001", "valida", CriterionKind::Validate),
            &evidence,
        );

        assert_eq!(compile.verdict, EvaluationVerdict::Pass);
        assert_eq!(validate.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn specification_evaluation_aggregates_multiple_criteria() {
        let engine = EvaluationEngine::new();
        let spec = sample_specification();
        let mut evidence = compile_pass_evidence();
        evidence.extend(validate_pass_evidence());

        let aggregated = engine.evaluate_specification(&spec, &evidence);
        assert_eq!(aggregated.criteria.len(), 2);
        assert_eq!(aggregated.specification_id.as_str(), "spec-api-001");
    }

    #[test]
    fn one_failed_criterion_produces_aggregate_fail() {
        let engine = EvaluationEngine::new();
        let spec = sample_specification();
        let mut evidence = compile_pass_evidence();
        evidence.extend(validate_fail_evidence());

        let aggregated = engine.evaluate_specification(&spec, &evidence);
        assert_eq!(aggregated.status, SpecificationEvaluationStatus::Fail);
    }

    #[test]
    fn all_pass_criteria_produce_aggregate_pass() {
        let engine = EvaluationEngine::new();
        let spec = sample_specification();
        let mut evidence = compile_pass_evidence();
        evidence.extend(validate_pass_evidence());

        let aggregated = engine.evaluate_specification(&spec, &evidence);
        assert_eq!(aggregated.status, SpecificationEvaluationStatus::Pass);
    }

    #[test]
    fn insufficient_evidence_produces_non_pass_aggregate() {
        let engine = EvaluationEngine::new();
        let spec = sample_specification();
        let aggregated = engine.evaluate_specification(&spec, &[]);
        assert_ne!(aggregated.status, SpecificationEvaluationStatus::Pass);
        assert_eq!(
            aggregated.status,
            SpecificationEvaluationStatus::InsufficientEvidence
        );
    }

    #[test]
    fn tool_result_success_is_not_automatic_criterion_pass() {
        let tool_result = ToolResult::success("ok", vec![Evidence::new("tool", REPAIR_DIAGNOSTIC)]);
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "compila", CriterionKind::Compile),
            &tool_result.evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::InsufficientEvidence);
    }

    #[test]
    fn evaluation_does_not_mutate_specification_or_artifact() {
        let spec = sample_specification();
        let artifact = RustArtifact::new("main.rs", "fn main() {}");
        let before_spec = spec.clone();
        let before_artifact = artifact.clone();

        let _ = EvaluationEngine::new().evaluate_specification(&spec, &compile_pass_evidence());

        assert_eq!(spec, before_spec);
        assert_eq!(artifact, before_artifact);
    }

    #[test]
    fn traceability_links_specification_criterion_and_evidence() {
        let engine = EvaluationEngine::new();
        let spec = sample_specification();
        let aggregated = engine.evaluate_specification(&spec, &compile_pass_evidence());
        let criterion_eval = &aggregated.criteria[0];
        assert_eq!(aggregated.specification_id, spec.id);
        assert_eq!(criterion_eval.criterion_id.as_str(), "ac-compile-001");
        assert!(
            criterion_eval
                .evidence_used
                .iter()
                .any(|e| e.label == "compile_status")
        );
    }

    #[test]
    fn compile_tool_chain_produces_pass_evaluation_without_llm() {
        let tool = CompileTool;
        let ctx = crate::harness::context::AgentContext::new("eval-chain")
            .with_working_code("fn main() {}");
        let result = tool.execute("", &ctx);
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new(
                "ac-compile-001",
                "El código compila",
                CriterionKind::Compile,
            ),
            &result.evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn validation_tool_chain_produces_fail_evaluation_from_evidence() {
        let tool = ValidationTool;
        let input = encode_validate_input("Crear una API REST", Some("fn main() {}"), "Api");
        let result = tool.execute(&input, &AgentContext::new("eval-chain"));
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new(
                "ac-validate-001",
                "Validación estructural",
                CriterionKind::Validate,
            ),
            &result.evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn arbitrary_id_with_compile_kind_is_evaluated_as_compile() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-anything", "compila", CriterionKind::Compile),
            &compile_pass_evidence(),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn compile_looking_id_with_validate_kind_is_evaluated_as_validate() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-compile-001", "valida", CriterionKind::Validate),
            &validate_pass_evidence(),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
        assert!(
            !evaluation.message.contains("compilación"),
            "no debe evaluar como compile: {}",
            evaluation.message
        );
        assert!(evaluation.message.contains("validación"));
    }

    #[test]
    fn changing_only_id_does_not_change_evaluation_result() {
        let engine = EvaluationEngine::new();
        let evidence = compile_pass_evidence();
        let a = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-001", "compila", CriterionKind::Compile),
            &evidence,
        );
        let b = engine.evaluate_criterion(
            &AcceptanceCriterion::new("totally-different-id", "compila", CriterionKind::Compile),
            &evidence,
        );
        assert_ne!(a.criterion_id, b.criterion_id);
        assert_eq!(a.verdict, b.verdict);
        assert_eq!(a.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn changing_only_kind_changes_evaluation_semantics() {
        let engine = EvaluationEngine::new();
        let mut evidence = compile_pass_evidence();
        evidence.extend(validate_fail_evidence());

        let as_compile = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-shared", "criterio", CriterionKind::Compile),
            &evidence,
        );
        let as_validate = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-shared", "criterio", CriterionKind::Validate),
            &evidence,
        );

        assert_eq!(as_compile.verdict, EvaluationVerdict::Pass);
        assert_eq!(as_validate.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn unknown_kind_produces_insufficient_evidence() {
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-001", "criterio libre", CriterionKind::Unknown),
            &compile_pass_evidence(),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::InsufficientEvidence);
    }

    #[test]
    fn same_kind_implies_same_evaluation_semantics_regardless_of_id() {
        let engine = EvaluationEngine::new();
        let evidence = compile_fail_evidence();
        let a = engine.evaluate_criterion(
            &AcceptanceCriterion::new("id-a", "x", CriterionKind::Compile),
            &evidence,
        );
        let b = engine.evaluate_criterion(
            &AcceptanceCriterion::new("id-b", "y", CriterionKind::Compile),
            &evidence,
        );
        assert_ne!(a.criterion_id.as_str(), b.criterion_id.as_str());
        assert_eq!(a.verdict, b.verdict);
        assert_eq!(a.verdict, EvaluationVerdict::Fail);
    }

    fn quality_evidence(tool: &str, exit: &str) -> Vec<Evidence> {
        vec![
            Evidence::new("tool", tool),
            Evidence::new("exit_status", exit),
        ]
    }

    #[test]
    fn run_tests_pass_with_favorable_evidence() {
        // A
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &quality_evidence(RUN_TESTS, "0"),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn run_tests_fail_with_unfavorable_evidence() {
        // B
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &quality_evidence(RUN_TESTS, "1"),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn run_tests_insufficient_without_evidence() {
        // C
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &[],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::InsufficientEvidence);
    }

    #[test]
    fn clippy_pass_with_favorable_evidence() {
        // D
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-clippy", "clippy", CriterionKind::Clippy),
            &quality_evidence(RUN_CLIPPY, "0"),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn clippy_fail_with_unfavorable_evidence() {
        // E
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-clippy", "clippy", CriterionKind::Clippy),
            &quality_evidence(RUN_CLIPPY, "1"),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn clippy_insufficient_without_evidence() {
        // F
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-clippy", "clippy", CriterionKind::Clippy),
            &[],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::InsufficientEvidence);
    }

    #[test]
    fn check_format_pass_with_favorable_evidence() {
        // G
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-fmt", "formato", CriterionKind::CheckFormat),
            &quality_evidence(CHECK_FORMAT, "0"),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn check_format_fail_with_unfavorable_evidence() {
        // H
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-fmt", "formato", CriterionKind::CheckFormat),
            &quality_evidence(CHECK_FORMAT, "1"),
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn check_format_insufficient_without_evidence() {
        // I
        let engine = EvaluationEngine::new();
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-fmt", "formato", CriterionKind::CheckFormat),
            &[],
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::InsufficientEvidence);
    }

    #[test]
    fn changing_run_tests_id_does_not_change_semantics() {
        // J
        let engine = EvaluationEngine::new();
        let evidence = quality_evidence(RUN_TESTS, "0");
        let a = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &evidence,
        );
        let b = engine.evaluate_criterion(
            &AcceptanceCriterion::new("totally-other-id", "tests", CriterionKind::RunTests),
            &evidence,
        );
        assert_ne!(a.criterion_id, b.criterion_id);
        assert_eq!(a.verdict, b.verdict);
        assert_eq!(a.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn same_run_tests_kind_with_different_ids_same_evaluation() {
        // K
        let engine = EvaluationEngine::new();
        let evidence = quality_evidence(RUN_TESTS, "1");
        let a = engine.evaluate_criterion(
            &AcceptanceCriterion::new("id-a", "x", CriterionKind::RunTests),
            &evidence,
        );
        let b = engine.evaluate_criterion(
            &AcceptanceCriterion::new("id-b", "y", CriterionKind::RunTests),
            &evidence,
        );
        assert_eq!(a.verdict, b.verdict);
        assert_eq!(a.verdict, EvaluationVerdict::Fail);
    }

    #[test]
    fn run_tests_history_fail_then_pass_evaluates_pass() {
        // L
        let engine = EvaluationEngine::new();
        let mut evidence = quality_evidence(RUN_TESTS, "1");
        evidence.extend(quality_evidence(RUN_TESTS, "0"));
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn clippy_history_fail_then_pass_evaluates_pass() {
        // M
        let engine = EvaluationEngine::new();
        let mut evidence = quality_evidence(RUN_CLIPPY, "1");
        evidence.extend(quality_evidence(RUN_CLIPPY, "0"));
        let evaluation = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-clippy", "clippy", CriterionKind::Clippy),
            &evidence,
        );
        assert_eq!(evaluation.verdict, EvaluationVerdict::Pass);
    }

    #[test]
    fn exit_status_is_scoped_to_tool_not_global_last() {
        // Interleaving: run_tests FAIL, luego compile/clippy PASS no debe “arreglar” RunTests.
        let engine = EvaluationEngine::new();
        let evidence = vec![
            Evidence::new("tool", RUN_TESTS),
            Evidence::new("exit_status", "1"),
            Evidence::new("tool", COMPILE),
            Evidence::new("compile_status", "ok"),
            Evidence::new("tool", RUN_CLIPPY),
            Evidence::new("exit_status", "0"),
        ];
        let tests = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-tests", "tests", CriterionKind::RunTests),
            &evidence,
        );
        let clippy = engine.evaluate_criterion(
            &AcceptanceCriterion::new("ac-clippy", "clippy", CriterionKind::Clippy),
            &evidence,
        );
        assert_eq!(tests.verdict, EvaluationVerdict::Fail);
        assert_eq!(clippy.verdict, EvaluationVerdict::Pass);
    }
}
