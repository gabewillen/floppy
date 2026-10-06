//! Documented manual-equivalent oracles for official W3C IRP manual cases.
//!
//! The W3C manifest marks these mandatory cases `manual="true"`: their pass
//! condition is a human inspecting logged output or observing that the
//! processor rejected the document, so they never log an `Outcome` of
//! `pass`. Each case below has one automated oracle that checks exactly what
//! the W3C instruction asks the tester to verify, over the session-attributed
//! [`ExecutionTrace`](crate::executor::ExecutionTrace) of the real engine run (or, for a document the
//! processor must reject, over the observable load rejection). Every other
//! official case, manual or not, keeps the exact `Outcome` oracle
//! ([`crate::oracle::evaluate_outcome_logs`]); these oracles never apply to
//! synthetic fixtures.
//!
//! | Case | W3C instruction (quoted from the `.txml` source) | What is checked | Why it is equivalent |
//! | --- | --- | --- | --- |
//! | 178 | "The tester must look at the log output and verify that both keys are there." | The root session takes the `event1` transition to `final` and emits exactly one log, labeled `_event `, whose value (`_event.raw`) contains the token `Var1` exactly twice, followed by `2` the first time and `3` the second. | The test sends two `<param name="Var1">` values (2 then 3) and logs only the raw event; seeing both key/value pairs, and no other `Var1` pair, in the logged raw text is the human's check. Raw text is tokenized on non-alphanumeric characters, so any serialization format that keeps each key next to its value passes. |
//! | 230 | "Both the parent and child process print out the contents of the event. The tester must check if they are the same and report his result." | The root session and its single invoked session each log exactly the seven labeled `_event` fields (`name`, `type`, `sendid`, `origin`, `origintype`, `invokeid`, `data`) in document order; the parent's `name` is `childToParent`; each child value equals the parent value for the same field. | The parent logs the event it received from the child; the child logs the autoforwarded copy. Field-by-field equality of the two printed sets is exactly "the same". |
//! | 250 | "Tester must examine log output from child process to determine success" | The root session logs nothing; its single invoked session logs exactly `Exiting sub01` then `Exiting sub0` and never logs `entering final state, invocation was not cancelled`. | Success means the cancelled child ran the `<onexit>` handlers of both active states (SCXML 6.4); exit order is innermost first (SCXML 3.13), and the "not cancelled" log would prove the invocation was not cancelled. |
//! | 301 | "the processor should  reject this document because it can't download the script. Therefore  we fail if it runs at all." | The processor rejected the document at load because the script loader could not fetch its `<script src>` ([`LoadRejection::ScriptUnavailable`](crate::executor::LoadRejection::ScriptUnavailable)): terminal status `rejected`, and no command, fuel, configuration, log, or trace observation at all. | SCXML 1.0 section 5.8 requires rejecting a document whose script cannot be downloaded; the test's only reachable state logs `Outcome` `fail`, so "fails if it runs at all" means any executed command, log, or state fails. |
//! | 307 | "the tester must report whether the output is the same in the two cases" | The root session's logs are exactly, for s0 then s1: either the state's `entering ...` value log followed by its no-error log (`no error in s0`/`No error in s1`), or its error log alone (`error in state s0`/`error in state s1`). Both states report an error, or both report equal values without an error. | Case one accesses a late-bound variable before load, case two a missing substructure of a loaded variable; the assertion requires the same execution-time behavior. The test logs "the values that both operations yield, and whether there are errors": an access without an error yields a value its entry log must report, and an access that errors aborts its onentry block before that log (SCXML 4.9, 5.9). Equal values, or an error in both, are the outputs the tester compares. |
//! | 415 | "there is no platform-independent way to test that event1 is not processed" | The root session's whole trace is exactly the start command followed by one raise of `event1`: the engine processed no event at all, completed no further command, logged nothing, and invoked no session. | The engine reports every event it dequeues and examines by transition selection, selected or ignored, as a processed-event observation; none for `event1` is the instruction's check, and the single start command with its single raise is the machine halting immediately on entering the top-level final state (SCXML 3.13). |
//!
//! Every oracle except 301 (whose document never runs) also requires: the
//! trace's root-session logs equal to the logs the official oracle would read (no divergent
//! evidence); no `Outcome` log at all (in these documents an `Outcome` log
//! only comes from the `conf:fail` state); and a finished machine whose only
//! active state is `final`.
//!
//! [`MANUAL_ORACLE_VERSION`](crate::manual_oracle::MANUAL_ORACLE_VERSION) identifies this table. Any change to a checked
//! condition MUST bump it; the version and a digest of this source are bound
//! into every conformance report identity so baselines detect the change.

use crate::executor::{ExecutionResult, LoadRejection, LogRecord, TraceObservation, TraceRecord};
use crate::manifest::CaseManifest;
use crate::oracle::{OUTCOME_LABEL, OracleFailure};
use crate::report::TerminalStatus;

/// Stable implementation name of the manual-equivalent oracle table.
pub const MANUAL_ORACLE_NAME: &str = "scxml-conformance-manual-oracle";

/// Version of the manual-equivalent oracle table; bumped on any change to a
/// checked condition.
pub const MANUAL_ORACLE_VERSION: &str = "4";

/// The only top-level final state every manual case must finish in.
const FINAL_STATE: &str = "final";

/// Labels of the seven `_event` fields official case 230 logs, in order.
const CASE_230_FIELD_LABELS: [&str; 7] = [
    "name is ",
    "type is ",
    "sendid is ",
    "origin is ",
    "origintype is ",
    "invokeid is ",
    "data is ",
];

const CASE_307_S0_ENTRY: &str = "entering s0 value of Var 1 is: ";
const CASE_307_S0_ERROR: &str = "error in state s0";
const CASE_307_S0_NO_ERROR: &str = "no error in s0";
const CASE_307_S1_ENTRY: &str = "entering s1, value of non-existent substructure of Var 1 is: ";
const CASE_307_S1_ERROR: &str = "error in state s1";
const CASE_307_S1_NO_ERROR: &str = "No error in s1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// One documented manual-equivalent oracle, keyed by official case ID.
pub enum ManualOracle {
    /// Case 178: duplicate `<param>` names both reach `_event.raw`.
    DuplicateParams178,
    /// Case 230: an autoforwarded event keeps every `_event` field value.
    AutoforwardFields230,
    /// Case 250: a cancelled invoked session runs its `<onexit>` handlers.
    CancelledChildOnexit250,
    /// Case 301: a document whose `<script src>` cannot be fetched is
    /// rejected at load and never runs.
    UnfetchableScriptRejected301,
    /// Case 307: late-bound access equals missing-substructure access.
    LateBindingEquivalence307,
    /// Case 415: entering a top-level final state halts processing.
    TopLevelFinalHalts415,
}

impl ManualOracle {
    /// Every documented manual-equivalent oracle, in case-ID order.
    pub const ALL: [Self; 6] = [
        Self::DuplicateParams178,
        Self::AutoforwardFields230,
        Self::CancelledChildOnexit250,
        Self::UnfetchableScriptRejected301,
        Self::LateBindingEquivalence307,
        Self::TopLevelFinalHalts415,
    ];

    /// Official W3C case (and assertion) identifier this oracle decides.
    #[must_use]
    pub const fn case_id(self) -> &'static str {
        match self {
            Self::DuplicateParams178 => "178",
            Self::AutoforwardFields230 => "230",
            Self::CancelledChildOnexit250 => "250",
            Self::UnfetchableScriptRejected301 => "301",
            Self::LateBindingEquivalence307 => "307",
            Self::TopLevelFinalHalts415 => "415",
        }
    }

    /// The W3C manual instruction this oracle automates, quoted verbatim
    /// from the official `.txml` source comment.
    #[must_use]
    pub const fn instruction(self) -> &'static str {
        match self {
            Self::DuplicateParams178 => {
                "The tester must look at the log output and verify that both keys are there."
            }
            Self::AutoforwardFields230 => {
                "Both the parent and child process print out the contents of the event. The tester must check if they are the same and report his result."
            }
            Self::CancelledChildOnexit250 => {
                "Tester must examine log output from child process to determine success"
            }
            Self::UnfetchableScriptRejected301 => {
                "the processor should  reject this document because it can't download the script.  \nTherefore  we fail if it runs at all."
            }
            Self::LateBindingEquivalence307 => {
                "the tester must report whether the output is the same in the two cases"
            }
            Self::TopLevelFinalHalts415 => {
                "there is no platform-independent way to test that event1 is not processed"
            }
        }
    }

    /// Selects the documented oracle for an official case.
    ///
    /// Returns `Some` only when the manifest marks the case manual and both
    /// its case ID and assertion ID equal a documented case; every other
    /// case keeps the official `Outcome` oracle.
    #[must_use]
    pub fn for_case(case: &CaseManifest) -> Option<Self> {
        if case.manual && case.assertion_id == case.id {
            Self::ALL
                .into_iter()
                .find(|oracle| oracle.case_id() == case.id)
        } else {
            None
        }
    }

    /// Evaluates this oracle over one completed execution.
    ///
    /// # Errors
    ///
    /// Returns [`OracleFailure`] with a stable `manual-*` code when the
    /// case-specific check in the module table fails, or, for every case
    /// except 301, when the trace is inconsistent, an `Outcome`
    /// log exists, or the machine did not finish in `final`.
    pub fn evaluate(self, actual: &ExecutionResult) -> Result<(), OracleFailure> {
        // Case 301's document never runs, so it has no trace to check;
        // every other case first passes the shared trace preconditions.
        match self {
            Self::UnfetchableScriptRejected301 => evaluate_301(actual),
            Self::DuplicateParams178 => evaluate_common(actual).and_then(|t| evaluate_178(&t)),
            Self::AutoforwardFields230 => evaluate_common(actual).and_then(|t| evaluate_230(&t)),
            Self::CancelledChildOnexit250 => evaluate_common(actual).and_then(|t| evaluate_250(&t)),
            Self::LateBindingEquivalence307 => {
                evaluate_common(actual).and_then(|t| evaluate_307(&t))
            }
            Self::TopLevelFinalHalts415 => evaluate_common(actual).and_then(|t| evaluate_415(&t)),
        }
    }
}

/// Session-partitioned view of a validated trace.
struct SessionTrace<'a> {
    /// Every root-session record in order.
    root: Vec<&'a TraceObservation>,
    /// Records of sessions invoked directly by the root, grouped by session
    /// in order of first appearance.
    children: Vec<(&'a str, Vec<&'a TraceObservation>)>,
    /// Whether any session was invoked below a invoked session.
    has_grandchildren: bool,
}

impl SessionTrace<'_> {
    fn root_logs(&self) -> Vec<&LogRecord> {
        logs_of(&self.root)
    }
}

fn logs_of<'a>(observations: &[&'a TraceObservation]) -> Vec<&'a LogRecord> {
    observations
        .iter()
        .filter_map(|observation| match observation {
            TraceObservation::Log(record) => Some(record),
            TraceObservation::Command
            | TraceObservation::Raise { .. }
            | TraceObservation::Processed { .. } => None,
        })
        .collect()
}

fn failure(code: &'static str, message: impl Into<String>) -> OracleFailure {
    OracleFailure {
        code,
        message: message.into(),
    }
}

/// Checks the preconditions shared by every manual oracle and partitions
/// the trace by session.
fn evaluate_common(actual: &ExecutionResult) -> Result<SessionTrace<'_>, OracleFailure> {
    let trace = partition(&actual.trace.records)?;
    let root_logs = trace.root_logs();
    if root_logs.len() != actual.logs.len()
        || root_logs
            .iter()
            .zip(&actual.logs)
            .any(|(traced, logged)| *traced != logged)
    {
        return Err(failure(
            "manual-trace-inconsistent",
            "root-session trace logs differ from the logs read by the official oracle",
        ));
    }
    if actual
        .logs
        .iter()
        .any(|record| record.label == OUTCOME_LABEL)
    {
        return Err(failure(
            "manual-outcome-logged",
            "a manual case logged an Outcome record, which only its fail state emits",
        ));
    }
    if actual.terminal_status != TerminalStatus::Finished || actual.configuration != [FINAL_STATE] {
        return Err(failure(
            "manual-final-not-reached",
            format!(
                "expected the machine to finish in `{FINAL_STATE}`, got status {:?} configuration {:?}",
                actual.terminal_status, actual.configuration
            ),
        ));
    }
    Ok(trace)
}

fn partition(records: &[TraceRecord]) -> Result<SessionTrace<'_>, OracleFailure> {
    let mut root_session: Option<&str> = None;
    let mut root = Vec::new();
    let mut children: Vec<(&str, Vec<&TraceObservation>)> = Vec::new();
    let mut has_grandchildren = false;
    for record in records {
        match record.parent_session.as_deref() {
            None => {
                match root_session {
                    None => root_session = Some(record.session.as_str()),
                    Some(session) if session == record.session => {}
                    Some(_) => {
                        return Err(failure(
                            "manual-trace-inconsistent",
                            "the trace names more than one root session",
                        ));
                    }
                }
                root.push(&record.observation);
            }
            Some(parent) => {
                if Some(parent) != root_session {
                    has_grandchildren = true;
                    continue;
                }
                if let Some((_, observations)) = children
                    .iter_mut()
                    .find(|(session, _)| *session == record.session)
                {
                    observations.push(&record.observation);
                } else {
                    children.push((record.session.as_str(), vec![&record.observation]));
                }
            }
        }
    }
    if root_session.is_none() {
        return Err(failure(
            "manual-trace-inconsistent",
            "the trace records no root-session observation",
        ));
    }
    Ok(SessionTrace {
        root,
        children,
        has_grandchildren,
    })
}

/// Returns the logs of the single session the root invoked.
fn single_child_logs<'a>(trace: &SessionTrace<'a>) -> Result<Vec<&'a LogRecord>, OracleFailure> {
    match trace.children.as_slice() {
        [(_, observations)] if !trace.has_grandchildren => Ok(logs_of(observations)),
        _ => Err(failure(
            "manual-invocation-count",
            format!(
                "expected exactly one invoked session and no nested invocation, observed {} direct invoked sessions",
                trace.children.len()
            ),
        )),
    }
}

/// Splits text into maximal runs of ASCII alphanumeric characters and `_`.
fn tokens(text: &str) -> Vec<&str> {
    text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|token| !token.is_empty())
        .collect()
}

/// The token after each occurrence of `key` in `tokens`, in order (`None`
/// for an occurrence that ends the text).
fn values_after_key<'a>(tokens: &[&'a str], key: &str) -> Vec<Option<&'a str>> {
    tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| **token == key)
        .map(|(index, _)| tokens.get(index + 1).copied())
        .collect()
}

/// Case 301: the processor rejected the document at load because its
/// `<script src>` could not be fetched, and nothing ran.
fn evaluate_301(actual: &ExecutionResult) -> Result<(), OracleFailure> {
    let ran = actual.commands != 0
        || actual.fuel_consumed != 0
        || !actual.configuration.is_empty()
        || !actual.logs.is_empty()
        || !actual.trace.records.is_empty();
    if actual.terminal_status != TerminalStatus::Rejected
        || actual.load_rejection != Some(LoadRejection::ScriptUnavailable)
    {
        Err(failure(
            "manual-301-not-rejected",
            format!(
                "expected the document rejected at load for an unfetchable script, got status {:?} rejection {:?}",
                actual.terminal_status, actual.load_rejection
            ),
        ))
    } else if ran {
        Err(failure(
            "manual-301-document-ran",
            "a rejected document recorded execution, which the test forbids",
        ))
    } else {
        Ok(())
    }
}

fn evaluate_178(trace: &SessionTrace<'_>) -> Result<(), OracleFailure> {
    match trace.root_logs().as_slice() {
        [record] if record.label == "_event " => {
            if values_after_key(&tokens(&record.value), "Var1") == [Some("2"), Some("3")] {
                Ok(())
            } else {
                Err(failure(
                    "manual-178-duplicate-params-missing",
                    "the logged _event.raw does not contain exactly the two sent Var1 key/value pairs (2 then 3)",
                ))
            }
        }
        logs => Err(failure(
            "manual-178-event-log",
            format!(
                "expected exactly one `_event ` log, observed {} root logs",
                logs.len()
            ),
        )),
    }
}

fn evaluate_230(trace: &SessionTrace<'_>) -> Result<(), OracleFailure> {
    let parent = trace.root_logs();
    let child = single_child_logs(trace)?;
    let labels_match = |logs: &[&LogRecord]| {
        logs.len() == CASE_230_FIELD_LABELS.len()
            && logs
                .iter()
                .zip(CASE_230_FIELD_LABELS)
                .all(|(record, label)| record.label == label)
    };
    if !labels_match(&parent) {
        return Err(failure(
            "manual-230-parent-fields",
            "the parent session did not log exactly the seven _event fields in order",
        ));
    }
    if !labels_match(&child) {
        return Err(failure(
            "manual-230-child-fields",
            "the invoked session did not log exactly the seven autoforwarded _event fields in order",
        ));
    }
    if parent.first().map(|record| record.value.as_str()) != Some("childToParent") {
        return Err(failure(
            "manual-230-event-name",
            "the parent session did not log the childToParent event",
        ));
    }
    if let Some((parent_field, _)) = parent
        .iter()
        .zip(&child)
        .find(|(parent_field, child_field)| parent_field.value != child_field.value)
    {
        return Err(failure(
            "manual-230-field-mismatch",
            format!(
                "the autoforwarded copy differs from the original in field `{}`",
                parent_field.label.trim_end()
            ),
        ));
    }
    Ok(())
}

fn evaluate_250(trace: &SessionTrace<'_>) -> Result<(), OracleFailure> {
    if !trace.root_logs().is_empty() {
        return Err(failure(
            "manual-250-parent-logged",
            "the invoking session emitted logs; only the child's logs decide this case",
        ));
    }
    let child = single_child_logs(trace)?;
    let values = child
        .iter()
        .map(|record| (record.label.as_str(), record.value.as_str()))
        .collect::<Vec<_>>();
    if values == [("", "Exiting sub01"), ("", "Exiting sub0")] {
        Ok(())
    } else {
        Err(failure(
            "manual-250-onexit-missing",
            "the cancelled invoked session did not log exactly `Exiting sub01` then `Exiting sub0`",
        ))
    }
}

fn evaluate_307(trace: &SessionTrace<'_>) -> Result<(), OracleFailure> {
    let logs = trace.root_logs();
    let mut remaining = logs.as_slice();
    let s0 = take_access_output(
        &mut remaining,
        CASE_307_S0_ENTRY,
        CASE_307_S0_ERROR,
        CASE_307_S0_NO_ERROR,
    )?;
    let s1 = take_access_output(
        &mut remaining,
        CASE_307_S1_ENTRY,
        CASE_307_S1_ERROR,
        CASE_307_S1_NO_ERROR,
    )?;
    if !remaining.is_empty() {
        return Err(failure(
            "manual-307-log-sequence",
            "the root session logged records outside the documented s0/s1 sequence",
        ));
    }
    match (s0, s1) {
        (AccessOutput::Value(s0_value), AccessOutput::Value(s1_value)) if s0_value != s1_value => {
            Err(failure(
                "manual-307-value-mismatch",
                "the late-bound access and the missing-substructure access logged different values",
            ))
        }
        (AccessOutput::Value(_), AccessOutput::Error)
        | (AccessOutput::Error, AccessOutput::Value(_)) => Err(failure(
            "manual-307-error-mismatch",
            "exactly one of the two accesses raised an error",
        )),
        _ => Ok(()),
    }
}

/// What one of case 307's two accesses produced: the value its entry log
/// reported, or an error.
#[derive(Debug, Eq, PartialEq)]
enum AccessOutput {
    /// The access raised no error and the entry log reported this value.
    Value(String),
    /// The access raised an error, which aborted the onentry block before
    /// its log (SCXML 4.9, 5.9), so no value was reported.
    Error,
}

/// Consumes one state's entry log (when present) and exactly one of its
/// error/no-error logs. The access either yielded a value, which the entry
/// log reports, and no error; or raised an error and yielded no value.
fn take_access_output(
    remaining: &mut &[&LogRecord],
    entry_label: &str,
    error_label: &str,
    no_error_label: &str,
) -> Result<AccessOutput, OracleFailure> {
    let value = match *remaining {
        [record, rest @ ..] if record.label == entry_label => {
            *remaining = rest;
            Some(record.value.clone())
        }
        _ => None,
    };
    let errored = match *remaining {
        [record, rest @ ..] if record.label == error_label || record.label == no_error_label => {
            *remaining = rest;
            record.label == error_label
        }
        _ => {
            return Err(failure(
                "manual-307-log-sequence",
                format!(
                    "expected `{error_label}` or `{no_error_label}` next in the root-session logs"
                ),
            ));
        }
    };
    match (value, errored) {
        (Some(value), false) => Ok(AccessOutput::Value(value)),
        (None, true) => Ok(AccessOutput::Error),
        (None, false) => Err(failure(
            "manual-307-value-missing",
            format!("`{no_error_label}` was logged without the value `{entry_label}` reports"),
        )),
        (Some(_), true) => Err(failure(
            "manual-307-value-with-error",
            format!("`{error_label}` was logged after the access reported a value"),
        )),
    }
}

fn evaluate_415(trace: &SessionTrace<'_>) -> Result<(), OracleFailure> {
    if !trace.children.is_empty() || trace.has_grandchildren {
        return Err(failure(
            "manual-415-invocation",
            "the top-level final machine must not invoke any session",
        ));
    }
    if !trace.root_logs().is_empty() {
        return Err(failure(
            "manual-415-logged",
            "the top-level final machine must not emit logs",
        ));
    }
    if let Some(event) = trace.root.iter().find_map(|observation| match observation {
        TraceObservation::Processed { event } => Some(event),
        _ => None,
    }) {
        return Err(failure(
            "manual-415-event-processed",
            format!(
                "the engine processed `{event}` although the machine halts on entering the top-level final state"
            ),
        ));
    }
    let raises: Vec<&str> = trace
        .root
        .iter()
        .filter_map(|observation| match observation {
            TraceObservation::Raise { event } => Some(event.as_str()),
            _ => None,
        })
        .collect();
    if !raises.contains(&"event1") {
        return Err(failure(
            "manual-415-event-not-raised",
            "the final state's onentry did not raise event1",
        ));
    }
    let raise_sequence = || {
        failure(
            "manual-415-raise-sequence",
            format!(
                "expected the start command to raise exactly event1 once, observed raises {raises:?}"
            ),
        )
    };
    if raises != ["event1"] {
        return Err(raise_sequence());
    }
    match trace.root.as_slice() {
        [TraceObservation::Command, TraceObservation::Raise { .. }] => Ok(()),
        [
            ..,
            TraceObservation::Raise { .. },
            TraceObservation::Command,
        ] => Err(failure(
            "manual-415-command-after-final",
            "the engine ran another command after entering the top-level final state",
        )),
        _ => Err(raise_sequence()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ExecutionTrace;

    const ROOT: &str = "conformance";
    const CHILD: &str = "conformance.1";

    fn log(label: &str, value: &str) -> TraceObservation {
        TraceObservation::Log(LogRecord {
            label: label.into(),
            value: value.into(),
        })
    }

    fn raise(event: &str) -> TraceObservation {
        TraceObservation::Raise {
            event: event.into(),
        }
    }

    fn processed_event_observation(event: &str) -> TraceObservation {
        TraceObservation::Processed {
            event: event.into(),
        }
    }

    fn root(observation: TraceObservation) -> TraceRecord {
        TraceRecord {
            session: ROOT.into(),
            parent_session: None,
            observation,
        }
    }

    fn child(observation: TraceObservation) -> TraceRecord {
        TraceRecord {
            session: CHILD.into(),
            parent_session: Some(ROOT.into()),
            observation,
        }
    }

    /// A finished run in `final` whose official-oracle logs are exactly the
    /// root-session logs of `records`.
    fn finished(records: Vec<TraceRecord>) -> ExecutionResult {
        let logs = records
            .iter()
            .filter(|record| record.parent_session.is_none())
            .filter_map(|record| match &record.observation {
                TraceObservation::Log(log) => Some(log.clone()),
                _ => None,
            })
            .collect();
        ExecutionResult {
            configuration: vec![FINAL_STATE.into()],
            terminal_status: TerminalStatus::Finished,
            commands: 1,
            fuel_consumed: 1,
            logs,
            trace: ExecutionTrace { records },
            load_rejection: None,
            snapshots: Vec::new(),
        }
    }

    /// A document rejected at load: nothing ran.
    fn rejected(rejection: LoadRejection) -> ExecutionResult {
        ExecutionResult {
            configuration: Vec::new(),
            terminal_status: TerminalStatus::Rejected,
            commands: 0,
            fuel_consumed: 0,
            logs: Vec::new(),
            trace: ExecutionTrace::default(),
            load_rejection: Some(rejection),
            snapshots: Vec::new(),
        }
    }

    fn code(oracle: ManualOracle, actual: &ExecutionResult) -> &'static str {
        oracle
            .evaluate(actual)
            .expect_err("mutated trace must fail the manual-equivalent oracle")
            .code
    }

    fn passing_178() -> Vec<TraceRecord> {
        vec![
            root(TraceObservation::Command),
            root(TraceObservation::Command),
            root(log("_event ", "name=event1&Var1=2&Var1=3")),
        ]
    }

    fn passing_230() -> Vec<TraceRecord> {
        let values = [
            "childToParent",
            "external",
            "",
            "#_scxml_conformance.1",
            "http://www.w3.org/TR/scxml/#SCXMLEventProcessor",
            "i1",
            "undefined",
        ];
        let mut records = vec![root(TraceObservation::Command)];
        records.extend(
            CASE_230_FIELD_LABELS
                .iter()
                .zip(values)
                .map(|(label, value)| root(log(label, value))),
        );
        records.push(child(TraceObservation::Command));
        records.extend(
            CASE_230_FIELD_LABELS
                .iter()
                .zip(values)
                .map(|(label, value)| child(log(label, value))),
        );
        records
    }

    fn passing_250() -> Vec<TraceRecord> {
        vec![
            root(TraceObservation::Command),
            child(TraceObservation::Command),
            root(TraceObservation::Command),
            child(log("", "Exiting sub01")),
            child(log("", "Exiting sub0")),
        ]
    }

    fn passing_307() -> Vec<TraceRecord> {
        vec![
            root(TraceObservation::Command),
            root(log(CASE_307_S0_ENTRY, "undefined")),
            root(raise("foo")),
            root(TraceObservation::Command),
            root(log(CASE_307_S0_NO_ERROR, "undefined")),
            root(log(CASE_307_S1_ENTRY, "undefined")),
            root(raise("bar")),
            root(TraceObservation::Command),
            root(log(CASE_307_S1_NO_ERROR, "undefined")),
        ]
    }

    fn passing_415() -> Vec<TraceRecord> {
        vec![root(TraceObservation::Command), root(raise("event1"))]
    }

    #[test]
    fn manual_oracles_select_only_documented_official_manual_cases() {
        #[derive(Clone, Copy)]
        enum Marking {
            Manual,
            Automatic,
        }
        let manifest = |id: &str, assertion: &str, marking: Marking| -> CaseManifest {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "assertion_id": assertion,
                "spec_num": "3.13",
                "spec_id": "#SelectingTransitions",
                "conformance": "mandatory",
                "datamodel": "ecmascript",
                "start_uri": "x/test.txml",
                "manual": matches!(marking, Marking::Manual),
                "expected": {"kind": "w3c-log-outcome", "outcome": "pass"},
            }))
            .expect("test case manifest must decode")
        };
        for oracle in ManualOracle::ALL {
            let id = oracle.case_id();
            assert_eq!(
                ManualOracle::for_case(&manifest(id, id, Marking::Manual)),
                Some(oracle)
            );
            assert_eq!(
                ManualOracle::for_case(&manifest(id, id, Marking::Automatic)),
                None
            );
            assert_eq!(
                ManualOracle::for_case(&manifest(id, "999", Marking::Manual)),
                None
            );
            assert!(!oracle.instruction().is_empty());
        }
        assert_eq!(
            ManualOracle::for_case(&manifest("144", "144", Marking::Manual)),
            None
        );
    }

    #[test]
    fn common_preconditions_reject_inconsistent_outcome_and_unfinished_runs() {
        let mut inconsistent = finished(passing_415());
        inconsistent.logs.push(LogRecord {
            label: "x".into(),
            value: "y".into(),
        });
        assert_eq!(
            code(ManualOracle::TopLevelFinalHalts415, &inconsistent),
            "manual-trace-inconsistent"
        );

        let mut two_roots = passing_415();
        two_roots.push(TraceRecord {
            session: "other".into(),
            parent_session: None,
            observation: TraceObservation::Command,
        });
        assert_eq!(
            code(ManualOracle::TopLevelFinalHalts415, &finished(two_roots)),
            "manual-trace-inconsistent"
        );

        let mut outcome = passing_178();
        outcome.push(root(log(OUTCOME_LABEL, "fail")));
        assert_eq!(
            code(ManualOracle::DuplicateParams178, &finished(outcome)),
            "manual-outcome-logged"
        );

        let mut running = finished(passing_415());
        running.terminal_status = TerminalStatus::Running;
        assert_eq!(
            code(ManualOracle::TopLevelFinalHalts415, &running),
            "manual-final-not-reached"
        );
        let mut failed_state = finished(passing_415());
        failed_state.configuration = vec!["fail".into()];
        assert_eq!(
            code(ManualOracle::TopLevelFinalHalts415, &failed_state),
            "manual-final-not-reached"
        );
    }

    #[test]
    fn case_178_requires_both_duplicate_param_pairs_in_the_logged_raw_event() {
        let oracle = ManualOracle::DuplicateParams178;
        assert_eq!(oracle.evaluate(&finished(passing_178())), Ok(()));
        let mut json = passing_178();
        json[2] = root(log("_event ", r#"{"params":[["Var1",2],["Var1",3]]}"#));
        assert_eq!(oracle.evaluate(&finished(json)), Ok(()));

        for raw in [
            "Var1=3",
            "Var1=2",
            "Var1=3&Var1=2",
            "Var10=2&Var1=3",
            "undefined",
            // Wrong behavior the tester would see: a key sent once appears
            // twice, a third pair appears, or the pairs are reordered.
            "Var1=2&Var1=2&Var1=3",
            "Var1=3&Var1=2&Var1=3",
            "Var1=2&Var1=3&Var1=4",
            "Var1=2&Var1=3&Var1=3",
        ] {
            let mut mutated = passing_178();
            mutated[2] = root(log("_event ", raw));
            assert_eq!(
                code(oracle, &finished(mutated)),
                "manual-178-duplicate-params-missing",
                "raw {raw:?} must not pass"
            );
        }
        let mut missing = passing_178();
        missing.pop();
        assert_eq!(code(oracle, &finished(missing)), "manual-178-event-log");
        let mut relabeled = passing_178();
        relabeled[2] = root(log("_event", "Var1=2&Var1=3"));
        assert_eq!(code(oracle, &finished(relabeled)), "manual-178-event-log");
    }

    #[test]
    fn case_230_requires_equal_parent_and_forwarded_child_event_fields() {
        let oracle = ManualOracle::AutoforwardFields230;
        assert_eq!(oracle.evaluate(&finished(passing_230())), Ok(()));

        let mut mismatch = passing_230();
        let last = mismatch.len() - 1;
        mismatch[last] = child(log("data is ", "{}"));
        assert_eq!(
            code(oracle, &finished(mismatch)),
            "manual-230-field-mismatch"
        );

        let child_logs_missing = passing_230()
            .into_iter()
            .filter(|record| {
                record.parent_session.is_none()
                    || matches!(record.observation, TraceObservation::Command)
            })
            .collect();
        assert_eq!(
            code(oracle, &finished(child_logs_missing)),
            "manual-230-child-fields"
        );

        let no_child = passing_230()
            .into_iter()
            .filter(|record| record.parent_session.is_none())
            .collect();
        assert_eq!(code(oracle, &finished(no_child)), "manual-invocation-count");

        let mut parent_missing_field = passing_230();
        parent_missing_field.remove(3);
        assert_eq!(
            code(oracle, &finished(parent_missing_field)),
            "manual-230-parent-fields"
        );

        let renamed = passing_230()
            .into_iter()
            .map(|record| match &record.observation {
                TraceObservation::Log(entry) if entry.label == "name is " => TraceRecord {
                    observation: log("name is ", "other"),
                    ..record
                },
                _ => record,
            })
            .collect();
        assert_eq!(code(oracle, &finished(renamed)), "manual-230-event-name");
    }

    #[test]
    fn case_250_requires_cancelled_child_onexit_logs_in_exit_order() {
        let oracle = ManualOracle::CancelledChildOnexit250;
        assert_eq!(oracle.evaluate(&finished(passing_250())), Ok(()));

        let dropped_child = passing_250()
            .into_iter()
            .filter(|record| {
                record.parent_session.is_none()
                    || matches!(record.observation, TraceObservation::Command)
            })
            .collect();
        assert_eq!(
            code(oracle, &finished(dropped_child)),
            "manual-250-onexit-missing"
        );

        let mut reversed = passing_250();
        reversed.swap(3, 4);
        assert_eq!(
            code(oracle, &finished(reversed)),
            "manual-250-onexit-missing"
        );

        let mut not_cancelled = passing_250();
        not_cancelled.push(child(log(
            "",
            "entering final state, invocation was not cancelled",
        )));
        assert_eq!(
            code(oracle, &finished(not_cancelled)),
            "manual-250-onexit-missing"
        );

        let mut parent_logged = passing_250();
        parent_logged.push(root(log("", "Exiting sub0")));
        assert_eq!(
            code(oracle, &finished(parent_logged)),
            "manual-250-parent-logged"
        );

        let mut grandchild = passing_250();
        grandchild.push(TraceRecord {
            session: "conformance.1.1".into(),
            parent_session: Some(CHILD.into()),
            observation: TraceObservation::Command,
        });
        assert_eq!(
            code(oracle, &finished(grandchild)),
            "manual-invocation-count"
        );
    }

    #[test]
    fn case_307_requires_identical_late_binding_and_missing_substructure_output() {
        let oracle = ManualOracle::LateBindingEquivalence307;
        assert_eq!(oracle.evaluate(&finished(passing_307())), Ok(()));
        let both_error = vec![
            root(TraceObservation::Command),
            root(log(CASE_307_S0_ERROR, "error.execution")),
            root(log(CASE_307_S1_ERROR, "error.execution")),
        ];
        assert_eq!(oracle.evaluate(&finished(both_error)), Ok(()));

        let mut value_mismatch = passing_307();
        value_mismatch[5] = root(log(CASE_307_S1_ENTRY, "null"));
        assert_eq!(
            code(oracle, &finished(value_mismatch)),
            "manual-307-value-mismatch"
        );

        // An access that raised no error yielded a value, and the test logs
        // it: a missing value log without an error is not the test's output.
        let mut one_sided_value = passing_307();
        one_sided_value.remove(1);
        assert_eq!(
            code(oracle, &finished(one_sided_value)),
            "manual-307-value-missing"
        );
        let no_values = vec![
            root(TraceObservation::Command),
            root(log(CASE_307_S0_NO_ERROR, "undefined")),
            root(log(CASE_307_S1_NO_ERROR, "undefined")),
        ];
        assert_eq!(
            code(oracle, &finished(no_values)),
            "manual-307-value-missing"
        );

        // An access that raised an error yielded no value: the error aborts
        // the onentry block before its log (SCXML 4.9, 5.9), so a value log
        // followed by the error outcome is not the test's output.
        let values_with_errors = vec![
            root(log(CASE_307_S0_ENTRY, "undefined")),
            root(log(CASE_307_S0_ERROR, "error.execution")),
            root(log(CASE_307_S1_ENTRY, "undefined")),
            root(log(CASE_307_S1_ERROR, "error.execution")),
        ];
        assert_eq!(
            code(oracle, &finished(values_with_errors)),
            "manual-307-value-with-error"
        );

        let error_mismatch = vec![
            root(log(CASE_307_S0_ENTRY, "undefined")),
            root(log(CASE_307_S0_NO_ERROR, "undefined")),
            root(log(CASE_307_S1_ERROR, "error.execution")),
        ];
        assert_eq!(
            code(oracle, &finished(error_mismatch)),
            "manual-307-error-mismatch"
        );

        let mut extra = passing_307();
        extra.push(root(log("unexpected", "x")));
        assert_eq!(code(oracle, &finished(extra)), "manual-307-log-sequence");

        let mut missing_outcome = passing_307();
        missing_outcome.remove(4);
        assert_eq!(
            code(oracle, &finished(missing_outcome)),
            "manual-307-log-sequence"
        );
    }

    #[test]
    fn case_415_requires_the_start_command_to_raise_event1_and_process_nothing() {
        let oracle = ManualOracle::TopLevelFinalHalts415;
        assert_eq!(oracle.evaluate(&finished(passing_415())), Ok(()));

        let mut commanded = passing_415();
        commanded.push(root(TraceObservation::Command));
        assert_eq!(
            code(oracle, &finished(commanded)),
            "manual-415-command-after-final"
        );

        // The engine processed event1 (or any event) without another
        // command observation after the raise.
        for processed_event in ["event1", "other"] {
            let mut processed = passing_415();
            processed.push(root(processed_event_observation(processed_event)));
            assert_eq!(
                code(oracle, &finished(processed)),
                "manual-415-event-processed",
                "{processed_event}"
            );
            let processed_first = vec![
                root(TraceObservation::Command),
                root(processed_event_observation(processed_event)),
                root(raise("event1")),
            ];
            assert_eq!(
                code(oracle, &finished(processed_first)),
                "manual-415-event-processed",
                "{processed_event}"
            );
        }

        // The onentry raises event1 exactly once, in the start command.
        let duplicate = vec![
            root(TraceObservation::Command),
            root(raise("event1")),
            root(raise("event1")),
        ];
        assert_eq!(
            code(oracle, &finished(duplicate)),
            "manual-415-raise-sequence"
        );
        let early = vec![
            root(raise("event1")),
            root(TraceObservation::Command),
            root(raise("event1")),
        ];
        assert_eq!(code(oracle, &finished(early)), "manual-415-raise-sequence");
        let extra_raise = vec![
            root(TraceObservation::Command),
            root(raise("event1")),
            root(raise("event2")),
        ];
        assert_eq!(
            code(oracle, &finished(extra_raise)),
            "manual-415-raise-sequence"
        );

        let not_raised = vec![root(TraceObservation::Command)];
        assert_eq!(
            code(oracle, &finished(not_raised)),
            "manual-415-event-not-raised"
        );

        let other_event = vec![root(TraceObservation::Command), root(raise("event2"))];
        assert_eq!(
            code(oracle, &finished(other_event)),
            "manual-415-event-not-raised"
        );

        let mut logged = passing_415();
        logged.push(root(log("", "event1 processed")));
        assert_eq!(code(oracle, &finished(logged)), "manual-415-logged");

        let mut invoked = passing_415();
        invoked.push(child(TraceObservation::Command));
        assert_eq!(code(oracle, &finished(invoked)), "manual-415-invocation");
    }

    #[test]
    fn case_301_requires_a_load_rejection_for_an_unfetchable_script_and_no_execution() {
        let oracle = ManualOracle::UnfetchableScriptRejected301;
        assert_eq!(
            oracle.evaluate(&rejected(LoadRejection::ScriptUnavailable)),
            Ok(())
        );

        // The document ran (the only reachable final logs `fail`).
        let mut ran = finished(vec![
            root(TraceObservation::Command),
            root(log(OUTCOME_LABEL, "fail")),
        ]);
        ran.configuration = vec!["fail".into()];
        assert_eq!(code(oracle, &ran), "manual-301-not-rejected");

        // Rejected for another reason than an unfetchable script.
        assert_eq!(
            code(oracle, &rejected(LoadRejection::ScriptInvalid)),
            "manual-301-not-rejected"
        );
        let mut unexplained = rejected(LoadRejection::ScriptUnavailable);
        unexplained.load_rejection = None;
        assert_eq!(code(oracle, &unexplained), "manual-301-not-rejected");

        // A rejection that still recorded any execution fails.
        for mutate in [
            (|result: &mut ExecutionResult| result.commands = 1) as fn(&mut ExecutionResult),
            |result| result.fuel_consumed = 1,
            |result| result.configuration = vec!["s0".into()],
            |result| {
                result.logs = vec![LogRecord {
                    label: OUTCOME_LABEL.into(),
                    value: "fail".into(),
                }];
            },
            |result| result.trace.records = vec![root(TraceObservation::Command)],
        ] {
            let mut ran = rejected(LoadRejection::ScriptUnavailable);
            mutate(&mut ran);
            assert_eq!(code(oracle, &ran), "manual-301-document-ran");
        }
    }
}
