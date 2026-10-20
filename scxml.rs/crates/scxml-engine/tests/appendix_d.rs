//! W3C SCXML Appendix D algorithm details that no official IRP case pins
//! down: eventless re-selection after an internal event that enables
//! nothing (`mainEventLoop`), `<history>` children of `<parallel>`
//! (`getChildStates` excludes history pseudostates), the order of enabled
//! transitions' content (3.13 runs it in document order, not in Appendix D's
//! selection order), the
//! atomicity of a rejected selection or microstep, and the top-level
//! `<final>` `<donedata>` return (`exitInterpreter`/`returnDoneEvent`).
//!
//! Every chart lowers through the canonical compiler and runs through
//! [`Engine::run`]; the recording executor only routes `<raise>` back into
//! the engine and records `<log>` labels in execution order.

use scxml_compiler_core::{MachineIr, SCXML_NAMESPACE, lower};
use scxml_engine::{
    BatchEnd, Command, Effect, EffectExecutor, EffectFailure, Engine, EngineError, EngineSnapshot,
    EngineStatus, EvaluationError, GuardEvaluator, PlannedEffect, Previews, RunFailureKind,
    RunStage, SnapshotOptions, TransitionListing,
};
use scxml_syntax::{SourceId, SourceText, parse_xml};
use scxml_types::{Event, EventName, EventType, StateId};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

/// Guard outcome for one condition expression and the zero-based count of
/// guard evaluations before it.
type Guard = fn(&str, usize) -> Result<bool, EvaluationError>;

#[derive(Default)]
struct Recorder {
    logs: Vec<String>,
    /// The configuration the engine's updates installed, as a host keeps it.
    configuration: Vec<StateId>,
    /// Guard evaluator; absent, every guard is unavailable.
    guard: Option<Guard>,
    /// Guard evaluator that reads the bound `_event`; consulted before
    /// [`Self::guard`] when present.
    event_guard: Option<fn(&str, Option<&Event>) -> bool>,
    guard_calls: usize,
}

impl GuardEvaluator for Recorder {
    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, EvaluationError> {
        let calls = self.guard_calls;
        self.guard_calls += 1;
        if let Some(event_guard) = self.event_guard {
            return Ok(event_guard(expression, event));
        }
        self.guard
            .map_or(Err(EvaluationError::Unavailable), |guard| {
                guard(expression, calls)
            })
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
        exited: &[StateId],
        entered: &[StateId],
    ) -> Result<(), EngineError> {
        self.configuration.retain(|state| !exited.contains(state));
        self.configuration.extend_from_slice(entered);
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
            Effect::Raise(name, ..) => {
                let name = EventName::new(name.clone()).expect("fixture event names are valid");
                Ok(engine.raise(Event::internal(name))?)
            }
            Effect::Log { label, .. } => {
                self.logs.push(label.clone().unwrap_or_default());
                Ok(())
            }
            Effect::ReturnDoneEvent(_) => {
                self.logs
                    .push(format!("return-donedata in {:?}", ids(&self.configuration)));
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

/// The whole engine state a rejected command must leave unchanged: the
/// read-only snapshot with every transition and its previews.
fn observable(engine: &Engine) -> EngineSnapshot {
    engine
        .snapshot(SnapshotOptions {
            transitions: TransitionListing::Include,
            previews: Previews::Include,
        })
        .expect("an idle engine snapshots")
}

fn external(name: &str) -> Event {
    let mut event = Event::internal(EventName::new(name.to_owned()).expect("valid event name"));
    event.event_type = EventType::External;
    event
}

fn send(engine: &mut Engine, recorder: &mut Recorder, name: &str) -> Vec<String> {
    let report = engine
        .run(Command::Send(&external(name)), recorder)
        .unwrap_or_else(|failure| panic!("event {name} runs: {:?}", failure.error));
    ids(&report.result.leaves)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// A parallel `p` whose regions `a` and `b` can both finish, plus a history
/// pseudostate `h` of the given type as a direct child of `p`. The fallback
/// of `h` targets `a2` and logs, so any expansion of `h` is observable.
fn parallel_with_history(history_type: &str) -> String {
    format!(
        r#"<state id="out">
    <transition event="enter" target="p"/>
    <transition event="restore" target="h"/>
  </state>
  <parallel id="p">
    <state id="a" initial="a1">
      <state id="a1"><transition event="step" target="a2"/></state>
      <state id="a2"><transition event="finish" target="af"/></state>
      <final id="af"/>
    </state>
    <state id="b" initial="b1">
      <state id="b1"><transition event="step" target="b2"/></state>
      <state id="b2"><transition event="finish" target="bf"/></state>
      <final id="bf"/>
    </state>
    <history id="h" type="{history_type}">
      <transition target="a2"><log label="h-default"/></transition>
    </history>
    <transition event="leave" target="out"/>
    <transition event="done.state.p" target="done"><log label="done.state.p"/></transition>
  </parallel>
  <final id="done"/>"#
    )
}

#[test]
fn parallel_default_entry_never_expands_a_history_child_as_a_region() {
    for history_type in ["shallow", "deep"] {
        let mut engine = Engine::new(machine(&parallel_with_history(history_type), "out"))
            .expect("parallel with a history child is supported");
        let mut recorder = Recorder::default();
        engine.run(Command::Start, &mut recorder).expect("start");
        let active = send(&mut engine, &mut recorder, "enter");
        assert_eq!(
            active,
            ["a1", "b1"],
            "{history_type}: W3C getChildStates excludes <history>, so default entry enters one leaf per region"
        );
        assert!(
            recorder.logs.is_empty(),
            "{history_type}: history fallback content must not run on default entry: {:?}",
            recorder.logs
        );
    }
}

#[test]
fn parallel_with_a_history_child_raises_done_state_when_every_region_finishes() {
    for history_type in ["shallow", "deep"] {
        let mut engine = Engine::new(machine(&parallel_with_history(history_type), "out"))
            .expect("parallel with a history child is supported");
        let mut recorder = Recorder::default();
        engine.run(Command::Start, &mut recorder).expect("start");
        send(&mut engine, &mut recorder, "enter");
        send(&mut engine, &mut recorder, "step");
        let report = engine
            .run(Command::Send(&external("finish")), &mut recorder)
            .expect("finish runs");
        assert_eq!(
            recorder.logs,
            ["done.state.p"],
            "{history_type}: the history pseudostate is not a region that must finish"
        );
        assert_eq!(report.result.status, EngineStatus::Finished);
    }
}

/// SCXML 1.0 Appendix D raises `done.state.<ID>` for a `<parallel>` only
/// as the grandparent of a `<final>` being entered ("if
/// isParallelState(grandparent): if
/// getChildStates(grandparent).every(isInFinalState)"). A `<parallel>` whose
/// regions are all `<parallel>` is no `<final>`'s grandparent: when every
/// region finishes, each region raises its own `done.state`, and the outer
/// one raises none.
#[test]
fn a_parallel_whose_regions_are_all_parallel_never_raises_done_state() {
    let body = r#"<parallel id="p">
    <parallel id="q"><state id="a" initial="a1"><state id="a1"><transition event="finish" target="af"/></state><final id="af"/></state></parallel>
    <parallel id="r"><state id="b" initial="b1"><state id="b1"><transition event="finish" target="bf"/></state><final id="bf"/></state></parallel>
    <transition event="done.state.q"><log label="done.state.q"/></transition>
    <transition event="done.state.r"><log label="done.state.r"/></transition>
    <transition event="done.state.p"><log label="done.state.p"/></transition>
  </parallel>"#;
    let mut engine = Engine::new(machine(body, "p")).expect("nested parallels are supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    assert_eq!(send(&mut engine, &mut recorder, "finish"), ["af", "bf"]);
    assert_eq!(recorder.logs, ["done.state.q", "done.state.r"]);
}

#[test]
fn parallel_history_restores_the_recorded_regions() {
    // Deep history records the active leaves (a2, b1); shallow history
    // records the active regions (a, b), which re-enter by default.
    for (history_type, restored) in [("deep", ["a2", "b1"]), ("shallow", ["a1", "b1"])] {
        let body = parallel_with_history(history_type).replace(
            r#"<state id="b1"><transition event="step" target="b2"/></state>"#,
            r#"<state id="b1"/>"#,
        );
        let mut engine =
            Engine::new(machine(&body, "out")).expect("parallel with a history child is supported");
        let mut recorder = Recorder::default();
        engine.run(Command::Start, &mut recorder).expect("start");
        send(&mut engine, &mut recorder, "enter");
        assert_eq!(send(&mut engine, &mut recorder, "step"), ["a2", "b1"]);
        assert_eq!(send(&mut engine, &mut recorder, "leave"), ["out"]);
        let active = send(&mut engine, &mut recorder, "restore");
        assert_eq!(active, restored, "{history_type} history of p");
        assert!(
            recorder.logs.is_empty(),
            "{history_type}: a stored history value never runs fallback content: {:?}",
            recorder.logs
        );
    }
}

#[test]
fn parallel_history_without_a_stored_value_runs_its_fallback_content_once() {
    // W3C addDescendantStatesToEnter records the fallback content as
    // defaultHistoryContent[p]; enterStates runs it once, when p itself is
    // entered, not once per entered region.
    for history_type in ["shallow", "deep"] {
        let mut engine = Engine::new(machine(&parallel_with_history(history_type), "out"))
            .expect("parallel with a history child is supported");
        let mut recorder = Recorder::default();
        engine.run(Command::Start, &mut recorder).expect("start");
        let active = send(&mut engine, &mut recorder, "restore");
        assert_eq!(
            active,
            ["a2", "b1"],
            "{history_type}: fallback target plus default region"
        );
        assert_eq!(
            recorder.logs,
            ["h-default"],
            "{history_type}: default history content runs exactly once"
        );
    }
}

#[test]
fn a_history_of_a_parallel_without_regions_has_no_legal_default() {
    // SCXML 3.10.2: the default transition "MUST contain only immediate
    // children of the parent state"; a regionless parallel has none, so
    // the only targets left are not descendants and lowering refuses them.
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="p" datamodel="null">
  <parallel id="p"><history id="h"><transition target="p"/></history></parallel>
</scxml>"#
    );
    let document = parse_xml(SourceText::new(SOURCE, "m.scxml", source))
        .expect("fixture SCXML is well-formed XML");
    assert!(lower(&document).is_err());
}

#[test]
fn transition_content_runs_in_the_document_order_of_the_transitions() {
    // SCXML 3.13: "It MUST then execute the executable content contained in
    // the transitions in document order." a1 selects a transition on its
    // region `a`; b1 finds none on itself or on `b` and selects one on the
    // parallel `p`. Both are targetless, so neither preempts the other.
    // `p`'s transition precedes `a`'s in the document, so its content runs
    // first, although Appendix D's (informative) selectTransitions orders
    // the set by selecting atomic state (docs/spec-errata.md).
    let body = r#"<parallel id="p">
    <transition event="e"><log label="p"/></transition>
    <state id="a">
      <state id="a1"/>
      <transition event="e"><log label="a"/></transition>
    </state>
    <state id="b">
      <state id="b1"/>
    </state>
  </parallel>"#;
    let mut engine = Engine::new(machine(body, "p")).expect("parallel chart is supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    assert_eq!(send(&mut engine, &mut recorder, "e"), ["a1", "b1"]);
    assert_eq!(recorder.logs, ["p", "a"]);
}

#[test]
fn rejected_send_after_a_recoverable_guard_error_leaves_the_engine_unchanged() {
    // The first guard fails recoverably (error.execution, condition false);
    // the second fails fatally, so selection rejects the Send. The run
    // contract says a Rejected command leaves the engine unchanged: the
    // recoverable error must not stay queued.
    let body = r#"<state id="s">
    <transition event="e" cond="recoverable" target="t"/>
    <transition event="e" cond="fatal" target="t"/>
  </state>
  <state id="t"/>"#;
    let mut engine = Engine::new(machine(body, "s")).expect("guarded chart is supported");
    let mut recorder = Recorder {
        guard: Some(|expression, _| match expression {
            "recoverable" => Err(EvaluationError::InvalidInput),
            _ => Err(EvaluationError::Platform),
        }),
        ..Recorder::default()
    };
    let started = engine.run(Command::Start, &mut recorder).expect("start");
    assert!(started.stable);
    let before = observable(&engine);
    let failure = engine
        .run(Command::Send(&external("e")), &mut recorder)
        .expect_err("the fatal guard rejects the Send");
    assert_eq!(failure.kind, RunFailureKind::Rejected);
    assert_eq!(
        failure.error,
        EngineError::Evaluation(EvaluationError::Platform)
    );
    assert_eq!(
        observable(&engine),
        before,
        "a rejected Send changes nothing"
    );
    // The engine stays reusable: a retry with a healthy evaluator is
    // accepted and ignores the event.
    recorder.guard = Some(|_, _| Ok(false));
    let retried = engine
        .run(Command::Send(&external("e")), &mut recorder)
        .expect("the retried Send is accepted");
    assert_eq!(ids(&retried.result.leaves), ["s"]);
    assert!(retried.stable);
    assert!(observable(&engine).internal.is_empty());
}

// The per-step effect bound applies to wasm builds only, so this rollback is
// exercised on wasm32; natively the same rollback is covered by
// `failed_internal_scan_restores_the_internal_queue_exactly`.
#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_only_rejected_microstep_after_a_recoverable_guard_error_leaves_the_engine_unchanged() {
    // Selection succeeds (the recoverable guard queues error.execution and
    // the unguarded transition is enabled), but planning the microstep
    // exceeds the per-step effect bound. The Send is rejected, so the
    // selection's error event must not stay queued either.
    let logs = "<log label=\"x\"/>".repeat(scxml_engine::MAX_EFFECTS_PER_COMMAND);
    let body = format!(
        r#"<state id="s">
    <transition event="e" cond="recoverable" target="t"/>
    <transition event="e" target="t"><log label="go"/></transition>
  </state>
  <state id="t"><onentry>{logs}</onentry></state>"#
    );
    let mut engine = Engine::new(machine(&body, "s")).expect("guarded chart is supported");
    let mut recorder = Recorder {
        guard: Some(|_, _| Err(EvaluationError::InvalidInput)),
        ..Recorder::default()
    };
    engine.run(Command::Start, &mut recorder).expect("start");
    let before = observable(&engine);
    let failure = engine
        .run(Command::Send(&external("e")), &mut recorder)
        .expect_err("the oversized microstep is rejected");
    assert_eq!(failure.kind, RunFailureKind::Rejected);
    assert_eq!(failure.error, EngineError::EffectLimit);
    assert_eq!(
        observable(&engine),
        before,
        "a rejected Send changes nothing"
    );
}

#[test]
fn failed_internal_scan_restores_the_internal_queue_exactly() {
    // Start enters s0, whose onentry raises e1. The first eventless
    // selection finds nothing; e1's selection hits a recoverable guard
    // error, then a fatal one in the same selection. The rollback must
    // restore the queue to exactly [e1], without the error.execution the
    // failed scan produced.
    let body = r#"<state id="s0">
    <onentry><raise event="e1"/></onentry>
    <transition event="e1" cond="recoverable" target="x"/>
    <transition event="e1" cond="fatal" target="x"/>
    <transition cond="eventless" target="x"/>
  </state>
  <state id="x"/>"#;
    let mut engine = Engine::new(machine(body, "s0")).expect("guarded chart is supported");
    let mut recorder = Recorder {
        guard: Some(|expression, calls| match (expression, calls) {
            ("recoverable", _) => Err(EvaluationError::InvalidInput),
            ("eventless", 0) => Ok(false),
            _ => Err(EvaluationError::Platform),
        }),
        ..Recorder::default()
    };
    let failure = engine
        .run(Command::Start, &mut recorder)
        .expect_err("the fatal guard fails the scan");
    assert_eq!(
        failure.error,
        EngineError::Evaluation(EvaluationError::Platform)
    );
    let pending: Vec<String> = observable(&engine)
        .internal
        .iter()
        .map(|event| event.name.as_str().to_owned())
        .collect();
    assert_eq!(
        pending,
        ["e1"],
        "the failed scan's error events are discarded"
    );
}

#[test]
fn top_level_final_plans_its_donedata_return_after_its_onexit() {
    // W3C exitInterpreter: the top-level final's onexit runs, the state
    // leaves the configuration, then returnDoneEvent(s.donedata).
    let body = r#"<state id="s"><transition event="go" target="end"/></state>
  <final id="end">
    <onexit><log label="exit:end"/></onexit>
    <donedata><param name="x" expr="1"/></donedata>
  </final>"#;
    let mut engine = Engine::new(machine(body, "s")).expect("donedata chart is supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    let report = engine
        .run(Command::Send(&external("go")), &mut recorder)
        .expect("go finishes the machine");
    assert_eq!(report.result.status, EngineStatus::Finished);
    assert_eq!(recorder.logs, ["exit:end", "return-donedata in []"]);
}

#[test]
fn top_level_final_without_donedata_plans_no_return() {
    let body = r#"<state id="s"><transition event="go" target="end"/></state>
  <final id="end"><onexit><log label="exit:end"/></onexit></final>"#;
    let mut engine = Engine::new(machine(body, "s")).expect("final chart is supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    engine
        .run(Command::Send(&external("go")), &mut recorder)
        .expect("go finishes the machine");
    assert_eq!(recorder.logs, ["exit:end"]);
}

#[test]
fn an_ignored_internal_event_reselects_eventless_transitions_with_it_bound() {
    // W3C Appendix D `mainEventLoop`: with no enabled eventless transition
    // the loop dequeues one internal event, binds it as `_event`, and
    // selects on it; when it enables nothing, the next iteration runs
    // `selectEventlessTransitions` again, now with that event bound. The
    // eventless guard below is true only while `_event` is `ignored`,
    // which no event-bearing transition consumes.
    let body = r#"<state id="s0">
    <onentry><raise event="ignored"/></onentry>
    <transition cond="bound-event-is-ignored" target="pass"/>
  </state>
  <final id="pass"/>"#;
    let mut engine = Engine::new(machine(body, "s0")).expect("eventless chart is supported");
    let mut recorder = Recorder {
        event_guard: Some(|expression, event| {
            expression == "bound-event-is-ignored"
                && event.is_some_and(|event| event.name.as_str() == "ignored")
        }),
        ..Recorder::default()
    };
    let report = engine.run(Command::Start, &mut recorder).expect("start");
    assert_eq!(
        report.result.status,
        EngineStatus::Finished,
        "the ignored internal event must re-run eventless selection with `_event` bound"
    );
    assert_eq!(ids(&recorder.configuration), ["pass"]);
}

#[test]
fn a_parallel_without_regions_is_active_but_enables_no_transition() {
    // SCXML 3.4.2: the `<state>` children of a `<parallel>` occur "0 or more
    // times". SCXML 3.13 enables a transition only "in atomic state S", and a
    // `<parallel>` is never atomic (3.3), so neither the regionless
    // parallel's own transition nor its parent's is enabled through it.
    let body = r#"<state id="s" initial="p">
    <parallel id="p"><transition event="go" target="f"><log label="p-go"/></transition></parallel>
    <transition event="go" target="f"><log label="s-go"/></transition>
  </state>
  <final id="f"/>"#;
    let mut engine = Engine::new(machine(body, "s")).expect("a regionless parallel is supported");
    let mut recorder = Recorder::default();
    let report = engine.run(Command::Start, &mut recorder).expect("start");
    assert_eq!(
        ids(&report.result.leaves),
        ["p"],
        "the parallel is a configuration leaf"
    );
    assert_eq!(send(&mut engine, &mut recorder, "go"), ["p"]);
    assert!(recorder.logs.is_empty(), "{:?}", recorder.logs);
}

#[test]
fn a_parallel_without_regions_counts_as_final_for_its_parallel_parent() {
    // Appendix D isInFinalState: a <parallel> is final when all of its
    // children are, which holds for one with none; SCXML 3.7.2 then raises
    // done.state.top once the other region reaches a final state.
    let body = r#"<parallel id="top">
    <state id="a" initial="a1">
      <state id="a1"><transition event="finish" target="af"/></state>
      <final id="af"/>
    </state>
    <parallel id="q"/>
    <transition event="done.state.top" target="done"><log label="done.state.top"/></transition>
  </parallel>
  <final id="done"/>"#;
    let mut engine = Engine::new(machine(body, "top")).expect("a regionless parallel is supported");
    let mut recorder = Recorder::default();
    let report = engine.run(Command::Start, &mut recorder).expect("start");
    assert_eq!(ids(&report.result.leaves), ["a1", "q"]);
    let report = engine
        .run(Command::Send(&external("finish")), &mut recorder)
        .expect("finish runs");
    assert_eq!(recorder.logs, ["done.state.top"]);
    assert_eq!(report.result.status, EngineStatus::Finished);
}

#[test]
fn default_history_content_runs_when_the_parent_stays_active() {
    // SCXML 3.10.2: "The Process MUST execute any executable content in the
    // transition after the parent state's onentry handlers". Appendix D
    // (informative) runs it only when the parent itself is entered, so it
    // would drop it here, where `s` is the transition domain and stays
    // active (docs/spec-errata.md). The content runs where the parent's
    // entry would end: before any state entered below `s`.
    let body = r#"<state id="s" initial="s1">
    <onentry><log label="in-s"/></onentry>
    <history id="h"><transition target="s2"><log label="hc"/></transition></history>
    <state id="s1"><transition event="go" target="h"/></state>
    <state id="s2"><onentry><log label="in-s2"/></onentry></state>
  </state>"#;
    let mut engine = Engine::new(machine(body, "s")).expect("history chart is supported");
    let mut recorder = Recorder::default();
    engine.run(Command::Start, &mut recorder).expect("start");
    assert_eq!(send(&mut engine, &mut recorder, "go"), ["s2"]);
    assert_eq!(recorder.logs, ["in-s", "hc", "in-s2"]);
}
