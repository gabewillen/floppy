//! The floppy bot's body (`bots/floppy/floppy.scx`): the original's Bot lifecycle
//! and processing turns, run through `scx run` on the wall clock as the one
//! participant (`floppy`) of the room (`environments/room/room.scx`, laid
//! out beside it by `support::environment`), which activates it on joining.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! and its schemas under `bots/floppy/`, the ability schemas under
//! `abilities/schemas/`, and the communication with its conversation and
//! speaking, which make no backend call) with
//! `tests/fixtures/phase4/cognition_double.scx` as
//! `abilities/cognition/cognition.scx`, so the body's literal
//! `<invoke src="../../abilities/cognition/cognition.scx">` reaches the double. The
//! double's behavior per turn is the `bot.input` observation's event name
//! (`reply`, `fail`, `stale`, `reboot`, `hang`, `deaf`). Timer bounds are
//! shortened by rewriting the copy's `<data>` expressions.
//!
//! The body sends each event it emits to the room, which writes it as a
//! `log` record labelled with the event's name; the double logs `double.input` and `double.exit`.
//!
//! The double offers `bot.ability.cognition.ignore` and the communication
//! `bot.ability.communication.respond`; the choice tests check the turn's
//! choices, each tagged with its offering session's id (`floppy.choices`,
//! `double.choices`), and what the body takes of the selections the double
//! sends it like a stage (`floppy.focus`) or sends itself (the ones that
//! name no target). `tests/tests/communication.rs` covers what the
//! communication does with what it is sent.
//!
//! One test sets `input` (the original's Bot input) to `['identity']` and puts
//! `tests/fixtures/phase4/input_double.scx` in the identity's place; the
//! double logs `input.sound` and `input.visual` for each `environment.*`
//! stimulus the body fans out to it.
//!
//! The firmware tests set `firmware_src` to `'firmware_double.scx'` and put
//! `tests/fixtures/phase4/firmware_double.scx` beside the body: it offers
//! `timer.set`, logs `firmware.set`, and reports `timer.started`; every
//! other test runs the body's own `firmware/firmware.scx`, which offers
//! nothing.
//!
//! One test runs the real cognition (`abilities/cognition/cognition.scx` and its
//! stages) with no backend: the decision layer's decision fails fast with
//! `unconfigured` and hands the turn on to reasoning, whose model call
//! fails the same way, and so does the turn; its failure, handed back,
//! fails the next turn alike.
//!
//! Every test ends by comparing its whole run with its pinned trace under
//! `tests/fixtures/phase4/traces/` (`support::trace`; `FLOPPY_TRACE_BLESS=1`
//! rewrites them).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Finished, Run, logs, trace, unique_scratch, workspace};

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(300);

/// The cognition the scratch layout gets.
enum Cognition {
    /// The double, with `mode`'s expression replaced (`'ready'` or
    /// `'silent'`).
    Double(&'static str),
    /// The repository's cognition.
    Real,
    /// No `abilities/cognition/cognition.scx` at all.
    Missing,
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("the directory is creatable");
    for entry in std::fs::read_dir(from).expect("the directory is readable") {
        let path = entry.expect("a directory entry").path();
        std::fs::copy(&path, to.join(path.file_name().expect("a file name")))
            .expect("the file is copyable");
    }
}

/// Lays out the body with `cognition`, its `<data>` expressions replaced
/// per `data` (`(id, expr)`), and returns the body's path.
fn layout(cognition: &Cognition, data: &[(&str, &str)]) -> PathBuf {
    let dir = unique_scratch("body");
    let body_dir = dir.join("bots/floppy");
    copy_dir(
        &workspace().join("bots/floppy/schemas"),
        &body_dir.join("schemas"),
    );
    copy_dir(
        &workspace().join("abilities/schemas"),
        &dir.join("abilities/schemas"),
    );
    let mut body = std::fs::read_to_string(workspace().join("bots/floppy/floppy.scx"))
        .expect("the body is readable");
    for (id, expr) in data {
        let start = body
            .find(&format!(r#"<data id="{id}" expr=""#))
            .unwrap_or_else(|| panic!("the body declares <data id=\"{id}\">"));
        let end = start + body[start..].find("/>").expect("the <data> element closes");
        body.replace_range(start..end, &format!(r#"<data id="{id}" expr="{expr}""#));
    }
    let body_path = body_dir.join("floppy.scx");
    std::fs::write(&body_path, body).expect("the body copy is writable");
    std::fs::copy(
        workspace().join("bots/floppy/floppy.js"),
        body_dir.join("floppy.js"),
    )
    .expect("the body's script is copyable");
    // The firmware of a bot with none (decision D160).
    copy_dir(
        &workspace().join("bots/floppy/firmware"),
        &body_dir.join("firmware"),
    );
    let abilities = dir.join("abilities");
    for name in [
        "communication",
        "conversation",
        "speaking",
        "identity",
        "memory",
        "stm",
        "persona",
    ] {
        support::copy_ability(name, &abilities);
    }
    support::copy_listening(&abilities);
    support::double_speech(&abilities, "'encode'");
    let ability = abilities.join("cognition/cognition.scx");
    match cognition {
        Cognition::Double(mode) => {
            let double = std::fs::read_to_string(
                workspace().join("tests/fixtures/phase4/cognition_double.scx"),
            )
            .expect("the double is readable")
            .replace(
                r#"<data id="mode" expr="'ready'"/>"#,
                &format!(r#"<data id="mode" expr="{mode}"/>"#),
            );
            std::fs::create_dir_all(abilities.join("cognition")).expect("creatable");
            std::fs::write(ability, double).expect("the double copy is writable");
        }
        Cognition::Real => {
            // The cognition and its stages.
            for stage in ["cognition", "autonomy", "intuition", "reasoning"] {
                support::copy_ability(stage, &abilities);
            }
            support::copy_tree(
                &workspace().join("abilities/behaviors"),
                &abilities.join("behaviors"),
            );
        }
        Cognition::Missing => {}
    }
    body_path
}

/// Starts the room with the laid-out body as its one bot, `floppy`.
fn start(body: &Path) -> Run {
    start_with(body, &[])
}

/// [`start`] with `params` as the bot's activation settings (decision
/// D107).
fn start_with(body: &Path, params: &[(&str, &str)]) -> Run {
    Run::start(&[support::environment_for(body, params)], &[])
}

fn input(action: &str, data: &str) -> Json {
    json!({"observation": {"event": action, "data": data}})
}

fn label_is(label: &'static str) -> impl Fn(&Json) -> bool {
    move |record| record["type"] == "log" && record["label"] == label
}

/// The `(label, text)` logs whose label is one of `labels`, in order.
fn only(records: &[Json], labels: &[&str]) -> Vec<(String, String)> {
    logs(records)
        .into_iter()
        .filter(|(label, _)| labels.contains(&label.as_str()))
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    support::labelled(expected)
}

/// Waits for the bot to activate: it does on joining the room (decision
/// D107), which logs its bot.activated.
fn activated(run: &mut Run) -> Vec<Json> {
    run.until(WAIT, label_is("bot.activated"))
}

/// Closes the run and asserts its pinned trace.
fn close(run: Run, test: &str) -> Finished {
    let finished = run.close(WAIT);
    trace::assert_trace(test, &finished.all, &[]);
    finished
}

#[test]
fn activation_waits_for_the_cognitions_offers() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    // The bot activates on joining (nothing sends bot.activate), once
    // every ability has offered.
    let records = activated(&mut run);
    assert!(
        only(&records, &["double.input", "floppy.output"]).is_empty(),
        "an activating bot takes no turn: {records:?}"
    );
    let finished = close(run, "activation_waits_for_the_cognitions_offers");
    assert!(
        only(&finished.records, &["bot.activating.failed"]).is_empty(),
        "{:?}",
        finished.records
    );
}

#[test]
fn a_missing_cognition_fails_the_activation() {
    let body = layout(&Cognition::Missing, &[]);
    let run = start(&body);
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    let diagnostic = only(&records, &["floppy.diagnostic"]);
    assert_eq!(diagnostic.len(), 1, "{records:?}");
    assert!(
        diagnostic[0].1.starts_with("error."),
        "the invoke's start error fails the activation: {diagnostic:?}"
    );
    assert!(only(&records, &["bot.activated"]).is_empty(), "{records:?}");
    // The session ended in inactive: the bot left its environment.
    run.until(WAIT, label_is("done.invoke"));
    close(run, "a_missing_cognition_fails_the_activation");
}

#[test]
fn an_ability_that_never_offers_times_the_activation_out() {
    let body = layout(
        &Cognition::Double("'silent'"),
        &[("activation_timeout_ms", "3000")],
    );
    let run = start(&body);
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    assert_eq!(
        only(&records, &["floppy.diagnostic"]),
        pairs(&[(
            "floppy.diagnostic",
            r#"the abilities ["cognition"] were not ready within 3000 ms"#
        )])
    );
    assert!(only(&records, &["bot.activated"]).is_empty(), "{records:?}");
    // Leaving live cancelled the silent double.
    assert!(
        logs(&records)
            .iter()
            .any(|(label, _)| label == "double.exit"),
        "{records:?}"
    );
    close(run, "an_ability_that_never_offers_times_the_activation_out");
}

#[test]
fn deactivate_cancels_the_abilities() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.deactivate", &json!({}));
    let records = run.until(WAIT, label_is("done.invoke"));
    assert_eq!(
        only(&records, &["double.exit", "bot.deactivated", "done.invoke"]),
        pairs(&[
            ("double.exit", "Qwen/Qwen3.8-Flash-Next"),
            ("bot.deactivated", "{}"),
            ("done.invoke", r#"{"invokeid":"floppy"}"#),
        ]),
        "the child's exit handler runs before the body reports, and the session ends in inactive"
    );
    // The bot left: nothing takes a turn any more.
    run.event("bot.input", &input("reply", "ignored"));
    let finished = close(run, "deactivate_cancels_the_abilities");
    assert!(
        only(&finished.records, &["double.input"]).is_empty(),
        "{:?}",
        finished.records
    );
}

#[test]
fn deactivate_while_activating_is_answered_and_cancels_the_abilities() {
    let body = layout(
        &Cognition::Double("'silent'"),
        &[("activation_timeout_ms", "60000")],
    );
    let mut run = start(&body);
    // The silent cognition never offers, so the bot is still activating.
    run.event("bot.deactivate", &json!({}));
    let records = run.until(WAIT, label_is("done.invoke"));
    // Every accepted bot.deactivate is answered (the original's
    // activation_cancelling, a2cbaa8): never activated, the bot still
    // reports bot.deactivated once its abilities are cancelled.
    assert_eq!(
        only(
            &records,
            &[
                "double.exit",
                "bot.activated",
                "bot.activating.failed",
                "bot.deactivated",
                "done.invoke"
            ]
        ),
        pairs(&[
            ("double.exit", "Qwen/Qwen3.8-Flash-Next"),
            ("bot.deactivated", "{}"),
            ("done.invoke", r#"{"invokeid":"floppy"}"#),
        ]),
        "{records:?}"
    );
    close(
        run,
        "deactivate_while_activating_is_answered_and_cancels_the_abilities",
    );
}

#[test]
fn the_activation_passes_its_model_to_the_cognition() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start_with(&body, &[("model", "'some/model'")]);
    run.until(WAIT, label_is("bot.activated"));
    run.event("bot.deactivate", &json!({}));
    let records = run.until(WAIT, label_is("bot.deactivated"));
    assert_eq!(
        only(&records, &["double.exit"]),
        pairs(&[("double.exit", "some/model")])
    );
    close(run, "the_activation_passes_its_model_to_the_cognition");
}

#[test]
fn a_reboot_re_enters_live() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    // From the cognition: live is re-entered, the cognition restarts and
    // the bot activates again.
    run.event("bot.input", &input("reboot", "please"));
    let records = run.until(WAIT, label_is("bot.activated"));
    assert_eq!(
        only(
            &records,
            &["double.input", "double.exit", "bot.reboot", "bot.activated"]
        ),
        pairs(&[
            ("double.input", r#"1 reboot "please""#),
            ("double.exit", "Qwen/Qwen3.8-Flash-Next"),
            (
                "bot.reboot",
                r#"{"reason":"cognition_child_teardown_failed"}"#
            ),
            ("bot.activated", "{}"),
        ])
    );
    // The restarted cognition takes the next turn.
    run.event("bot.input", &input("reply", "again"));
    let records = run.until(WAIT, label_is("floppy.output"));
    assert_eq!(
        only(&records, &["double.input"]),
        pairs(&[("double.input", r#"2 reply "again""#)])
    );
    close(run, "a_reboot_re_enters_live");
}

#[test]
fn a_turn_completes_with_the_cognitions_output() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("reply", "hello"));
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    let output = r#"[{"data":{"text":"reply 1"},"event":"bot.ability.communication.respond","target":"session.floppy.communication"}]"#;
    assert_eq!(
        only(
            &records,
            &[
                "double.input",
                "double.participants",
                "floppy.output",
                "bot.processing.completed"
            ]
        ),
        pairs(&[
            ("double.input", r#"1 reply "hello""#),
            // A device's bot.input names no one (the original's f6c4e08).
            ("double.participants", "[]"),
            (
                "floppy.output",
                &format!(r#"{{"turn":1,"output":{output}}}"#)
            ),
            (
                "bot.processing.completed",
                &format!(r#"{{"focus_candidates":[],"output":{output},"turn":1}}"#)
            ),
        ])
    );
    // A failed turn ends the turn too.
    run.event("bot.input", &input("fail", "x"));
    let records = run.until(WAIT, label_is("bot.processing.failed"));
    assert_eq!(
        only(&records, &["bot.processing.failed"]),
        pairs(&[(
            "bot.processing.failed",
            r#"{"message":"double failed","turn":2}"#
        )])
    );
    close(run, "a_turn_completes_with_the_cognitions_output");
}

#[test]
fn a_reply_for_another_turn_is_ignored() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("stale", "s"));
    let records = run.until(WAIT, label_is("floppy.output"));
    assert_eq!(
        only(&records, &["floppy.stale", "floppy.output"]),
        pairs(&[
            (
                "floppy.stale",
                "bot.ability.cognition.output for turn 101 (current 1)"
            ),
            (
                "floppy.output",
                r#"{"turn":1,"output":[{"data":{"text":"fresh 1"},"event":"bot.ability.communication.respond","target":"session.floppy.communication"}]}"#
            ),
        ])
    );
    close(run, "a_reply_for_another_turn_is_ignored");
}

#[test]
fn inputs_during_a_turn_are_held_and_perceived_together_by_the_next() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "1500")],
    );
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("hang", "first"));
    run.until(WAIT, label_is("double.input"));
    run.event("bot.input", &input("reply", "second"));
    run.event("bot.input", &input("reply", "third"));
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "floppy.output"
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("reply 2"))
    });
    assert_eq!(
        only(
            &records,
            &[
                "floppy.held",
                "bot.processing.timed_out",
                "bot.processing.failed",
                "double.input"
            ]
        ),
        // processing holds both (its count after each); the turn's end
        // starts turn 2 at once, which perceives them together, in
        // arrival order. The environment logs the events the body sends
        // it after the body's step.
        pairs(&[
            ("floppy.held", "1"),
            ("floppy.held", "2"),
            ("bot.processing.timed_out", r#"{"turn":1}"#),
            (
                "bot.processing.failed",
                r#"{"message":"Bot processing timed out after 1.5 seconds.","turn":1}"#
            ),
            ("double.input", r#"2 perceived ["second","third"]"#),
        ])
    );
    close(
        run,
        "inputs_during_a_turn_are_held_and_perceived_together_by_the_next",
    );
}

/// A `bot.input` whose device stamped `participants`.
fn input_from(action: &str, data: &str, participants: &[&str]) -> Json {
    let participants: Vec<Json> = participants
        .iter()
        .map(|reference| json!({"ref": reference, "role": "source"}))
        .collect();
    json!({"observation": {"event": action, "data": data}, "participants": participants})
}

#[test]
fn a_backlog_splits_into_turns_of_at_most_eight_percepts() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "1500")],
    );
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("hang", "first"));
    run.until(WAIT, label_is("double.participants"));
    // Ten percepts while turn 1 hangs: turn 2 takes the first eight
    // (MAX_PERCEPTS), turn 3 the other two; the participants of a turn
    // are its percepts' union, in order, once each.
    for n in 1..=10 {
        let person = format!("person-{}", n % 3);
        run.event(
            "bot.input",
            &input_from("reply", &format!("p{n}"), &[&person, "caller"]),
        );
    }
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "floppy.output"
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("reply 3"))
    });
    assert_eq!(
        only(&records, &["double.input", "double.participants"]),
        pairs(&[
            (
                "double.input",
                r#"2 perceived ["p1","p2","p3","p4","p5","p6","p7","p8"]"#
            ),
            (
                "double.participants",
                r#"[{"ref":"person-1","role":"source"},{"ref":"caller","role":"source"},{"ref":"person-2","role":"source"},{"ref":"person-0","role":"source"}]"#
            ),
            ("double.input", r#"3 perceived ["p9","p10"]"#),
            (
                "double.participants",
                r#"[{"ref":"person-0","role":"source"},{"ref":"caller","role":"source"},{"ref":"person-1","role":"source"}]"#
            ),
        ])
    );
    // Turn 2's environment counts the two percepts still held.
    let environments: Vec<Json> = logs(&records)
        .into_iter()
        .filter(|(label, _)| label == "double.environment")
        .map(|(_, text)| serde_json::from_str(&text).expect("JSON"))
        .collect();
    let held: Vec<&Json> = environments
        .iter()
        .map(|environment| &environment["self"]["held_percepts"])
        .collect();
    assert_eq!(held, [&json!(2), &json!(0)]);
    close(run, "a_backlog_splits_into_turns_of_at_most_eight_percepts");
}

#[test]
fn a_turn_takes_at_most_thirty_two_participants() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "1500")],
    );
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("hang", "first"));
    run.until(WAIT, label_is("double.participants"));
    // 15, 15 and 15 distinct participants: the first two percepts' union
    // is 30, and the third would make it 45, so it waits for turn 3.
    for n in 0..3 {
        let people: Vec<String> = (0..15).map(|k| format!("p{n}-{k}")).collect();
        let people: Vec<&str> = people.iter().map(String::as_str).collect();
        run.event(
            "bot.input",
            &input_from("reply", &format!("batch {n}"), &people),
        );
    }
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "floppy.output"
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("reply 3"))
    });
    assert_eq!(
        only(&records, &["double.input"]),
        pairs(&[
            ("double.input", r#"2 perceived ["batch 0","batch 1"]"#),
            ("double.input", r#"3 reply "batch 2""#),
        ])
    );
    let participants: Vec<usize> = logs(&records)
        .into_iter()
        .filter(|(label, _)| label == "double.participants")
        .map(|(_, text)| {
            serde_json::from_str::<Vec<Json>>(&text)
                .expect("JSON")
                .len()
        })
        .collect();
    assert_eq!(participants, [30, 15]);
    close(run, "a_turn_takes_at_most_thirty_two_participants");
}

#[test]
fn a_turn_whose_input_is_over_the_event_bound_fails() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "1500")],
    );
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("hang", "first"));
    run.until(WAIT, label_is("double.participants"));
    // Two percepts of 32000 bytes fit a turn's 65536, but not with the
    // turn's choices and environment in one 64 KiB event.
    let big = "x".repeat(32000);
    run.event("bot.input", &input("reply", &big));
    run.event("bot.input", &input("reply", &big));
    let records = run.until(WAIT, |record| {
        label_is("bot.processing.failed")(record)
            && record["text"]
                .as_str()
                .is_some_and(|text| text.contains(r#""turn":2"#))
    });
    let failed: Vec<Json> = only(&records, &["bot.processing.failed"])
        .into_iter()
        .map(|(_, text)| serde_json::from_str(&text).expect("JSON"))
        .collect();
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(
        failed[1]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("the turn's input could not be sent: ")),
        "{failed:?}"
    );
    // The bot is not stuck: it takes the next turn.
    run.event("bot.input", &input("reply", "next"));
    run.until(WAIT, label_is("floppy.output"));
    close(run, "a_turn_whose_input_is_over_the_event_bound_fails");
}

#[test]
fn what_is_held_when_the_bot_leaves_active_is_released() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("hang", "first"));
    run.until(WAIT, label_is("double.input"));
    run.event("bot.input", &input("reply", "second"));
    run.event("bot.input", &input("reply", "third"));
    run.until(WAIT, |record| {
        label_is("floppy.held")(record) && record["text"] == "2"
    });
    run.event("bot.deactivate", &json!({}));
    let records = run.until(WAIT, label_is("done.invoke"));
    assert_eq!(
        only(
            &records,
            &["floppy.released", "bot.deactivated", "double.input"]
        ),
        pairs(&[
            ("floppy.released", r#"{"count":2,"reason":"deactivated"}"#),
            ("bot.deactivated", "{}"),
        ]),
        "held percepts are released, never replayed"
    );
    close(run, "what_is_held_when_the_bot_leaves_active_is_released");
}

/// Puts `tests/fixtures/phase4/firmware_double.scx` beside the laid-out
/// body, its `mode`'s expression replaced, for the body's `firmware_src`
/// `'firmware_double.scx'`.
fn with_firmware(body: &Path, mode: &str) {
    let double =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase4/firmware_double.scx"))
            .expect("the firmware double is readable")
            .replace(
                r#"<data id="mode" expr="'ready'"/>"#,
                &format!(r#"<data id="mode" expr="{mode}"/>"#),
            );
    let dir = body.parent().expect("the body has a directory");
    std::fs::write(dir.join("firmware_double.scx"), double).expect("writable");
}

#[test]
fn the_firmware_is_offered_shown_and_reports_like_an_ability() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("firmware_src", "'firmware_double.scx'")],
    );
    with_firmware(&body, "'ready'");
    let mut run = start(&body);
    activated(&mut run);
    run.event(
        "bot.input",
        &json!({"observation": {"event": "act", "data": {"event": "timer.set", "data": {"minutes": 3}}}}),
    );
    let mut records = run.until(WAIT, |record| {
        label_is("double.input")(record)
            && record["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("2 "))
    });
    // Turn 2 logs its environment after its input.
    records.extend(run.until(WAIT, label_is("double.participants")));
    assert_eq!(
        only(&records, &["firmware.set", "double.input"]),
        pairs(&[
            (
                "double.input",
                r#"1 act {"data":{"minutes":3},"event":"timer.set"}"#
            ),
            ("firmware.set", "3"),
            // The firmware's report takes the next turn.
            ("double.input", r#"2 reply "timer 3""#),
        ])
    );
    // Its command is offered under its session id, which the selection
    // reached.
    let choices: Vec<Json> = only(&records, &["double.choices"])
        .into_iter()
        .map(|(_, text)| serde_json::from_str(&text).expect("JSON"))
        .collect();
    let set: Vec<&Json> = choices[0]
        .as_array()
        .expect("an array")
        .iter()
        .filter(|choice| choice["event"] == "timer.set")
        .collect();
    assert_eq!(set.len(), 1, "{choices:?}");
    assert_eq!(set[0]["target"], "session.floppy.firmware");
    // Each turn reads it as a device: idle with 0 minutes, then running
    // with 3.
    let firmware: Vec<(Json, Json, Json, Json)> = only(&records, &["double.environment"])
        .into_iter()
        .map(|(_, text)| {
            let environment: Json = serde_json::from_str(&text).expect("JSON");
            let firmware = &environment["self"]["firmware"];
            (
                firmware["ref"].clone(),
                firmware["id"].clone(),
                firmware["state"].clone(),
                firmware["attributes"]["minutes"].clone(),
            )
        })
        .collect();
    assert_eq!(
        firmware,
        [
            (
                json!("firmware"),
                json!("session.floppy.firmware"),
                json!("idle"),
                json!(0)
            ),
            (
                json!("firmware"),
                json!("session.floppy.firmware"),
                json!("running"),
                json!(3)
            ),
        ]
    );
    close(
        run,
        "the_firmware_is_offered_shown_and_reports_like_an_ability",
    );
}

#[test]
fn a_firmware_that_never_offers_fails_the_activation() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[
            ("firmware_src", "'firmware_double.scx'"),
            ("firmware_timeout_ms", "2000"),
        ],
    );
    with_firmware(&body, "'silent'");
    let run = start(&body);
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    assert_eq!(
        only(&records, &["floppy.diagnostic"]),
        pairs(&[(
            "floppy.diagnostic",
            "the firmware did not start within 2000 ms"
        )])
    );
    assert!(only(&records, &["bot.activated"]).is_empty(), "{records:?}");
    run.until(WAIT, label_is("done.invoke"));
    close(run, "a_firmware_that_never_offers_fails_the_activation");
}

#[test]
fn without_firmware_src_the_body_shows_no_firmware() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("reply", "hello"));
    let records = run.until(WAIT, label_is("floppy.output"));
    let environment: Json =
        serde_json::from_str(&only(&records, &["double.environment"])[0].1).expect("JSON");
    assert_eq!(environment["self"].get("firmware"), None, "{environment}");
    let choices = &only(&records, &["double.choices"])[0].1;
    assert!(!choices.contains(".firmware"), "{choices}");
    close(run, "without_firmware_src_the_body_shows_no_firmware");
}

#[test]
fn what_is_held_when_the_bot_degrades_is_released() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "1500")],
    );
    let mut run = start(&body);
    activated(&mut run);
    // A cancel the cognition never acknowledges degrades the bot.
    run.event("bot.input", &input("deaf", "first"));
    run.until(WAIT, label_is("double.participants"));
    run.event("bot.input", &input("reply", "second"));
    run.event("bot.input", &input("reply", "third"));
    run.until(WAIT, |record| {
        label_is("floppy.held")(record) && record["text"] == "2"
    });
    let records = run.until(WAIT, label_is("floppy.released"));
    assert_eq!(
        only(&records, &["floppy.released"]),
        pairs(&[("floppy.released", r#"{"count":2,"reason":"degraded"}"#)])
    );
    run.until(WAIT, label_is("done.invoke"));
    close(run, "what_is_held_when_the_bot_degrades_is_released");
}

#[test]
fn a_turn_that_times_out_is_cancelled_and_fails() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "300")],
    );
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("hang", "slow"));
    let records = run.until(WAIT, label_is("bot.processing.failed"));
    assert_eq!(
        only(
            &records,
            &["bot.processing.timed_out", "bot.processing.failed"]
        ),
        pairs(&[
            ("bot.processing.timed_out", r#"{"turn":1}"#),
            (
                "bot.processing.failed",
                r#"{"message":"Bot processing timed out after 0.3 seconds.","turn":1}"#
            ),
        ])
    );
    // The bot is back in attention and takes the next turn.
    run.event("bot.input", &input("reply", "next"));
    run.until(WAIT, label_is("floppy.output"));
    close(run, "a_turn_that_times_out_is_cancelled_and_fails");
}

#[test]
fn an_unacknowledged_cancel_degrades_the_bot() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "300")],
    );
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("deaf", "x"));
    run.until(WAIT, label_is("done.invoke"));
    let finished = close(run, "an_unacknowledged_cancel_degrades_the_bot");
    let records = &finished.all;
    assert_eq!(
        only(
            records,
            &[
                "bot.processing.timed_out",
                "bot.processing.cancel.timed_out",
                "bot.processing.failed",
                "bot.degraded",
                "floppy.degraded"
            ]
        ),
        // The environment logs the events the body sends it after the
        // body's step, which enters degraded.
        pairs(&[
            ("bot.processing.timed_out", r#"{"turn":1}"#),
            (
                "floppy.degraded",
                "turn 1: the cognition did not acknowledge its cancel"
            ),
            ("bot.processing.cancel.timed_out", r#"{"turn":1}"#),
            (
                "bot.processing.failed",
                r#"{"message":"Bot processing timed out after 0.3 seconds.","turn":1}"#
            ),
            // The transition into degraded announces it (the original's
            // a2cbaa8).
            ("bot.degraded", r#"{"stage":"processing_cancel"}"#),
        ])
    );
    // degraded is a final sink: the bot's session ends with its reason,
    // and the bot leaves its environment.
    assert_eq!(
        last_json(records, "done.invoke"),
        json!({"invokeid": "floppy", "data": {"reason": "turn 1: the cognition did not acknowledge its cancel"}}),
        "{records:?}"
    );
}

#[test]
fn without_a_backend_the_real_cognitions_turns_fail_unconfigured() {
    let body = layout(&Cognition::Real, &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("text", "hello"));
    let mut records = run.until(WAIT, label_is("bot.processing.failed"));
    records.extend(run.until(WAIT, label_is("bot.processing.failed")));
    assert_eq!(
        only(&records, &["bot.processing.failed"]),
        pairs(&[
            (
                "bot.processing.failed",
                r#"{"message":"the reasoning failed (unconfigured)","turn":1}"#
            ),
            (
                "bot.processing.failed",
                r#"{"message":"the reasoning failed (unconfigured)","turn":2}"#
            )
        ])
    );
    close(
        run,
        "without_a_backend_the_real_cognitions_turns_fail_unconfigured",
    );
}

/// An offered schema as the body compacts it: `$schema`, `title` and
/// `examples` dropped everywhere but in property names.
fn compacted(schema: &Json) -> Json {
    match schema {
        Json::Array(items) => Json::Array(items.iter().map(compacted).collect()),
        Json::Object(members) => Json::Object(
            members
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "$schema" | "title" | "examples"))
                .map(|(key, value)| {
                    let value = match (key.as_str(), value) {
                        ("properties", Json::Object(named)) => Json::Object(
                            named
                                .iter()
                                .map(|(name, schema)| (name.clone(), compacted(schema)))
                                .collect(),
                        ),
                        ("enum" | "const" | "required" | "default", value) => value.clone(),
                        (_, value) => compacted(value),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        value => value.clone(),
    }
}

/// The body's `bot.focus_device` choice for a turn whose focus candidates
/// are `candidates`.
fn focus_choice(candidates: &[&str]) -> Json {
    let schema: Json = serde_json::from_str(
        &std::fs::read_to_string(workspace().join("bots/floppy/schemas/bot.focus_device.json"))
            .expect("the schema is readable"),
    )
    .expect("the schema is JSON");
    let description = schema["description"]
        .as_str()
        .and_then(|text| text.lines().next())
        .expect("a description")
        .to_owned();
    let mut schema = compacted(&schema);
    schema["properties"]["device"]["enum"] = json!(candidates);
    json!({"event": "bot.focus_device", "schema": schema, "description": description, "target": BODY})
}

/// The body's session id, which tags its own choices.
const BODY: &str = "session.floppy";

/// The offer of `event` (its schema file) by the ability whose session is
/// `target`, compacted and tagged with it.
fn offered_choice(target: &str, event: &str) -> Json {
    let schema: Json = serde_json::from_str(
        &std::fs::read_to_string(
            workspace()
                .join("abilities/schemas")
                .join(format!("{event}.json")),
        )
        .expect("the schema is readable"),
    )
    .expect("the schema is JSON");
    let description = schema["description"]
        .as_str()
        .and_then(|text| text.lines().next())
        .expect("a description")
        .to_owned();
    json!({"event": event, "schema": compacted(&schema), "description": description, "target": target})
}

/// The communication's offers, then the identity's.
fn ability_choices() -> [Json; 2] {
    [
        offered_choice(
            "session.floppy.communication",
            "bot.ability.communication.respond",
        ),
        offered_choice("session.floppy.identity", "bot.ability.identity.adopt"),
    ]
}

/// The double's offer, compacted and tagged with its session's id.
fn ignore_choice() -> Json {
    json!({
        "event": "bot.ability.cognition.ignore",
        "description": "Deliberately ignore this stimulus.",
        "schema": {
            "description": "Deliberately ignore this stimulus.",
            "type": "object",
            "properties": {},
            "additionalProperties": false
        },
        "target": "session.floppy.cognition"
    })
}

fn device_input(action: &str, data: &str, device: &str) -> Json {
    json!({"observation": {"event": action, "data": data}, "target_device": device})
}

fn last_json(records: &[Json], label: &str) -> Json {
    let text = logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .next_back()
        .unwrap_or_else(|| panic!("a {label} log: {records:?}"));
    serde_json::from_str(&text).expect("the log text is JSON")
}

#[test]
fn the_choices_are_the_bodys_and_the_offers_and_focus_is_grounded() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    // The bot holds the lamp and the fan, which the room does not run:
    // each focused turn's read of the lamp fails, so it offers no device
    // choice.
    let layout_root = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the layout's root");
    let room = support::environment_with_devices(
        layout_root,
        &["floppy"],
        &[],
        &[("floppy", &["lamp", "fan"])],
        &[],
    );
    let mut run = Run::start(&[room.display().to_string()], &[]);
    activated(&mut run);
    // Unfocused, the input names the lamp: the body turns to it (the
    // original's reflex floor), its only candidate. The body offers
    // bot.focus_device pinned to it, then the double's offer, compacted
    // and tagged.
    run.event("bot.input", &device_input("focus", "lamp", "lamp"));
    let mut records = run.until(WAIT, label_is("floppy.focus"));
    // The environment logs the turn's outcome after the body's step.
    records.extend(run.until(WAIT, label_is("bot.processing.completed")));
    let [respond, adopt] = ability_choices();
    let choices = json!([focus_choice(&["lamp"]), ignore_choice(), respond, adopt]);
    assert_eq!(last_json(&records, "double.choices"), choices);
    assert_eq!(last_json(&records, "floppy.choices"), choices);
    // The reflex, then the double's selection.
    assert_eq!(
        only(&records, &["floppy.focus"]),
        pairs(&[("floppy.focus", "lamp"), ("floppy.focus", "lamp")])
    );
    // Focused on the lamp, an input for the lamp again: the same choices
    // (nothing clears the focus). A focus on the fan, outside the turn's candidates (which
    // the generate service's pinned enum would refuse), is not taken: the
    // body's guard is the original's _ability_selected_configured_focus_device.
    run.event("bot.input", &device_input("focus", "fan", "lamp"));
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(
        last_json(&records, "double.choices"),
        json!([focus_choice(&["lamp"]), ignore_choice(), respond, adopt])
    );
    assert_eq!(
        only(&records, &["bot.processing.failed", "floppy.focus"]),
        pairs(&[])
    );
    // Still focused on the lamp: the next turn's candidates include it.
    run.event("bot.input", &device_input("focus", "fan", "fan"));
    let records = run.until(WAIT, label_is("floppy.focus"));
    assert_eq!(
        last_json(&records, "double.choices")[0],
        focus_choice(&["lamp", "fan"])
    );
    assert_eq!(
        only(&records, &["floppy.focus"]),
        pairs(&[("floppy.focus", "fan")])
    );
    close(
        run,
        "the_choices_are_the_bodys_and_the_offers_and_focus_is_grounded",
    );
}

#[test]
fn an_ignore_selection_goes_to_the_cognition_and_ends_the_turn() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    // The reference without_ignore_selections ("cognition only"): the
    // ignore goes to the cognition that offers it, and the turn completes
    // with it as its output.
    run.event("bot.input", &input("ignore", "noise"));
    // The double logs the ignore it sent itself and the environment the
    // outcome, in either order.
    let mut records = run.until(WAIT, label_is("bot.processing.completed"));
    if only(&records, &["double.ignore"]).is_empty() {
        records.extend(run.until(WAIT, label_is("double.ignore")));
    }
    assert_eq!(
        only(
            &records,
            &["bot.processing.completed", "bot.processing.failed"]
        ),
        pairs(&[(
            "bot.processing.completed",
            r#"{"focus_candidates":[],"output":[{"data":{},"event":"bot.ability.cognition.ignore","reason":"nothing to do","target":"session.floppy.cognition"}],"turn":1}"#
        )])
    );
    assert_eq!(
        only(&records, &["double.ignore"]),
        pairs(&[("double.ignore", "{}")])
    );
    // The bot is back in attention and takes the next turn.
    run.event("bot.input", &input("reply", "next"));
    let records = run.until(WAIT, label_is("double.input"));
    assert_eq!(
        only(&records, &["double.input", "double.ignore"]),
        pairs(&[("double.input", r#"2 reply "next""#)])
    );
    close(
        run,
        "an_ignore_selection_goes_to_the_cognition_and_ends_the_turn",
    );
}

#[test]
fn choices_over_the_budget_fail_the_turn_before_the_cognition() {
    let body = layout(&Cognition::Double("'huge'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("reply", "x"));
    let records = run.until(WAIT, label_is("bot.processing.failed"));
    let failed = last_json(&records, "bot.processing.failed");
    assert_eq!(failed["turn"], 1, "{failed}");
    let message = failed["message"].as_str().expect("a message");
    assert!(
        message.starts_with("the turn's choices are ") && message.ends_with(" bytes, over 32768"),
        "{message}"
    );
    assert!(only(&records, &["double.input"]).is_empty(), "{records:?}");
    close(
        run,
        "choices_over_the_budget_fail_the_turn_before_the_cognition",
    );
}

#[test]
fn environment_stimuli_fan_out_to_the_input_abilities_and_not_the_cognition() {
    let body = layout(&Cognition::Double("'ready'"), &[("input", "['identity']")]);
    // The input double stands in for the identity, the one input ability.
    let abilities = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the layout's root")
        .join("abilities");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/input_double.scx"),
        abilities.join("identity/identity.scx"),
    )
    .expect("the input double is copyable");
    let mut run = start(&body);
    activated(&mut run);
    let sound = json!({"audio": "media/audio-1", "kind": "knock"});
    let visual = json!({"image": "media/image-1", "media_type": "image/png"});
    run.event("environment.sound", &sound);
    run.event("environment.visual", &visual);
    run.until(WAIT, label_is("input.visual"));
    // The stimuli start no turn: the next turn is the first.
    run.event("bot.input", &input("reply", "after"));
    run.until(WAIT, label_is("bot.processing.completed"));
    // The reply is played out into the environment, where the bot hears
    // it too.
    run.until(WAIT, label_is("bot.ability.speaking.output"));
    let finished = close(
        run,
        "environment_stimuli_fan_out_to_the_input_abilities_and_not_the_cognition",
    );
    assert_eq!(
        only(
            &finished.all,
            &["input.sound", "input.visual", "double.input"]
        ),
        pairs(&[
            ("input.sound", &sound.to_string()),
            ("input.visual", &visual.to_string()),
            ("double.input", r#"1 reply "after""#),
            (
                "input.sound",
                r#"{"audio":"media/speaking-1.pcm","channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"source":"speaking"}"#,
            ),
        ])
    );
}

#[test]
fn the_listenings_products_are_turn_stimuli() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    // The real speech service detects voice and decodes against a fake
    // backend on 127.0.0.1 that reads the words spelled into the audio.
    let backend = support::hearing::Backend::start();
    let mut args = support::hearing::granted(
        "the_listenings_products_are_turn_stimuli",
        &backend,
        &[
            (
                "reply-to-me.wav",
                support::hearing::spoken_wav("reply to me"),
            ),
            ("silence.wav", support::hearing::silent_wav()),
            ("ring.wav", support::hearing::silent_wav()),
        ],
    )
    .args;
    args.push(support::environment_for(&body, &[]));
    let mut run = Run::start(&args, &[]);
    activated(&mut run);
    // Decoded speech: a turn whose stimulus is
    // bot.ability.listening.speech.output.
    let speech = json!({"audio": "media/reply-to-me.wav", "media_type": "audio/wav"});
    run.event("environment.sound", &speech);
    let silence = json!({"audio": "media/silence.wav", "media_type": "audio/wav"});
    run.event("environment.sound", &silence);
    run.until(WAIT, label_is("bot.processing.completed"));
    // Labelled non-speech: a turn whose stimulus is the environment.sound.
    let ring = json!({"audio": "media/ring.wav", "kind": "phone.ringing"});
    run.event("environment.sound", &ring);
    run.until(WAIT, label_is("bot.processing.completed"));
    let finished = close(run, "the_listenings_products_are_turn_stimuli");
    assert_eq!(
        only(
            &finished.all,
            &["double.input", "bot.ability.speaking.output"]
        ),
        pairs(&[
            ("double.input", r#"1 reply "reply to me""#),
            (
                "bot.ability.speaking.output",
                r#"{"channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"text":"reply 1"}"#
            ),
            ("double.input", r#"2 ignore "phone.ringing""#),
        ])
    );
    assert_eq!(
        backend.decoded(),
        vec!["reply to me"],
        "only the speech is decoded"
    );
}

#[test]
fn a_selected_communication_input_reaches_the_communication() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let backend = support::hearing::Backend::start();
    let mut args = support::hearing::granted(
        "a_selected_communication_input_reaches_the_communication",
        &backend,
        &[
            ("admit.wav", support::hearing::spoken_wav("admit reply now")),
            ("silence.wav", support::hearing::silent_wav()),
        ],
    )
    .args;
    args.push(support::environment_for(&body, &[]));
    let mut run = Run::start(&args, &[]);
    activated(&mut run);
    // Heard speech whose turn selects bot.ability.communication.input (as
    // the original's SpeechHeard seed does), naming no target: the body
    // sends it to itself and relays it to the communication, whose
    // conversation product is the next turn's stimulus.
    let speech = json!({"audio": "media/admit.wav", "media_type": "audio/wav"});
    run.event("environment.sound", &speech);
    let silence = json!({"audio": "media/silence.wav", "media_type": "audio/wav"});
    run.event("environment.sound", &silence);
    run.until(WAIT, label_is("bot.ability.speaking.output"));
    let finished = close(
        run,
        "a_selected_communication_input_reaches_the_communication",
    );
    assert_eq!(
        only(
            &finished.all,
            &["double.input", "bot.ability.speaking.output"]
        ),
        pairs(&[
            ("double.input", r#"1 admit "admit reply now""#),
            ("double.input", "2 reply 1"),
            (
                "bot.ability.speaking.output",
                r#"{"channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"text":"reply 2"}"#
            ),
        ])
    );
}
