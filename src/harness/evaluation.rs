/// Veredicto estructurado de una evaluación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationVerdict {
    Pass,
    Fail,
    /// Evidencia insuficiente para concluir satisfacción del criterio.
    InsufficientEvidence,
}

/// Evidencia observable asociada a una herramienta o evaluación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub label: String,
    pub detail: String,
    /// Artifact asociado cuando la Evidence proviene de una Tool sobre código de sesión.
    pub artifact_id: Option<crate::harness::artifact::ArtifactId>,
}

impl Evidence {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            artifact_id: None,
        }
    }

    pub fn with_artifact_id(mut self, artifact_id: crate::harness::artifact::ArtifactId) -> Self {
        self.artifact_id = Some(artifact_id);
        self
    }
}

/// Evaluación con veredicto y evidencia estructurada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub verdict: EvaluationVerdict,
    pub message: String,
    pub evidence: Vec<Evidence>,
}

impl Evaluation {
    pub fn pass(message: impl Into<String>, evidence: Vec<Evidence>) -> Self {
        Self {
            verdict: EvaluationVerdict::Pass,
            message: message.into(),
            evidence,
        }
    }

    pub fn fail(message: impl Into<String>, evidence: Vec<Evidence>) -> Self {
        Self {
            verdict: EvaluationVerdict::Fail,
            message: message.into(),
            evidence,
        }
    }

    pub fn insufficient_evidence(message: impl Into<String>, evidence: Vec<Evidence>) -> Self {
        Self {
            verdict: EvaluationVerdict::InsufficientEvidence,
            message: message.into(),
            evidence,
        }
    }

    pub fn is_pass(&self) -> bool {
        matches!(self.verdict, EvaluationVerdict::Pass)
    }

    pub fn is_fail(&self) -> bool {
        matches!(self.verdict, EvaluationVerdict::Fail)
    }

    pub fn is_insufficient_evidence(&self) -> bool {
        matches!(self.verdict, EvaluationVerdict::InsufficientEvidence)
    }
}
