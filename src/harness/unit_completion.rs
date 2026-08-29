//! Protocolo mínimo de cierre de unidad autónoma de ingeniería.
//!
//! Ortogonal al ciclo Goal/AgentLoop de construcción de software:
//! modela la distinción entre éxito técnico y unidad cerrada con informe.
//!
//! **Límite arquitectónico:** `FINAL_REPORT_DELIVERED` solo puede afirmarse
//! tras un reconocimiento explícito de entrega externa (p. ej. runtime Cursor).
//! Este módulo **no** envía mensajes al chat ni simula esa entrega.

/// Fase observable del protocolo de cierre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitCompletionPhase {
    InProgress,
    TechnicalWorkCompleted,
    AwaitingTerminalVerification,
    TerminalVerified,
    FinalReportBuilt,
    FinalReportDelivered,
    FailedToComplete,
    BlockedWithEvidence,
}

impl UnitCompletionPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::TechnicalWorkCompleted => "technical_work_completed",
            Self::AwaitingTerminalVerification => "awaiting_terminal_verification",
            Self::TerminalVerified => "terminal_verified",
            Self::FinalReportBuilt => "final_report_built",
            Self::FinalReportDelivered => "final_report_delivered",
            Self::FailedToComplete => "failed_to_complete",
            Self::BlockedWithEvidence => "blocked_with_evidence",
        }
    }
}

/// Estado terminal verificable de una unidad (no del AgentLoop de construcción).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitTerminalStatus {
    PostMergeVerified,
    PostMergeVerificationFailed,
    BlockedWithEvidence,
    NoChangeRequiredWithEvidence,
    IncompleteOrUnreportedTermination,
}

impl UnitTerminalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PostMergeVerified => "POST_MERGE_VERIFIED",
            Self::PostMergeVerificationFailed => "POST_MERGE_VERIFICATION_FAILED",
            Self::BlockedWithEvidence => "BLOCKED_WITH_EVIDENCE",
            Self::NoChangeRequiredWithEvidence => "NO_CHANGE_REQUIRED_WITH_EVIDENCE",
            Self::IncompleteOrUnreportedTermination => "INCOMPLETE_OR_UNREPORTED_TERMINATION",
        }
    }
}

/// Error de transición ilegal del protocolo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitCompletionTransitionError {
    pub message: String,
}

impl UnitCompletionTransitionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Registro mutable del protocolo de cierre.
///
/// Invariante:
/// `unit_completed()` ⟺ `terminal_state_verified && final_report_delivered`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitCompletionRecord {
    phase: UnitCompletionPhase,
    technical_work_completed: bool,
    terminal_state_reached: bool,
    terminal_state_verified: bool,
    final_report_built: bool,
    final_report_delivered: bool,
    terminal_status: Option<UnitTerminalStatus>,
    evidence_note: String,
}

impl UnitCompletionRecord {
    pub fn new() -> Self {
        Self {
            phase: UnitCompletionPhase::InProgress,
            technical_work_completed: false,
            terminal_state_reached: false,
            terminal_state_verified: false,
            final_report_built: false,
            final_report_delivered: false,
            terminal_status: None,
            evidence_note: String::new(),
        }
    }

    pub fn phase(&self) -> UnitCompletionPhase {
        self.phase
    }

    pub fn technical_work_completed(&self) -> bool {
        self.technical_work_completed
    }

    pub fn terminal_state_reached(&self) -> bool {
        self.terminal_state_reached
    }

    pub fn terminal_state_verified(&self) -> bool {
        self.terminal_state_verified
    }

    pub fn final_report_built(&self) -> bool {
        self.final_report_built
    }

    pub fn final_report_delivered(&self) -> bool {
        self.final_report_delivered
    }

    pub fn terminal_status(&self) -> Option<UnitTerminalStatus> {
        self.terminal_status
    }

    pub fn evidence_note(&self) -> &str {
        &self.evidence_note
    }

    /// Cierre de unidad: requiere verificación terminal **y** entrega del informe.
    pub fn unit_completed(&self) -> bool {
        self.terminal_state_verified && self.final_report_delivered
    }

    /// Éxito técnico ≠ cierre de unidad.
    pub fn technical_success_without_unit_close(&self) -> bool {
        self.technical_work_completed && !self.unit_completed()
    }

    /// Informe construido pero aún no entregado al usuario.
    pub fn report_built_but_not_delivered(&self) -> bool {
        self.final_report_built && !self.final_report_delivered
    }

    pub fn mark_technical_work_completed(
        &mut self,
        note: impl Into<String>,
    ) -> Result<(), UnitCompletionTransitionError> {
        if matches!(
            self.phase,
            UnitCompletionPhase::FailedToComplete | UnitCompletionPhase::FinalReportDelivered
        ) {
            return Err(UnitCompletionTransitionError::new(
                "no se puede marcar trabajo técnico tras fallo terminal o entrega",
            ));
        }
        self.technical_work_completed = true;
        self.evidence_note = note.into();
        self.phase = UnitCompletionPhase::TechnicalWorkCompleted;
        Ok(())
    }

    pub fn mark_awaiting_terminal_verification(
        &mut self,
    ) -> Result<(), UnitCompletionTransitionError> {
        if !self.technical_work_completed
            && !matches!(self.phase, UnitCompletionPhase::BlockedWithEvidence)
        {
            return Err(UnitCompletionTransitionError::new(
                "requiere trabajo técnico completado o bloqueo con evidencia",
            ));
        }
        self.phase = UnitCompletionPhase::AwaitingTerminalVerification;
        Ok(())
    }

    /// Verifica un estado terminal alcanzable con entrega posterior de informe.
    pub fn verify_terminal(
        &mut self,
        status: UnitTerminalStatus,
        note: impl Into<String>,
    ) -> Result<(), UnitCompletionTransitionError> {
        let phase = match status {
            UnitTerminalStatus::IncompleteOrUnreportedTermination => {
                return Err(UnitCompletionTransitionError::new(
                    "IncompleteOrUnreportedTermination no se verifica como terminal cerrado; usar mark_incomplete_unreported",
                ));
            }
            UnitTerminalStatus::BlockedWithEvidence => UnitCompletionPhase::BlockedWithEvidence,
            UnitTerminalStatus::PostMergeVerificationFailed => {
                UnitCompletionPhase::FailedToComplete
            }
            UnitTerminalStatus::PostMergeVerified
            | UnitTerminalStatus::NoChangeRequiredWithEvidence => {
                UnitCompletionPhase::TerminalVerified
            }
        };

        self.terminal_status = Some(status);
        self.terminal_state_reached = true;
        self.terminal_state_verified = true;
        self.evidence_note = note.into();
        self.phase = phase;
        Ok(())
    }

    /// Construye el informe final a partir de evidencia (aún no entregado).
    pub fn build_final_report(
        &mut self,
        note: impl Into<String>,
    ) -> Result<(), UnitCompletionTransitionError> {
        if !self.terminal_state_verified {
            return Err(UnitCompletionTransitionError::new(
                "no se puede construir informe sin terminal verificado",
            ));
        }
        if self.final_report_delivered {
            return Err(UnitCompletionTransitionError::new(
                "informe ya entregado; no reconstruir en frío",
            ));
        }
        self.final_report_built = true;
        self.evidence_note = note.into();
        self.phase = UnitCompletionPhase::FinalReportBuilt;
        Ok(())
    }

    /// Reconoce entrega **externa** visible (runtime/orquestador). No simula chat.
    ///
    /// El caller afirma que el informe ya fue emitido como respuesta visible.
    pub fn acknowledge_external_report_delivery(
        &mut self,
        note: impl Into<String>,
    ) -> Result<(), UnitCompletionTransitionError> {
        if !self.final_report_built {
            return Err(UnitCompletionTransitionError::new(
                "no se puede entregar un informe no construido",
            ));
        }
        if !self.terminal_state_verified {
            return Err(UnitCompletionTransitionError::new(
                "entrega requiere terminal verificado",
            ));
        }
        self.final_report_delivered = true;
        self.evidence_note = note.into();
        self.phase = UnitCompletionPhase::FinalReportDelivered;
        Ok(())
    }

    /// Caso E: terminación sin informe entregado (SILENT_TERMINATION modelado).
    pub fn mark_incomplete_unreported(
        &mut self,
        note: impl Into<String>,
    ) -> Result<(), UnitCompletionTransitionError> {
        self.terminal_status = Some(UnitTerminalStatus::IncompleteOrUnreportedTermination);
        self.terminal_state_reached = true;
        self.terminal_state_verified = true;
        self.final_report_built = false;
        self.final_report_delivered = false;
        self.evidence_note = note.into();
        self.phase = UnitCompletionPhase::FailedToComplete;
        Ok(())
    }

    pub fn summary(&self) -> String {
        format!(
            "phase={} technical={} terminal_reached={} terminal_verified={} report_built={} report_delivered={} unit_completed={} status={} note={}",
            self.phase.as_str(),
            self.technical_work_completed,
            self.terminal_state_reached,
            self.terminal_state_verified,
            self.final_report_built,
            self.final_report_delivered,
            self.unit_completed(),
            self.terminal_status.map(|s| s.as_str()).unwrap_or("none"),
            self.evidence_note,
        )
    }
}

impl Default for UnitCompletionRecord {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod unit_completion_tests {
    use super::*;

    /// 1. Technical success + report delivered → UNIT completed.
    #[test]
    fn case_technical_success_and_report_delivered() {
        let mut record = UnitCompletionRecord::new();
        record
            .mark_technical_work_completed("tests+ci+merge")
            .expect("tech");
        record.mark_awaiting_terminal_verification().expect("await");
        record
            .verify_terminal(UnitTerminalStatus::PostMergeVerified, "master ok")
            .expect("verify");
        record
            .build_final_report("informe construido")
            .expect("build");
        assert!(record.report_built_but_not_delivered());
        assert!(!record.unit_completed());
        record
            .acknowledge_external_report_delivery("emitido en chat")
            .expect("deliver");

        assert!(record.final_report_delivered());
        assert!(record.unit_completed());
        assert_eq!(
            record.terminal_status(),
            Some(UnitTerminalStatus::PostMergeVerified)
        );
        assert_eq!(record.phase(), UnitCompletionPhase::FinalReportDelivered);
    }

    /// 2. Technical success + report not delivered → UNIT incomplete.
    #[test]
    fn case_technical_success_report_not_delivered() {
        let mut record = UnitCompletionRecord::new();
        record
            .mark_technical_work_completed("pr merged")
            .expect("tech");
        record
            .verify_terminal(UnitTerminalStatus::PostMergeVerified, "post-merge ok")
            .expect("verify");
        record.build_final_report("built only").expect("build");

        assert!(record.technical_work_completed());
        assert!(record.final_report_built());
        assert!(!record.final_report_delivered());
        assert!(!record.unit_completed());
        assert!(record.technical_success_without_unit_close());
        assert!(record.report_built_but_not_delivered());
    }

    /// 3. Post-merge failure + report delivered.
    #[test]
    fn case_post_merge_failure_with_report_delivered() {
        let mut record = UnitCompletionRecord::new();
        record
            .mark_technical_work_completed("merged but verify failed")
            .expect("tech");
        record
            .verify_terminal(
                UnitTerminalStatus::PostMergeVerificationFailed,
                "cargo test fail on master",
            )
            .expect("verify");
        record
            .build_final_report("informe del fallo")
            .expect("build");
        record
            .acknowledge_external_report_delivery("chat")
            .expect("deliver");

        assert_eq!(
            record.terminal_status(),
            Some(UnitTerminalStatus::PostMergeVerificationFailed)
        );
        assert!(record.final_report_delivered());
        assert!(record.unit_completed());
        assert_eq!(record.phase(), UnitCompletionPhase::FinalReportDelivered);
    }

    /// 4. Blocked + report delivered.
    #[test]
    fn case_blocked_with_report_delivered() {
        let mut record = UnitCompletionRecord::new();
        record
            .verify_terminal(
                UnitTerminalStatus::BlockedWithEvidence,
                "missing credentials",
            )
            .expect("verify");
        assert_eq!(record.phase(), UnitCompletionPhase::BlockedWithEvidence);
        record.build_final_report("blocked report").expect("build");
        record
            .acknowledge_external_report_delivery("chat")
            .expect("deliver");

        assert_eq!(
            record.terminal_status(),
            Some(UnitTerminalStatus::BlockedWithEvidence)
        );
        assert!(record.final_report_delivered());
        assert!(record.unit_completed());
    }

    /// 5. No change required + report delivered.
    #[test]
    fn case_no_change_required_with_report_delivered() {
        let mut record = UnitCompletionRecord::new();
        record
            .mark_technical_work_completed("research: no gap")
            .expect("tech");
        record
            .verify_terminal(
                UnitTerminalStatus::NoChangeRequiredWithEvidence,
                "protocol already external",
            )
            .expect("verify");
        record
            .build_final_report("no-change report")
            .expect("build");
        record
            .acknowledge_external_report_delivery("chat")
            .expect("deliver");

        assert_eq!(
            record.terminal_status(),
            Some(UnitTerminalStatus::NoChangeRequiredWithEvidence)
        );
        assert!(record.unit_completed());
    }

    /// 6. Incomplete / unreported termination.
    #[test]
    fn case_incomplete_or_unreported_termination() {
        let mut record = UnitCompletionRecord::new();
        record
            .mark_technical_work_completed("pr merged silently")
            .expect("tech");
        record
            .mark_incomplete_unreported("agent stopped without chat report")
            .expect("incomplete");

        assert_eq!(
            record.terminal_status(),
            Some(UnitTerminalStatus::IncompleteOrUnreportedTermination)
        );
        assert!(!record.final_report_delivered());
        assert!(!record.final_report_built());
        assert!(!record.unit_completed());
        assert!(record.technical_success_without_unit_close());
        assert_ne!(
            record.terminal_status(),
            Some(UnitTerminalStatus::PostMergeVerified)
        );
    }

    #[test]
    fn rejects_fake_delivery_without_built_report() {
        let mut record = UnitCompletionRecord::new();
        record.mark_technical_work_completed("tech").expect("tech");
        record
            .verify_terminal(UnitTerminalStatus::PostMergeVerified, "ok")
            .expect("verify");
        let err = record
            .acknowledge_external_report_delivery("fake")
            .expect_err("must reject");
        assert!(err.message.contains("no construido"));
        assert!(!record.final_report_delivered());
        assert!(!record.unit_completed());
    }

    #[test]
    fn rejects_verify_incomplete_as_closed_terminal() {
        let mut record = UnitCompletionRecord::new();
        let err = record
            .verify_terminal(UnitTerminalStatus::IncompleteOrUnreportedTermination, "no")
            .expect_err("must reject");
        assert!(err.message.contains("mark_incomplete_unreported"));
    }

    #[test]
    fn invariant_unit_completed_requires_both_flags() {
        let mut record = UnitCompletionRecord::new();
        assert!(!record.unit_completed());
        record.mark_technical_work_completed("t").expect("tech");
        record
            .verify_terminal(UnitTerminalStatus::PostMergeVerified, "v")
            .expect("v");
        assert!(record.terminal_state_verified());
        assert!(!record.unit_completed());
        record.build_final_report("r").expect("b");
        assert!(!record.unit_completed());
        record.acknowledge_external_report_delivery("d").expect("d");
        assert!(record.unit_completed());
    }
}
