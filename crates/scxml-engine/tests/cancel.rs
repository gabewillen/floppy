//! Session cancellation (`Command::Cancel`): an invoking parent cancels its
//! invoked session (SCXML 6.4), and the cancelled session exits every active
//! state through the W3C `exitInterpreter` procedure, running `<onexit>` in
//! exit order (SCXML 3.13; official manual IRP case 250).

use scxml_compiler_core::{MachineIr, SCXML_NAMESPACE, lower};
use scxml_engine::{
    BatchEnd, Command, Effect, EffectExecutionPhase, EffectExecutor, EffectFailure, Engine,
    EngineError, EngineStatus, EvaluationError, GuardEvaluator, PlannedEffect, RunFailureKind,
    RunStage,
};
use scxml_syntax::{SourceId, SourceText, parse_xml};
use scxml_types::{Event, EventName, EventType, StateId};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

#[derive(Default)]
struct Recorder {
    applied: Vec<String>,
}

impl GuardEvaluator for Recorder {
    fn evaluate_condition(
        &mut self,
        _expression: &str,
        _event: Option<&Event>,
    ) -> Result<bool, EvaluationError> {
        Err(EvaluationError::Unavailable)
    }
}

impl EffectExecutor for Recorder {
    type Error = EngineError;

    fn admit_command(&mut self) -> Result<(), EngineError> {
        Ok(())
    }

    fn command_completed(&mut self) {}

    fn engine_error(&mut self, error: EngineError, _stage: RunStage) -> EngineError {
        error
    }

    fn update_configuration(
        &mut self,
        _exited: &[StateId],
        _entered: &[StateId],
    ) -> Result<(), EngineError> {
        Ok(())
    }

    fn begin_batch(&mut self, _trigger: Option<&Event>) -> Result<(), EngineError> {
        Ok(())
    }

    fn apply_effect(
        &mut self,
        engine: &mut Engine,
        planned: &PlannedEffect,
        _trigger: Option<&Event>,
    ) -> Result<(), EffectFailure<EngineError>> {
        match &planned.effect {
            Effect::Raise(name) => {
                self.applied.push(format!("raise:{name}"));
                let name = EventName::new(name.clone()).expect("fixture event names are valid");
                Ok(engine.raise(Event::internal(name))?)
            }
            Effect::Log { label, .. } => {
                assert_eq!(
                    planned.context.phase,
                    EffectExecutionPhase::Exit,
                    "cancellation runs only exit content"
                );
                self.applied
                    .push(format!("log:{}", label.clone().unwrap_or_default()));
                Ok(())
            }
            _ => Err(EffectFailure::Fatal(EngineError::UnsupportedModel)),
        }
    }

    fn end_batch(&mut self, _end: BatchEnd) {}
}

fn machine(body: &str, initial: &str) -> MachineIr {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="{initial}" datamodel="null">{body}</scxml>"#
    );
    let document = parse_xml(SourceText::new(SOURCE, "m.scxml", source))
        .expect("fixture SCXML is well-formed XML");
    lower(&document).expect("fixture SCXML lowers")
}

fn external(name: &str) -> Event {
    let mut event = Event::internal(EventName::new(name.to_owned()).expect("valid event name"));
    event.event_type = EventType::External;
    event
}

/// Official case 250's invoked child: `sub0` with its initial `sub01`.
const CASE_250_CHILD: &str = r#"<state id="sub0" initial="sub01">
    <transition event="timeout" target="subFinal"/>
    <onexit><log label="Exiting sub0"/></onexit>
    <state id="sub01"><onexit><log label="Exiting sub01"/></onexit></state>
  </state>
  <final id="subFinal"><onentry><log label="not cancelled"/></onentry></final>"#;

#[test]
fn cancel_runs_onexit_of_every_active_state_in_exit_order() {
    let mut engine = Engine::new(machine(CASE_250_CHILD, "sub0")).expect("child is supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    assert!(recorder.applied.is_empty());
    let report = engine
        .run(Command::Cancel, &mut recorder)
        .expect("a running session cancels");
    assert_eq!(
        recorder.applied,
        ["log:Exiting sub01", "log:Exiting sub0"],
        "descendants exit before ancestors (SCXML 3.13 exit order)"
    );
    assert_eq!(report.commands, 1, "cancellation is one engine command");
    assert!(report.stable);
    assert_eq!(report.result.status, EngineStatus::Finished);
    assert!(report.result.configuration.is_empty());
    assert!(report.result.leaves.is_empty());
    assert_eq!(
        report
            .result
            .exited_states
            .iter()
            .map(StateId::as_str)
            .collect::<Vec<_>>(),
        ["sub01", "sub0"]
    );
    assert_eq!(
        engine
            .run(Command::Send(&external("timeout")), &mut recorder)
            .map_err(|failure| (failure.kind, failure.error)),
        Err((RunFailureKind::Rejected, EngineError::Finished)),
        "a cancelled session accepts no further events"
    );
}

#[test]
fn cancel_exits_parallel_regions_and_discards_queued_events() {
    let body = r#"<parallel id="p">
    <onexit><log label="exit p"/><raise event="late"/></onexit>
    <state id="a"><onexit><log label="exit a"/></onexit></state>
    <state id="b"><onexit><log label="exit b"/></onexit></state>
    <transition event="late" target="gone"/>
  </parallel>
  <final id="gone"><onentry><log label="not cancelled"/></onentry></final>"#;
    let mut engine = Engine::new(machine(body, "p")).expect("parallel is supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    let report = engine
        .run(Command::Cancel, &mut recorder)
        .expect("a running session cancels");
    assert_eq!(
        recorder.applied,
        ["log:exit b", "log:exit a", "log:exit p", "raise:late"],
        "regions exit in reverse document order before the parallel"
    );
    assert_eq!(report.result.status, EngineStatus::Finished);
    assert_eq!(
        engine.internal_len(),
        0,
        "events raised while exiting are never processed"
    );
}

#[test]
fn cancel_rejects_sessions_that_are_not_running() {
    let mut engine = Engine::new(machine(CASE_250_CHILD, "sub0")).expect("child is supported");
    let mut recorder = Recorder::default();
    assert_eq!(
        engine
            .run(Command::Cancel, &mut recorder)
            .map_err(|failure| (failure.kind, failure.error)),
        Err((RunFailureKind::Rejected, EngineError::NotStarted))
    );
    engine.run(Command::Start, &mut recorder).expect("start");
    engine.run(Command::Cancel, &mut recorder).expect("cancel");
    assert_eq!(
        engine
            .run(Command::Cancel, &mut recorder)
            .map_err(|failure| (failure.kind, failure.error)),
        Err((RunFailureKind::Rejected, EngineError::Finished))
    );
}
