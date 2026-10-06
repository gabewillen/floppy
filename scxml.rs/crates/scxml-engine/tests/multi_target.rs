//! Multi-target transitions (SCXML 3.13 `computeExitSet`/`computeEntrySet`
//! with several targets; official IRP case 364).
//!
//! Every chart lowers through the canonical compiler and runs through
//! [`Engine::run`]; the recording executor only routes `<raise>` back into
//! the engine and records `<log>` labels in execution order.

use scxml_compiler_core::{MachineIr, SCXML_NAMESPACE, lower};
use scxml_engine::{
    BatchEnd, Command, Effect, EffectExecutor, EffectFailure, Engine, EngineError, EngineStatus,
    EvaluationError, GuardEvaluator, PlannedEffect, RunStage,
};
use scxml_syntax::{SourceId, SourceText, parse_xml};
use scxml_types::{Event, EventName, EventType, StateId};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

#[derive(Default)]
struct Recorder {
    logs: Vec<String>,
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
                let name = EventName::new(name.clone()).expect("fixture event names are valid");
                Ok(engine.raise(Event::internal(name))?)
            }
            Effect::Log { label, .. } => {
                self.logs.push(label.clone().unwrap_or_default());
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

fn ids(states: &[StateId]) -> Vec<&str> {
    states.iter().map(StateId::as_str).collect()
}

fn external(name: &str) -> Event {
    let mut event = Event::internal(EventName::new(name.to_owned()).expect("valid event name"));
    event.event_type = EventType::External;
    event
}

/// `on-entry` logs each state's id so the recorder observes entry order.
fn entry_log(id: &str) -> String {
    format!(
        r#"<onentry><log label="enter:{id}"/></onentry><onexit><log label="exit:{id}"/></onexit>"#
    )
}

fn parallel_chart() -> String {
    format!(
        r#"<state id="out">{out}<transition event="go" target="a2 b2"/></state>
  <parallel id="p">{p}
    <state id="a" initial="a1">{a}<state id="a1">{a1}</state><state id="a2">{a2}</state></state>
    <state id="b" initial="b1">{b}<state id="b1">{b1}</state><state id="b2">{b2}</state></state>
    <state id="c">{c}</state>
    <transition event="swap" target="a1 b1"/>
  </parallel>"#,
        out = entry_log("out"),
        p = entry_log("p"),
        a = entry_log("a"),
        a1 = entry_log("a1"),
        a2 = entry_log("a2"),
        b = entry_log("b"),
        b1 = entry_log("b1"),
        b2 = entry_log("b2"),
        c = entry_log("c"),
    )
}

#[test]
fn transition_from_outside_enters_every_target_region_and_defaults_the_rest() {
    let mut engine =
        Engine::new(machine(&parallel_chart(), "out")).expect("multi-target chart is supported");
    let mut recorder = Recorder::default();
    engine
        .run(Command::Start, &mut recorder)
        .expect("start enters out");
    recorder.logs.clear();
    let report = engine
        .run(Command::Send(&external("go")), &mut recorder)
        .expect("multi-target transition runs");
    assert_eq!(
        recorder.logs,
        [
            "exit:out", "enter:p", "enter:a", "enter:a2", "enter:b", "enter:b2", "enter:c"
        ],
        "W3C entry order: ancestors first, document order, explicit targets replace default initial states"
    );
    assert_eq!(
        ids(&report.result.leaves),
        ["a2", "b2", "c"],
        "one leaf per region, the uncovered region entered by default"
    );
    assert_eq!(
        ids(&report.result.configuration),
        ["p", "a", "a2", "b", "b2", "c"]
    );
    assert_eq!(report.result.status, EngineStatus::Running);
}

#[test]
fn transition_from_the_parallel_exits_and_reenters_it_with_every_target() {
    let mut engine =
        Engine::new(machine(&parallel_chart(), "out")).expect("multi-target chart is supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    engine
        .run(Command::Send(&external("go")), &mut recorder)
        .expect("go");
    recorder.logs.clear();
    let report = engine
        .run(Command::Send(&external("swap")), &mut recorder)
        .expect("swap runs");
    // A parallel is never a transition domain, so the external transition
    // exits and re-enters `p` (SCXML 3.13 getTransitionDomain/findLCCA).
    assert_eq!(
        recorder.logs,
        [
            "exit:c", "exit:b2", "exit:b", "exit:a2", "exit:a", "exit:p", "enter:p", "enter:a",
            "enter:a1", "enter:b", "enter:b1", "enter:c"
        ]
    );
    assert_eq!(ids(&report.result.leaves), ["a1", "b1", "c"]);
}

#[test]
fn initial_attribute_with_several_targets_enters_nested_parallel_regions() {
    // Official case 364 shape: a compound `initial` naming one leaf in each
    // region of a parallel nested below a sibling-bearing compound child.
    let body = r#"<state id="s1" initial="s11p112 s11p122">
    <state id="s11" initial="s111">
      <state id="s111"/>
      <parallel id="s11p1">
        <state id="s11p11" initial="s11p111"><state id="s11p111"/><state id="s11p112"/></state>
        <state id="s11p12" initial="s11p121"><state id="s11p121"/><state id="s11p122"/></state>
      </parallel>
    </state>
  </state>"#;
    let mut engine = Engine::new(machine(body, "s1")).expect("multi-target initial is supported");
    let report = engine
        .run(Command::Start, &mut Recorder::default())
        .expect("start enters both targets");
    assert_eq!(ids(&report.result.leaves), ["s11p112", "s11p122"]);
    assert_eq!(
        ids(&report.result.entered_states),
        [
            "s1", "s11", "s11p1", "s11p11", "s11p112", "s11p12", "s11p122"
        ]
    );
}

#[test]
fn document_initial_with_several_targets_enters_top_level_parallel_regions() {
    let body = r#"<parallel id="p">
    <state id="a" initial="a1"><state id="a1"/><state id="a2"/></state>
    <state id="b" initial="b1"><state id="b1"/><state id="b2"/></state>
  </parallel>"#;
    let mut engine = Engine::new(machine(body, "a2 b2")).expect("multi-target scxml initial");
    let report = engine
        .run(Command::Start, &mut Recorder::default())
        .expect("start enters both targets");
    assert_eq!(ids(&report.result.leaves), ["a2", "b2"]);
}

#[test]
fn targets_that_are_not_orthogonal_never_reach_the_engine() {
    // SCXML 3.13 / validator SCXML030: targets must lie in distinct regions
    // of one parallel. The canonical lowering rejects the rest, so the
    // engine never sees an illegal configuration.
    for targets in ["a1 a2", "a a1", "a1 out"] {
        let source = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="out" datamodel="null">
  <state id="out"><transition event="go" target="{targets}"/></state>
  <parallel id="p">
    <state id="a"><state id="a1"/><state id="a2"/></state>
    <state id="b"/>
  </parallel>
</scxml>"#
        );
        let document = parse_xml(SourceText::new(SOURCE, "m.scxml", source))
            .expect("fixture SCXML is well-formed XML");
        assert!(
            lower(&document).is_err(),
            "non-orthogonal target set {targets:?} must be rejected by lowering"
        );
    }
}

#[test]
fn a_state_listed_twice_is_one_legal_state_specification() {
    // SCXML 3.11: "a legal state specification to be a set of states such
    // that 1) no state is an ancestor of any other state on the list, and 2)
    // a full legal state configuration results when all ancestors and
    // default initial descendants have been added". `a2 a2` denotes the set
    // {a2}: no state is an ancestor of any *other* state on the list.
    let body = r#"<state id="out"><transition event="go" target="a2 a2"/></state>
  <parallel id="p">
    <state id="a" initial="a1"><state id="a1"/><state id="a2"><onentry><log label="enter:a2"/></onentry></state></state>
    <state id="b"/>
  </parallel>"#;
    let chart = machine(body, "out");
    let go = chart
        .transitions
        .iter()
        .find(|transition| transition.events == ["go"])
        .expect("the go transition lowers");
    assert_eq!(go.targets.len(), 1, "the set names a2 once");
    let mut engine = Engine::new(chart).expect("a repeated target is legal");
    let mut recorder = Recorder::default();
    engine
        .run(Command::Start, &mut recorder)
        .expect("start enters out");
    let report = engine
        .run(Command::Send(&external("go")), &mut recorder)
        .expect("the repeated-target transition runs");
    assert_eq!(ids(&report.result.leaves), ["a2", "b"]);
    assert_eq!(recorder.logs, ["enter:a2"], "a2 is entered once");

    let initial = machine(r#"<state id="x"/><state id="y"/>"#, "x x");
    let mut engine = Engine::new(initial).expect("a repeated initial target is legal");
    let report = engine
        .run(Command::Start, &mut Recorder::default())
        .expect("start enters the repeated initial target once");
    assert_eq!(ids(&report.result.leaves), ["x"]);
}
