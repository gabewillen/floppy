//! The floppy bot's body (`bots/floppy/floppy.scx`): mosfet's Bot lifecycle
//! and processing turns, run through `scx run` on the wall clock.
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
//! The body writes each event it emits as a `log` record labelled with the
//! event's name; the double logs `double.input` and `double.exit`.
//!
//! The double offers `bot.ability.cognition.ignore` and the communication
//! `bot.ability.communication.respond`; the routing tests
//! check the turn's assembled choices (`double.choices`) and where the
//! body routes each selection (`bot.route`, `bot.route.dropped`,
//! `bot.focus`, `double.ignore`). `tests/tests/communication.rs` covers
//! what the communication does with what it is routed.
//!
//! One test sets `input` (mosfet's Bot input) to `['identity']` and puts
//! `tests/fixtures/phase4/input_double.scx` in the identity's place; the
//! double logs `input.sound` and `input.visual` for each `environment.*`
//! stimulus the body fans out to it.
//!
//! One test runs the real cognition (`abilities/cognition/cognition.scx` and its
//! stages) with no backend: the intuition's decision fails fast with
//! `unconfigured`, and so does the turn.
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
    let abilities = dir.join("abilities");
    for name in [
        "communication",
        "conversation",
        "speaking",
        "identity",
        "memory",
    ] {
        support::copy_ability(name, &abilities);
    }
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

fn start(body: &Path) -> Run {
    Run::start(&[body.display().to_string()], &[])
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

fn activated(run: &mut Run) -> Vec<Json> {
    run.event("bot.activate", &json!({}));
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
    // Nothing happens before bot.activate: the body starts inactive.
    run.event("bot.input", &input("reply", "early"));
    let records = activated(&mut run);
    assert!(
        only(&records, &["double.input", "bot.output"]).is_empty(),
        "an inactive bot takes no turn: {records:?}"
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
    let mut run = start(&body);
    run.event("bot.activate", &json!({}));
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    let diagnostic = only(&records, &["bot.diagnostic"]);
    assert_eq!(diagnostic.len(), 1, "{records:?}");
    assert!(
        diagnostic[0].1.starts_with("error."),
        "the invoke's start error fails the activation: {diagnostic:?}"
    );
    assert!(only(&records, &["bot.activated"]).is_empty(), "{records:?}");
    // Back in inactive: a second bot.activate is taken again.
    run.event("bot.activate", &json!({}));
    run.until(WAIT, label_is("bot.activating.failed"));
    close(run, "a_missing_cognition_fails_the_activation");
}

#[test]
fn an_ability_that_never_offers_times_the_activation_out() {
    let body = layout(
        &Cognition::Double("'silent'"),
        &[("activation_timeout_ms", "3000")],
    );
    let mut run = start(&body);
    run.event("bot.activate", &json!({}));
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    assert_eq!(
        only(&records, &["bot.diagnostic"]),
        pairs(&[(
            "bot.diagnostic",
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
    let records = run.until(WAIT, label_is("bot.deactivated"));
    assert_eq!(
        only(&records, &["double.exit", "bot.deactivated"]),
        pairs(&[
            ("double.exit", "Qwen/Qwen3.8-Flash-Next"),
            ("bot.deactivated", "{}")
        ]),
        "the child's exit handler runs before the body reports"
    );
    // Inactive again: a turn is not taken until the next activation.
    run.event("bot.input", &input("reply", "ignored"));
    activated(&mut run);
    run.event("bot.input", &input("reply", "taken"));
    let records = run.until(WAIT, label_is("bot.output"));
    assert_eq!(
        only(&records, &["double.input"]),
        pairs(&[("double.input", r#"1 reply "taken""#)])
    );
    close(run, "deactivate_cancels_the_abilities");
}

#[test]
fn the_activation_passes_its_model_to_the_cognition() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    run.event("bot.activate", &json!({"model": "some/model"}));
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
fn a_reboot_is_taken_only_from_the_cognition() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    // From outside: rejected, nothing restarts.
    run.event(
        "bot.reboot",
        &json!({"reason": "cognition_child_teardown_failed"}),
    );
    let records = run.until(WAIT, label_is("bot.rejected"));
    assert!(only(&records, &["double.exit"]).is_empty(), "{records:?}");
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
    let records = run.until(WAIT, label_is("bot.output"));
    assert_eq!(
        only(&records, &["double.input"]),
        pairs(&[("double.input", r#"2 reply "again""#)])
    );
    close(run, "a_reboot_is_taken_only_from_the_cognition");
}

#[test]
fn a_turn_completes_with_the_cognitions_output() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("reply", "hello"));
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    let output = r#"[{"data":{"text":"reply 1"},"event":"bot.ability.communication.respond"}]"#;
    assert_eq!(
        only(
            &records,
            &["double.input", "bot.output", "bot.processing.completed"]
        ),
        pairs(&[
            ("double.input", r#"1 reply "hello""#),
            ("bot.output", &format!(r#"{{"turn":1,"output":{output}}}"#)),
            (
                "bot.processing.completed",
                &format!(r#"{{"output":{output},"focus_candidates":[],"turn":1}}"#)
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
    let records = run.until(WAIT, label_is("bot.output"));
    assert_eq!(
        only(&records, &["bot.stale", "bot.output"]),
        pairs(&[
            (
                "bot.stale",
                "bot.ability.cognition.output for turn 101 (current 1)"
            ),
            (
                "bot.output",
                r#"{"turn":1,"output":[{"data":{"text":"fresh 1"},"event":"bot.ability.communication.respond"}]}"#
            ),
        ])
    );
    close(run, "a_reply_for_another_turn_is_ignored");
}

#[test]
fn inputs_during_a_turn_wait_and_run_in_order() {
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
            && record["label"] == "bot.output"
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("reply 3"))
    });
    assert_eq!(
        only(
            &records,
            &[
                "bot.deferred",
                "bot.processing.timed_out",
                "bot.processing.failed",
                "double.input"
            ]
        ),
        pairs(&[
            ("bot.deferred", "1"),
            ("bot.deferred", "2"),
            ("bot.processing.timed_out", r#"{"turn":1}"#),
            (
                "bot.processing.failed",
                r#"{"message":"the turn timed out after 1500 ms and was cancelled","turn":1}"#
            ),
            ("double.input", r#"2 reply "second""#),
            ("double.input", r#"3 reply "third""#),
        ])
    );
    close(run, "inputs_during_a_turn_wait_and_run_in_order");
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
                r#"{"message":"the turn timed out after 300 ms and was cancelled","turn":1}"#
            ),
        ])
    );
    // The bot is back in attention and takes the next turn.
    run.event("bot.input", &input("reply", "next"));
    run.until(WAIT, label_is("bot.output"));
    close(run, "a_turn_that_times_out_is_cancelled_and_fails");
}

#[test]
fn an_unacknowledged_cancel_degrades_the_bot() {
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("processing_timeout_ms", "300"), ("cancel_timeout_ms", "1")],
    );
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("deaf", "x"));
    let finished = run.finish(WAIT);
    trace::assert_trace(
        "an_unacknowledged_cancel_degrades_the_bot",
        &finished.all,
        &[],
    );
    let records = &finished.records;
    assert_eq!(
        only(
            records,
            &[
                "bot.processing.timed_out",
                "bot.processing.cancel.timed_out",
                "bot.processing.failed",
                "bot.degraded"
            ]
        ),
        pairs(&[
            ("bot.processing.timed_out", r#"{"turn":1}"#),
            ("bot.processing.cancel.timed_out", r#"{"turn":1}"#),
            (
                "bot.degraded",
                "turn 1: the cognition did not acknowledge its cancel"
            ),
        ])
    );
    // degraded is a final sink: the session ends with its reason.
    assert!(support::finished(records), "{records:?}");
    let done: Vec<&Json> = support::of_type(records, "done")
        .into_iter()
        .filter(|record| record.get("parent").is_none())
        .collect();
    assert_eq!(
        done.last().map(|record| &record["data"]),
        Some(&json!({"reason": "turn 1: the cognition did not acknowledge its cancel"})),
        "{records:?}"
    );
}

#[test]
fn without_a_decision_backend_the_real_cognitions_turn_fails_unconfigured() {
    let body = layout(&Cognition::Real, &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("text", "hello"));
    let records = run.until(WAIT, label_is("bot.processing.failed"));
    assert_eq!(
        only(&records, &["bot.processing.failed"]),
        pairs(&[(
            "bot.processing.failed",
            r#"{"message":"the intuition failed: the decision failed (unconfigured)","turn":1}"#
        )])
    );
    close(
        run,
        "without_a_decision_backend_the_real_cognitions_turn_fails_unconfigured",
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
    json!({"event": "bot.focus_device", "schema": schema, "description": description})
}

fn clear_choice() -> Json {
    let schema: Json = serde_json::from_str(
        &std::fs::read_to_string(workspace().join("bots/floppy/schemas/bot.clear_focus.json"))
            .expect("the schema is readable"),
    )
    .expect("the schema is JSON");
    let description = schema["description"]
        .as_str()
        .and_then(|text| text.lines().next())
        .expect("a description")
        .to_owned();
    json!({"event": "bot.clear_focus", "schema": compacted(&schema), "description": description})
}

/// The offer of `event` (its schema file) by the ability `target`,
/// compacted and tagged with its target.
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
        offered_choice("communication", "bot.ability.communication.respond"),
        offered_choice("identity", "bot.ability.identity.adopt"),
    ]
}

/// The double's offer, compacted and tagged with its target.
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
        "target": "cognition"
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
    let body = layout(
        &Cognition::Double("'ready'"),
        &[("devices", "['lamp', 'fan']")],
    );
    let mut run = start(&body);
    activated(&mut run);
    // Unfocused, the input names the lamp: the only candidate. The body
    // offers bot.focus_device pinned to it (no clear_focus: nothing is
    // focused), then the double's offer, compacted and tagged.
    run.event("bot.input", &device_input("focus", "lamp", "lamp"));
    let records = run.until(WAIT, label_is("bot.focus"));
    let [respond, adopt] = ability_choices();
    assert_eq!(
        last_json(&records, "double.choices"),
        json!([focus_choice(&["lamp"]), ignore_choice(), respond, adopt])
    );
    assert_eq!(
        only(&records, &["bot.choices", "bot.route", "bot.focus"]),
        pairs(&[
            (
                "bot.choices",
                "bot.focus_device bot.ability.cognition.ignore@cognition bot.ability.communication.respond@communication bot.ability.identity.adopt@identity"
            ),
            ("bot.route", "bot.focus_device -> bot"),
            ("bot.focus", "lamp"),
        ])
    );
    // Focused on the lamp, an input for the lamp again: clear_focus is
    // offered now. A focus on the fan, outside the turn's candidates, is
    // routed to the body and taken by no transition.
    run.event("bot.input", &device_input("focus", "fan", "lamp"));
    let records = run.until(WAIT, label_is("bot.route.dropped"));
    assert_eq!(
        last_json(&records, "double.choices"),
        json!([
            focus_choice(&["lamp"]),
            clear_choice(),
            ignore_choice(),
            respond,
            adopt
        ])
    );
    assert_eq!(
        only(&records, &["bot.route", "bot.route.dropped", "bot.focus"]),
        pairs(&[
            ("bot.route", "bot.focus_device -> bot"),
            ("bot.route.dropped", "bot.focus_device was not taken"),
        ])
    );
    // Still focused on the lamp: the next turn's candidates include it.
    run.event("bot.input", &device_input("focus", "fan", "fan"));
    let records = run.until(WAIT, label_is("bot.focus"));
    assert_eq!(
        last_json(&records, "double.choices")[0],
        focus_choice(&["lamp", "fan"])
    );
    assert_eq!(
        only(&records, &["bot.focus"]),
        pairs(&[("bot.focus", "fan")])
    );
    close(
        run,
        "the_choices_are_the_bodys_and_the_offers_and_focus_is_grounded",
    );
}

#[test]
fn a_selection_the_turn_did_not_offer_is_dropped() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    // No ability offers bot.ability.reasoning.input, and no device is
    // configured, so the body offers no focus.
    run.event("bot.input", &input("unoffered", "hello"));
    let records = run.until(WAIT, label_is("bot.route.dropped"));
    let [respond, adopt] = ability_choices();
    assert_eq!(
        last_json(&records, "double.choices"),
        json!([ignore_choice(), respond, adopt])
    );
    assert_eq!(
        only(&records, &["bot.route.dropped"]),
        pairs(&[(
            "bot.route.dropped",
            "bot.ability.reasoning.input was not offered this turn"
        )])
    );
    // An offered event for an ability that does not offer it.
    run.event("bot.input", &input("elsewhere", "x"));
    let records = run.until(WAIT, label_is("bot.route.dropped"));
    assert_eq!(
        only(
            &records,
            &["bot.route", "bot.route.dropped", "double.ignore"]
        ),
        pairs(&[(
            "bot.route.dropped",
            "bot.ability.cognition.ignore is not offered by speaking"
        )])
    );
    close(run, "a_selection_the_turn_did_not_offer_is_dropped");
}

#[test]
fn an_ignore_selection_reaches_the_cognition_and_ends_the_turn() {
    let body = layout(&Cognition::Double("'ready'"), &[]);
    let mut run = start(&body);
    activated(&mut run);
    run.event("bot.input", &input("ignore", "noise"));
    let records = run.until(WAIT, label_is("double.ignore"));
    assert_eq!(
        only(
            &records,
            &[
                "bot.processing.completed",
                "bot.route",
                "bot.route.dropped",
                "double.ignore"
            ]
        ),
        pairs(&[
            (
                "bot.processing.completed",
                r#"{"output":[{"data":{},"event":"bot.ability.cognition.ignore","reason":"nothing to do","target":"cognition"}],"focus_candidates":[],"turn":1}"#
            ),
            ("bot.route", "bot.ability.cognition.ignore -> cognition"),
            ("double.ignore", "{}"),
        ])
    );
    // The bot is back in attention and takes the next turn.
    run.event("bot.input", &input("reply", "next"));
    let records = run.until(WAIT, label_is("double.input"));
    assert_eq!(
        only(&records, &["double.input"]),
        pairs(&[("double.input", r#"2 reply "next""#)])
    );
    close(
        run,
        "an_ignore_selection_reaches_the_cognition_and_ends_the_turn",
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
        ])
    );
}
