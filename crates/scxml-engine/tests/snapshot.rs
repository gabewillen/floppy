//! The read-only engine snapshot (`docs/session-snapshot.md`, "Session
//! object" and "When a snapshot can be taken").
//!
//! - The transition walk lists what an event could trigger, in the
//!   documented order, with authored `cond` text and the transition domain.
//! - Each listed transition's `exit`/`entry` previews equal the exit and
//!   entry sets the engine uses when that transition fires alone.
//! - Snapshotting evaluates no condition and changes nothing: a run that
//!   snapshots after every command reports exactly what a run that never
//!   snapshots reports.
//!
//! Every event name in the fixture chart triggers exactly one transition
//! and every guard is true, so sending a listed transition's event fires
//! that transition alone. Runs are deterministic: a fixed RNG seed, a fixed
//! case count, and no failure-persistence files.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use scxml_compiler_core::{MachineIr, SCXML_NAMESPACE, TransitionType, lower};
use scxml_engine::{
    BatchEnd, Command, Effect, EffectExecutor, EffectFailure, Engine, EngineError, EngineSnapshot,
    EngineStatus, EvaluationError, GuardEvaluator, HistoryValue, PlannedEffect, Previews,
    RunReport, RunStage, SnapshotOptions, TransitionListing,
};
use scxml_syntax::{SourceId, SourceText, parse_xml};
use scxml_types::{Event, EventName, EventType, StateId};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

const ALL: SnapshotOptions = SnapshotOptions {
    transitions: TransitionListing::Include,
    previews: Previews::Include,
};

/// Every guard is true; `calls` counts evaluations.
#[derive(Default)]
struct Executor {
    calls: usize,
}

impl GuardEvaluator for Executor {
    fn evaluate_condition(
        &mut self,
        _expression: &str,
        _event: Option<&Event>,
    ) -> Result<bool, EvaluationError> {
        self.calls += 1;
        Ok(true)
    }
}

impl EffectExecutor for Executor {
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
        _engine: &mut Engine,
        planned: &PlannedEffect,
        _trigger: Option<&Event>,
    ) -> Result<(), EffectFailure<EngineError>> {
        match planned.effect {
            Effect::Log { .. } => Ok(()),
            _ => Err(EffectFailure::Fatal(EngineError::UnsupportedModel)),
        }
    }

    fn end_batch(&mut self, _end: BatchEnd) {}
}

/// Compound, nested compound, shallow and deep history, transitions that
/// exit the parent of their own history target (so firing records the
/// history they then resolve), an internal transition, a targetless
/// guarded transition, and a parallel with a cross-region transition. No final state and no eventless transition, so
/// every send is exactly one microstep.
const CHART: &str = r#"
  <state id="idle">
    <transition event="t.enter" target="work"/>
    <transition event="t.par" target="p"/>
    <transition event="t.shallow" target="work.h"/>
    <transition event="t.deep" target="work.dh"/>
  </state>
  <state id="work" initial="w1">
    <history id="work.h" type="shallow"><transition target="w1"><log label="h"/></transition></history>
    <history id="work.dh" type="deep"><transition target="w2.b"/></history>
    <state id="w1"><transition event="t.w1w2" target="w2"/></state>
    <state id="w2" initial="w2.a">
      <state id="w2.a"><transition event="t.ab" target="w2.b"/></state>
      <state id="w2.b"><transition event="t.ba" target="w2.a"/></state>
      <transition event="t.inner" type="internal" target="w2.b"/>
    </state>
    <transition event="t.leave" target="idle"/>
    <transition event="t.noop t.noop2" cond="1 &lt; 2"><log label="n"/></transition>
    <transition event="t.reenter" target="work.h"/>
    <transition event="t.redeep" target="work.dh"/>
  </state>
  <parallel id="p">
    <state id="r1" initial="r1a">
      <state id="r1a"><transition event="t.r1" target="r1b"/></state>
      <state id="r1b"/>
    </state>
    <state id="r2" initial="r2a">
      <state id="r2a"><transition event="t.cross" target="r1b"/></state>
      <state id="r2b"/>
    </state>
    <transition event="t.out" target="idle"/>
  </parallel>"#;

const EVENTS: [&str; 17] = [
    "t.enter",
    "t.par",
    "t.shallow",
    "t.deep",
    "t.w1w2",
    "t.ab",
    "t.ba",
    "t.inner",
    "t.leave",
    "t.noop",
    "t.r1",
    "t.cross",
    "t.out",
    "t.unknown",
    "t.noop2",
    "t.reenter",
    "t.redeep",
];

fn chart() -> MachineIr {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="idle" datamodel="null">{CHART}</scxml>"#
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

fn sid(id: &str) -> StateId {
    StateId::new(id).expect("valid state id")
}

fn sids(ids: &[&str]) -> Vec<StateId> {
    ids.iter().map(|id| sid(id)).collect()
}

fn run(engine: &mut Engine, executor: &mut Executor, event: &str) -> RunReport {
    engine
        .run(Command::Send(&external(event)), executor)
        .unwrap_or_else(|failure| panic!("{event} runs: {:?}", failure.error))
}

/// A started engine after `events`.
fn engine_after(events: &[&str]) -> Engine {
    let mut engine = Engine::new(chart()).expect("fixture chart is supported");
    let mut executor = Executor::default();
    engine.run(Command::Start, &mut executor).expect("start");
    for event in events {
        run(&mut engine, &mut executor, event);
    }
    engine
}

#[test]
fn transitions_follow_the_contract_walk_with_authored_cond_and_domain() {
    let engine = engine_after(&["t.enter", "t.w1w2"]);
    let image = engine.snapshot(ALL).expect("snapshot");
    assert_eq!(image.status, EngineStatus::Running);
    assert_eq!(
        image.name, None,
        "SCXML 5.10: no `name` attribute, no `_name` value"
    );
    assert_eq!(image.atomic, sids(&["w2.a"]));
    assert_eq!(image.configuration, sids(&["work", "w2", "w2.a"]));
    let listed: Vec<(&str, u32)> = image
        .transitions
        .iter()
        .map(|transition| (transition.source.as_str(), transition.index))
        .collect();
    // w2.a, then its ancestors w2 and work; eventless and duplicate
    // transitions never appear.
    assert_eq!(
        listed,
        [
            ("w2.a", 0),
            ("w2", 0),
            ("work", 0),
            ("work", 1),
            ("work", 2),
            ("work", 3)
        ]
    );
    let inner = &image.transitions[1];
    assert_eq!(inner.transition_type, TransitionType::Internal);
    assert_eq!(inner.domain, Some(sid("w2")), "internal: the source");
    assert_eq!(inner.exit, sids(&["w2.a"]));
    assert_eq!(inner.entry, sids(&["w2.b"]));
    let leave = &image.transitions[2];
    assert_eq!(leave.domain, None, "a domain of <scxml> is omitted");
    assert_eq!(leave.exit, sids(&["w2.a", "w2", "work"]), "exit order");
    assert_eq!(leave.entry, sids(&["idle"]));
    let noop = &image.transitions[3];
    assert_eq!(noop.events, ["t.noop", "t.noop2"]);
    assert_eq!(noop.cond.as_deref(), Some("1 < 2"));
    assert!(noop.targets.is_empty() && noop.domain.is_none());
    assert!(noop.exit.is_empty() && noop.entry.is_empty());

    let bare = engine
        .snapshot(SnapshotOptions {
            transitions: TransitionListing::Include,
            previews: Previews::Omit,
        })
        .expect("snapshot");
    assert!(
        bare.transitions
            .iter()
            .all(|transition| transition.exit.is_empty() && transition.entry.is_empty())
    );
    assert_eq!(bare.transitions.len(), image.transitions.len());
    let none = engine
        .snapshot(SnapshotOptions::default())
        .expect("snapshot");
    assert!(none.transitions.is_empty());
}

#[test]
fn history_values_are_listed_and_previews_resolve_through_them() {
    let engine = engine_after(&["t.enter", "t.w1w2", "t.ab", "t.leave"]);
    let image = engine.snapshot(ALL).expect("snapshot");
    assert_eq!(
        image.history,
        [
            HistoryValue {
                state: sid("work.h"),
                value: sids(&["w2"]),
            },
            HistoryValue {
                state: sid("work.dh"),
                value: sids(&["w2.b"]),
            },
        ]
    );
    let deep = image
        .transitions
        .iter()
        .find(|transition| transition.events == ["t.deep"])
        .expect("t.deep is listed from idle");
    assert_eq!(deep.entry, sids(&["work", "w2", "w2.b"]));
    let shallow = image
        .transitions
        .iter()
        .find(|transition| transition.events == ["t.shallow"])
        .expect("t.shallow is listed from idle");
    assert_eq!(shallow.entry, sids(&["work", "w2", "w2.a"]));
}

/// A transition that exits the parent of its own history target records
/// that history on exit and then resolves the target through it, so the
/// preview must show the state the transition re-enters, not the history
/// default or an older recorded value.
#[test]
fn previews_resolve_through_the_history_their_own_exit_records() {
    let script = ["t.enter", "t.w1w2"];
    let image = engine_after(&script).snapshot(ALL).expect("snapshot");
    assert!(image.history.is_empty(), "nothing recorded yet");
    for (event, exit, entry) in [
        (
            "t.reenter",
            &["w2.a", "w2", "work"],
            &["work", "w2", "w2.a"],
        ),
        ("t.redeep", &["w2.a", "w2", "work"], &["work", "w2", "w2.a"]),
    ] {
        let preview = image
            .transitions
            .iter()
            .find(|transition| transition.events == [event])
            .unwrap_or_else(|| panic!("{event} is listed from w2.a"));
        assert_eq!(preview.exit, sids(exit), "{event} exit");
        assert_eq!(preview.entry, sids(entry), "{event} entry");
        let mut fired = engine_after(&script);
        let report = run(&mut fired, &mut Executor::default(), event);
        assert_eq!(
            report.result.exited_states, preview.exit,
            "{event} fired exit"
        );
        assert_eq!(
            report.result.entered_states, preview.entry,
            "{event} fired entry"
        );
    }

    // An older recorded value is overwritten by the exit, too.
    let script = ["t.enter", "t.leave", "t.enter", "t.w1w2", "t.ab"];
    let image = engine_after(&script).snapshot(ALL).expect("snapshot");
    assert_eq!(
        image.history,
        [
            HistoryValue {
                state: sid("work.h"),
                value: sids(&["w1"]),
            },
            HistoryValue {
                state: sid("work.dh"),
                value: sids(&["w1"]),
            },
        ]
    );
    for event in ["t.reenter", "t.redeep"] {
        let preview = image
            .transitions
            .iter()
            .find(|transition| transition.events == [event])
            .unwrap_or_else(|| panic!("{event} is listed from w2.b"));
        let entry: &[&str] = if event == "t.reenter" {
            &["work", "w2", "w2.a"]
        } else {
            &["work", "w2", "w2.b"]
        };
        assert_eq!(preview.entry, sids(entry), "{event} entry");
    }
}

#[test]
fn a_parallel_lists_each_region_then_the_shared_ancestor_once() {
    let image = engine_after(&["t.par"]).snapshot(ALL).expect("snapshot");
    assert_eq!(image.atomic, sids(&["r1a", "r2a"]));
    let listed: Vec<&str> = image
        .transitions
        .iter()
        .map(|transition| transition.events[0].as_str())
        .collect();
    assert_eq!(listed, ["t.r1", "t.out", "t.cross"]);
    let cross = &image.transitions[2];
    assert_eq!(cross.domain, None, "a parallel is never a domain");
    assert_eq!(cross.exit, sids(&["r2a", "r2", "r1a", "r1", "p"]));
    assert_eq!(cross.entry, sids(&["p", "r1", "r1b", "r2", "r2a"]));
}

fn config() -> Config {
    Config {
        cases: 128,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x05a9_5a08),
        ..Config::default()
    }
}

fn script() -> impl Strategy<Value = Vec<&'static str>> {
    prop::collection::vec(prop::sample::select(EVENTS.as_slice()), 0..12)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn previews_equal_the_sets_the_transition_uses_when_it_fires_alone(script in script()) {
        let engine = engine_after(&script);
        let image = engine.snapshot(ALL).expect("snapshot");
        for transition in &image.transitions {
            let mut fired = engine_after(&script);
            let mut executor = Executor::default();
            let report = run(&mut fired, &mut executor, &transition.events[0]);
            prop_assert_eq!(&report.result.exited_states, &transition.exit, "{:?}", transition);
            prop_assert_eq!(&report.result.entered_states, &transition.entry, "{:?}", transition);
        }
    }

    #[test]
    fn snapshots_evaluate_nothing_and_change_nothing(script in script()) {
        let mut plain = Engine::new(chart()).expect("fixture chart is supported");
        let mut observed = Engine::new(chart()).expect("fixture chart is supported");
        let mut plain_executor = Executor::default();
        let mut observed_executor = Executor::default();
        let take = |engine: &Engine, executor: &Executor| -> Result<EngineSnapshot, TestCaseError> {
            let calls = executor.calls;
            let first = engine.snapshot(ALL).expect("snapshot");
            prop_assert_eq!(executor.calls, calls, "no condition is evaluated");
            let second = engine.snapshot(ALL).expect("snapshot");
            prop_assert_eq!(&first, &second, "deterministic");
            Ok(first)
        };
        take(&observed, &observed_executor)?;
        let a = plain.run(Command::Start, &mut plain_executor).expect("start");
        let b = observed.run(Command::Start, &mut observed_executor).expect("start");
        prop_assert_eq!(a, b);
        for event in &script {
            take(&observed, &observed_executor)?;
            let a = run(&mut plain, &mut plain_executor, event);
            let b = run(&mut observed, &mut observed_executor, event);
            prop_assert_eq!(a, b);
            prop_assert_eq!(plain_executor.calls, observed_executor.calls);
        }
        let image = take(&observed, &observed_executor)?;
        prop_assert_eq!(image, plain.snapshot(ALL).expect("snapshot"));
    }
}

/// Every `id`, `initial`, `target`, `event`, `name`, and `datamodel` below
/// is an XML Schema type whose white space collapses (Part 2 §4.3.6), so
/// its value is the collapsed one: the engine runs and the snapshot names
/// `idle`, `work`, `a`, `b`, and `h`, never a padded name.
#[test]
fn padded_schema_values_run_and_snapshot_as_their_collapsed_values() {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version=" 1.0 " initial=" idle " name="&#10;m " datamodel=" null "><state id=" idle "><transition event=" go " target=" work&#9;"/></state><state id="work " initial=" a"><history id=" h "><transition target="a"/></history><state id=" a "><transition event="next  " target=" b "/></state><state id="&#13;b"/><transition event=" back " target="idle"/></state></scxml>"#
    );
    let document = parse_xml(SourceText::new(SOURCE, "m.scxml", source)).expect("well-formed");
    let mut engine = Engine::new(lower(&document).expect("padded values lower"))
        .expect("the chart is supported");
    let mut executor = Executor::default();
    engine.run(Command::Start, &mut executor).expect("start");
    run(&mut engine, &mut executor, "go");
    let image = engine.snapshot(ALL).expect("snapshot");
    assert_eq!(image.name.as_deref(), Some("m"));
    assert_eq!(image.configuration, sids(&["work", "a"]));
    let listed: Vec<(&str, &[String])> = image
        .transitions
        .iter()
        .map(|transition| (transition.source.as_str(), transition.events.as_slice()))
        .collect();
    assert_eq!(
        listed,
        [
            ("a", &["next".to_owned()][..]),
            ("work", &["back".to_owned()][..])
        ]
    );
    run(&mut engine, &mut executor, "next");
    run(&mut engine, &mut executor, "back");
    let image = engine.snapshot(ALL).expect("snapshot");
    assert_eq!(image.atomic, sids(&["idle"]));
    assert_eq!(
        image.history,
        [HistoryValue {
            state: sid("h"),
            value: sids(&["b"]),
        }]
    );
}
