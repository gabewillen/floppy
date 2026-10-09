//! The floppy bot's body (`bots/floppy/floppy.scx`): mosfet's Bot lifecycle
//! and processing turns, run through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! and its schemas under `bots/floppy/`, the ability schemas under
//! `abilities/schemas/`) with `tests/fixtures/phase4/cognition_double.scx`
//! as `abilities/cognition.scx`, so the body's literal
//! `<invoke src="../../abilities/cognition.scx">` reaches the double. The
//! double's behavior per turn is the `bot.input` observation's event name
//! (`reply`, `fail`, `stale`, `reboot`, `hang`, `deaf`). Timer bounds are
//! shortened by rewriting the copy's `<data>` expressions.
//!
//! The body writes each event it emits as a `log` record labelled with the
//! event's name; the double logs `double.input` and `double.exit`.
//!
//! One test runs the real cognition (`abilities/cognition.scx` and its
//! stages) with no backend: the intuition's decision fails fast with
//! `unconfigured`, and so does the turn.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Finished, Run, logs, unique_scratch, workspace};

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(60);

/// The cognition the scratch layout gets.
enum Cognition {
    /// The double, with `mode`'s expression replaced (`'ready'` or
    /// `'silent'`).
    Double(&'static str),
    /// The repository's cognition.
    Real,
    /// No `abilities/cognition.scx` at all.
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
    let ability = dir.join("abilities/cognition.scx");
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
            std::fs::write(ability, double).expect("the double copy is writable");
        }
        Cognition::Real => {
            // The cognition and its stages.
            for stage in ["cognition", "autonomy", "intuition", "reasoning"] {
                let name = format!("{stage}.scx");
                std::fs::copy(
                    workspace().join("abilities").join(&name),
                    dir.join("abilities").join(&name),
                )
                .expect("the ability is copyable");
            }
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

fn close(run: Run) -> Finished {
    run.close(WAIT)
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
    let finished = close(run);
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
    close(run);
}

#[test]
fn an_ability_that_never_offers_times_the_activation_out() {
    let body = layout(
        &Cognition::Double("'silent'"),
        &[("activation_timeout_ms", "300")],
    );
    let mut run = start(&body);
    run.event("bot.activate", &json!({}));
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    assert_eq!(
        only(&records, &["bot.diagnostic"]),
        pairs(&[(
            "bot.diagnostic",
            r#"the abilities ["cognition"] were not ready within 300 ms"#
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
    close(run);
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
    close(run);
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
    close(run);
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
    close(run);
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
    close(run);
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
    close(run);
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
    close(run);
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
    close(run);
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
    close(run);
}
