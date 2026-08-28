use crate::harness::artifact_file_operation::ArtifactFileOperation;
use crate::harness::correction::Correction;

/// Acción explícita y verificable que un agente puede proponer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    /// Compila un fragmento de código Rust mediante CompileTool.
    Compile { code: String },
    /// Ejecuta la suite de tests del workspace mediante TestTool.
    RunTests { filter: String },
    /// Ejecuta `cargo clippy -- -D warnings` mediante ClippyTool.
    RunClippy,
    /// Ejecuta `cargo fmt --check` mediante FmtTool.
    CheckFormat,
    /// Valida código/plan mediante ValidationTool (Validator real).
    Validate {
        request: String,
        code: Option<String>,
        plan_kind: String,
    },
    /// Analiza errores existentes y genera feedback diagnóstico (Repairer real).
    ///
    /// No repara código; solo produce diagnóstico a partir de `errors`.
    RepairDiagnostic { errors: Vec<String> },
    /// Aplica correcciones estructuradas al código de sesión mediante CorrectionTool.
    ///
    /// No reemplaza el código completo; cada [`Correction`] es una operación atómica.
    ApplyCorrection { corrections: Vec<Correction> },
    /// Modifica la estructura del Artifact (create/delete/rename files).
    ApplyFileOperations {
        operations: Vec<ArtifactFileOperation>,
    },
    /// Invoca una herramienta registrada por nombre (superficie controlada).
    InvokeTool { tool_name: String, input: String },
    /// Solicita terminar la ejecución con un resumen.
    Finish { summary: String },
    /// No hace nada en este paso.
    NoOp,
}

impl AgentAction {
    /// Nombre de herramienta asociado a la acción, si aplica.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            AgentAction::Compile { .. } => Some(crate::harness::tools::COMPILE),
            AgentAction::RunTests { .. } => Some(crate::harness::tools::RUN_TESTS),
            AgentAction::RunClippy => Some(crate::harness::tools::RUN_CLIPPY),
            AgentAction::CheckFormat => Some(crate::harness::tools::CHECK_FORMAT),
            AgentAction::Validate { .. } => Some(crate::harness::tools::VALIDATE),
            AgentAction::RepairDiagnostic { .. } => Some(crate::harness::tools::REPAIR_DIAGNOSTIC),
            AgentAction::ApplyCorrection { .. } => Some(crate::harness::tools::APPLY_CORRECTION),
            AgentAction::ApplyFileOperations { .. } => {
                Some(crate::harness::tools::APPLY_FILE_OPERATIONS)
            }
            AgentAction::InvokeTool { tool_name, .. } => Some(tool_name.as_str()),
            AgentAction::Finish { .. } | AgentAction::NoOp => None,
        }
    }
}
