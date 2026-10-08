use crate::executor::{ExecutionResult, LogRecord};
use crate::manifest::{CaseManifest, CorpusKind, ExpectedOutcome};
use crate::manual_oracle::ManualOracle;
use crate::report::OracleKind;

/// Exact log label used by the official W3C pass/fail oracle.
pub const OUTCOME_LABEL: &str = "Outcome";

/// Identifier of the top-level `<final>` state that the W3C IRP templates
/// generate for the `conf:pass` success criterion
/// (`tests/conformance/w3c-irp/templates/ecmascript.xsl:18-30` and
/// `xpath.xsl:18-30`: under "Success criteria" both rewrite `conf:pass` to
/// `<final id="pass">` whose `onentry` logs the `Outcome`).
pub const PASS_FINAL_STATE: &str = "pass";

#[derive(Clone, Debug, Eq, PartialEq)]
/// A deterministic mismatch between expected and observed execution results.
pub struct OracleFailure {
    /// Stable machine-readable mismatch code.
    pub code: &'static str,
    /// Human-readable mismatch description.
    pub message: String,
}

/// Evaluates an execution using the oracle permitted by its corpus provenance
/// and returns the kind of oracle that decided it.
///
/// Official IRP cases use the exact pass-only W3C log protocol, independent
/// of any unvalidated in-memory case expectation, except the documented
/// manual cases selected by [`ManualOracle::for_case`], which use their
/// automated manual-equivalent oracle. Synthetic cases may use only the
/// explicitly non-official expectation variants.
///
/// # Errors
///
/// Returns [`OracleFailure`] when the observed execution does not satisfy the
/// provenance-specific oracle or a synthetic case claims the official oracle.
pub fn evaluate_for_corpus(
    corpus_kind: CorpusKind,
    case: &CaseManifest,
    actual: &ExecutionResult,
) -> Result<OracleKind, OracleFailure> {
    match corpus_kind {
        CorpusKind::OfficialIrp => match ManualOracle::for_case(case) {
            Some(manual) => manual
                .evaluate(actual)
                .map(|()| OracleKind::ManualEquivalent),
            None => evaluate_outcome_logs(&actual.logs)
                .and_then(|()| evaluate_pass_termination(actual))
                .map(|()| OracleKind::OutcomeLog),
        },
        // A namespace suite's executed cases use exactly the W3C `Outcome`
        // protocol over the same templates; its rejection cases never run
        // (`evaluate_document_rejection`).
        CorpusKind::NamespaceIrp => match &case.expected {
            ExpectedOutcome::W3cLogOutcome { .. } => evaluate_outcome_logs(&actual.logs)
                .and_then(|()| evaluate_pass_termination(actual))
                .map(|()| OracleKind::OutcomeLog),
            _ => Err(OracleFailure {
                code: "namespace-case-oracle",
                message: "an executed namespace case is decided only by the Outcome log".into(),
            }),
        },
        CorpusKind::SyntheticHarness => match &case.expected {
            ExpectedOutcome::W3cLogOutcome { .. } => Err(OracleFailure {
                code: "synthetic-official-oracle",
                message: "synthetic harness cases cannot claim the official IRP log oracle".into(),
            }),
            expected => {
                evaluate_synthetic(expected, actual).map(|()| OracleKind::SyntheticExpectation)
            }
        },
    }
}

/// Returns the oracle kind that decides `case` under `corpus_kind`, the same
/// selection [`evaluate_for_corpus`] applies.
#[must_use]
pub fn oracle_kind_for_corpus(corpus_kind: CorpusKind, case: &CaseManifest) -> OracleKind {
    match corpus_kind {
        CorpusKind::OfficialIrp if ManualOracle::for_case(case).is_some() => {
            OracleKind::ManualEquivalent
        }
        CorpusKind::OfficialIrp => OracleKind::OutcomeLog,
        CorpusKind::NamespaceIrp => match case.expected {
            ExpectedOutcome::DocumentRejection { .. } => OracleKind::DocumentRejection,
            _ => OracleKind::OutcomeLog,
        },
        CorpusKind::SyntheticHarness => OracleKind::SyntheticExpectation,
    }
}

fn evaluate_synthetic(
    expected: &ExpectedOutcome,
    actual: &ExecutionResult,
) -> Result<(), OracleFailure> {
    match expected {
        ExpectedOutcome::FinalState { state } => {
            let is_finished = actual.terminal_status == crate::report::TerminalStatus::Finished;
            if is_finished && actual.configuration == [state.clone()] {
                Ok(())
            } else {
                Err(OracleFailure {
                    code: "final-state-mismatch",
                    message: format!(
                        "expected finished in `{state}`, got status {:?} configuration {:?}",
                        actual.terminal_status, actual.configuration
                    ),
                })
            }
        }
        ExpectedOutcome::ActiveConfiguration { states } => {
            let mut expected = states.clone();
            expected.sort();
            expected.dedup();
            let mut actual_states = actual.configuration.clone();
            actual_states.sort();
            actual_states.dedup();
            if expected == actual_states {
                Ok(())
            } else {
                Err(OracleFailure {
                    code: "configuration-mismatch",
                    message: format!("expected configuration {expected:?}, got {actual_states:?}"),
                })
            }
        }
        ExpectedOutcome::TerminalStatus { status } => {
            if status == &actual.terminal_status {
                Ok(())
            } else {
                Err(OracleFailure {
                    code: "terminal-status-mismatch",
                    message: format!(
                        "expected terminal status {status:?}, got {:?}",
                        actual.terminal_status
                    ),
                })
            }
        }
        ExpectedOutcome::W3cLogOutcome { .. } => Err(OracleFailure {
            code: "synthetic-official-oracle",
            message: "synthetic harness cases cannot claim the official IRP log oracle".into(),
        }),
        ExpectedOutcome::DocumentRejection { .. } => Err(OracleFailure {
            code: "synthetic-official-oracle",
            message: "synthetic harness cases cannot claim the namespace document-rejection oracle"
                .into(),
        }),
    }
}

/// The document-rejection oracle of a namespace suite: the canonical
/// compiler rejected the document (`rejected`) and the distinct codes of
/// its error diagnostics (`error_codes`) are exactly `expected`. Nothing ran.
///
/// # Errors
///
/// [`OracleFailure`] `document-accepted` when the compiler accepted the
/// document, or `rejection-codes-mismatch` when it rejected it with other
/// error codes.
pub fn evaluate_document_rejection<'a>(
    expected: &[String],
    rejected: bool,
    error_codes: impl IntoIterator<Item = &'a str>,
) -> Result<(), OracleFailure> {
    if !rejected {
        return Err(OracleFailure {
            code: "document-accepted",
            message: format!(
                "the namespace requires the document to be rejected with {expected:?}, but the canonical compiler accepted it"
            ),
        });
    }
    let actual: std::collections::BTreeSet<&str> = error_codes.into_iter().collect();
    let wanted: std::collections::BTreeSet<&str> = expected.iter().map(String::as_str).collect();
    if actual == wanted {
        Ok(())
    } else {
        Err(OracleFailure {
            code: "rejection-codes-mismatch",
            message: format!("expected the error codes {wanted:?}, got {actual:?}"),
        })
    }
}

/// Applies the exact-one-record W3C `Outcome` log oracle.
///
/// # Errors
///
/// Returns [`OracleFailure`] when no `Outcome` record exists, multiple records
/// exist, or the value is not exactly `pass`.
pub fn evaluate_outcome_logs(logs: &[LogRecord]) -> Result<(), OracleFailure> {
    {
        let outcomes = logs
            .iter()
            .filter(|record| record.label == OUTCOME_LABEL)
            .map(|record| record.value.as_str())
            .collect::<Vec<_>>();
        match outcomes.as_slice() {
            [] => Err(OracleFailure {
                code: "outcome-missing",
                message:
                    "machine reached a stable configuration or final state without logging Outcome"
                        .into(),
            }),
            ["pass"] => Ok(()),
            ["fail"] => Err(OracleFailure {
                code: "outcome-fail",
                message: "official Outcome log reported fail".into(),
            }),
            [_] => Err(OracleFailure {
                code: "outcome-invalid",
                message: "official Outcome log value must be exactly pass".into(),
            }),
            _ => Err(OracleFailure {
                code: "outcome-multiple",
                message: "machine emitted more than one Outcome log".into(),
            }),
        }
    }
}

/// Requires that a run whose logs satisfied [`evaluate_outcome_logs`]
/// actually terminated in the success-criterion state: the root session
/// finished (SCXML 1.0 section 3.7: entering a top-level `<final>` ends the
/// session) and its configuration is exactly the template's
/// [`PASS_FINAL_STATE`]. The IRP success criterion is reaching the
/// `conf:pass` final state, whose entry is what logs `Outcome` `pass`; a
/// document that logs the same record elsewhere and keeps running, or
/// finishes anywhere else, did not reach it.
///
/// # Errors
///
/// Returns [`OracleFailure`] with code `outcome-pass-not-final` when the
/// root session is not finished in exactly the `pass` final state.
pub fn evaluate_pass_termination(actual: &ExecutionResult) -> Result<(), OracleFailure> {
    let finished = actual.terminal_status == crate::report::TerminalStatus::Finished;
    if finished && actual.configuration == [PASS_FINAL_STATE] {
        Ok(())
    } else {
        Err(OracleFailure {
            code: "outcome-pass-not-final",
            message: format!(
                "Outcome pass was logged but the root session did not finish in the \
                 `{PASS_FINAL_STATE}` final state (status {:?}, configuration {:?})",
                actual.terminal_status, actual.configuration
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(label: &str, value: &str) -> LogRecord {
        LogRecord {
            label: label.into(),
            value: value.into(),
        }
    }

    fn execution(
        terminal_status: crate::report::TerminalStatus,
        configuration: &[&str],
        logs: Vec<LogRecord>,
    ) -> ExecutionResult {
        ExecutionResult {
            configuration: configuration
                .iter()
                .map(|&state| state.to_owned())
                .collect(),
            terminal_status,
            commands: 2,
            fuel_consumed: 0,
            logs,
            trace: crate::executor::ExecutionTrace::default(),
            load_rejection: None,
            snapshots: Vec::new(),
        }
    }

    /// How the W3C IRP decides a case.
    #[derive(Clone, Copy)]
    enum Checking {
        /// Automatically, from the run.
        Automated,
        /// By a person (the manifest's `manual` flag).
        Manual,
    }

    fn official_case(id: &str, checking: Checking) -> CaseManifest {
        let manual = matches!(checking, Checking::Manual);
        serde_json::from_value(serde_json::json!({
            "id": id,
            "assertion_id": id,
            "spec_num": "3.2",
            "spec_id": "#scxml",
            "conformance": "mandatory",
            "datamodel": "ecmascript",
            "start_uri": "x/test.txml",
            "manual": manual,
            "expected": {"kind": "w3c-log-outcome", "outcome": "pass"},
        }))
        .expect("test case manifest must decode")
    }

    #[test]
    fn outcome_oracle_passes_only_a_run_finished_in_the_pass_final_state() {
        use crate::report::TerminalStatus;
        let case = official_case("355", Checking::Automated);
        let pass = || vec![log("Outcome", "pass")];
        assert_eq!(
            evaluate_for_corpus(
                CorpusKind::OfficialIrp,
                &case,
                &execution(TerminalStatus::Finished, &["pass"], pass())
            ),
            Ok(OracleKind::OutcomeLog)
        );
        // Logged pass, then kept running in a non-final state (the audit's
        // edited 355), finished elsewhere, or finished in a configuration
        // that is not exactly the success-criterion state.
        for (status, configuration) in [
            (TerminalStatus::Running, &["s1"][..]),
            (TerminalStatus::Running, &["pass"][..]),
            (TerminalStatus::Finished, &["fail"][..]),
            (TerminalStatus::Finished, &["pass", "s1"][..]),
            (TerminalStatus::Finished, &[][..]),
        ] {
            assert_eq!(
                evaluate_for_corpus(
                    CorpusKind::OfficialIrp,
                    &case,
                    &execution(status, configuration, pass())
                )
                .expect_err("a pass log without termination in `pass` must not pass")
                .code,
                "outcome-pass-not-final",
                "{status:?} {configuration:?}"
            );
        }
        // The log verdict is reported first: a fail log stays outcome-fail.
        assert_eq!(
            evaluate_for_corpus(
                CorpusKind::OfficialIrp,
                &case,
                &execution(
                    TerminalStatus::Running,
                    &["s1"],
                    vec![log("Outcome", "fail")]
                )
            )
            .expect_err("a fail log never passes")
            .code,
            "outcome-fail"
        );
    }

    #[test]
    fn manual_oracles_keep_their_own_termination_semantics() {
        use crate::report::TerminalStatus;
        // Manual case 301 is decided by its manual-equivalent oracle, never
        // by the pass-state termination check: a rejected load with no log
        // at all is its passing observation.
        let case = official_case("301", Checking::Manual);
        let rejected = ExecutionResult {
            load_rejection: Some(crate::executor::LoadRejection::ScriptUnavailable),
            commands: 0,
            ..execution(TerminalStatus::Rejected, &[], Vec::new())
        };
        assert_eq!(
            evaluate_for_corpus(CorpusKind::OfficialIrp, &case, &rejected),
            Ok(OracleKind::ManualEquivalent)
        );
    }

    #[test]
    fn official_outcome_protocol_accepts_one_exact_pass() {
        assert_eq!(
            evaluate_outcome_logs(&[log("diagnostic", "x"), log("Outcome", "pass")]),
            Ok(())
        );
    }

    #[test]
    fn official_outcome_protocol_rejects_fail_missing_invalid_and_multiple() {
        assert_eq!(
            evaluate_outcome_logs(&[log("Outcome", "fail")])
                .unwrap_err()
                .code,
            "outcome-fail"
        );
        assert_eq!(
            evaluate_outcome_logs(&[]).unwrap_err().code,
            "outcome-missing"
        );
        assert_eq!(
            evaluate_outcome_logs(&[log("Outcome", "PASS")])
                .unwrap_err()
                .code,
            "outcome-invalid"
        );
        for hostile in [" pass", "pass ", "\tpass", "pass\n", " fail", "fail\r\n"] {
            assert_eq!(
                evaluate_outcome_logs(&[log("Outcome", hostile)])
                    .unwrap_err()
                    .code,
                "outcome-invalid",
                "Outcome value {hostile:?} must not be normalized"
            );
        }
        assert_eq!(
            evaluate_outcome_logs(&[log("Outcome", "pass"), log("Outcome", "pass")])
                .unwrap_err()
                .code,
            "outcome-multiple"
        );
    }

    #[test]
    fn a_rejection_passes_only_with_exactly_the_expected_codes() {
        let expected = ["EVENT003".to_owned()];
        assert_eq!(
            evaluate_document_rejection(&expected, true, ["EVENT003", "EVENT003"]),
            Ok(())
        );
        let failure = |rejected: bool, codes: &[&'static str]| {
            evaluate_document_rejection(&expected, rejected, codes.iter().copied())
                .expect_err("the oracle fails")
                .code
        };
        assert_eq!(failure(false, &[]), "document-accepted");
        assert_eq!(failure(true, &["EVENT004"]), "rejection-codes-mismatch");
        assert_eq!(
            failure(true, &["EVENT003", "EVENT004"]),
            "rejection-codes-mismatch"
        );
    }
}
