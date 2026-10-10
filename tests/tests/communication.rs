//! The floppy bot's communication (`abilities/communication.scx` with its
//! children `abilities/conversation.scx` and `abilities/speaking.scx`),
//! run as the body's child through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! under `bots/floppy/`, the communication, conversation, speaking,
//! identity, memory and every schema under `abilities/`) with
//! `tests/fixtures/phase4/cognition_double.scx` as `abilities/cognition.scx`
//! (no model; its behavior per turn is the `bot.input` observation's name,
//! or a conversation product's last text). No backend is involved: the
//! communication path makes no service call.
//!
//! The speaking child logs `bot.say`, the conversation each commit
//! (`conversation`) and what memory recalled (`conversation.memories`),
//! the communication what it forwards (`communication.product`,
//! `communication.history`, `communication.deferred`,
//! `communication.activate`), and the body each
//! event it emits (`bot.ability.speaking.output`,
//! `bot.ability.communication.failed`).
//!
//! `the_conversation_commits_each_message_into_its_history` runs the
//! conversation alone under `tests/fixtures/phase4/conversation_harness.scx`,
//! which logs each `bot.ability.conversation.output` whole.
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

/// A speaking child that is ready but never answers.
const DEAF_SPEAKING: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="speaking" initial="idle">
  <state id="idle">
    <onentry>
      <send event="bot.ability.offers" target="#_parent">
        <param name="ability" expr="'speaking'"/>
        <param name="choices" expr="[]"/>
      </send>
    </onentry>
    <transition event="bot.ability.speaking.input">
      <log label="deaf.input" expr="_event.data.text"/>
    </transition>
  </state>
</scxml>
"##;

/// The speaking child the layout gets.
enum Speaking {
    /// The repository's text-mode speaking.
    Real,
    /// [`DEAF_SPEAKING`].
    Deaf,
    /// No `abilities/speaking.scx` at all.
    Missing,
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("the directory is creatable");
    for entry in std::fs::read_dir(from).expect("the directory is readable") {
        let path = entry.expect("a directory entry").path();
        if path.is_file() {
            std::fs::copy(&path, to.join(path.file_name().expect("a file name")))
                .expect("the file is copyable");
        }
    }
}

/// Replaces the `<data id>` expressions of the document at `path`.
fn rewrite(path: &Path, data: &[(&str, &str)]) {
    let mut text = std::fs::read_to_string(path).expect("the document is readable");
    for (id, expr) in data {
        let start = text
            .find(&format!(r#"<data id="{id}" expr=""#))
            .unwrap_or_else(|| panic!("{} declares <data id=\"{id}\">", path.display()));
        let end = start + text[start..].find("/>").expect("the <data> element closes");
        text.replace_range(start..end, &format!(r#"<data id="{id}" expr="{expr}""#));
    }
    std::fs::write(path, text).expect("the document copy is writable");
}

/// Lays out the bot with the cognition double, `speaking`, and `edits`
/// (`(path under the scratch root, [(id, expr)])`); returns the body.
fn layout(speaking: &Speaking, edits: &[(&str, &[(&str, &str)])]) -> PathBuf {
    let dir = unique_scratch("communication");
    copy_dir(&workspace().join("bots/floppy"), &dir.join("bots/floppy"));
    copy_dir(
        &workspace().join("bots/floppy/schemas"),
        &dir.join("bots/floppy/schemas"),
    );
    copy_dir(
        &workspace().join("abilities/schemas"),
        &dir.join("abilities/schemas"),
    );
    for name in [
        "communication",
        "conversation",
        "speaking",
        "identity",
        "memory",
    ] {
        let name = format!("{name}.scx");
        std::fs::copy(
            workspace().join("abilities").join(&name),
            dir.join("abilities").join(&name),
        )
        .expect("the ability is copyable");
    }
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/cognition_double.scx"),
        dir.join("abilities/cognition.scx"),
    )
    .expect("the double is copyable");
    let speaking_path = dir.join("abilities/speaking.scx");
    match speaking {
        Speaking::Real => {}
        Speaking::Deaf => {
            std::fs::write(&speaking_path, DEAF_SPEAKING).expect("the deaf speaking is writable");
        }
        Speaking::Missing => {
            std::fs::remove_file(&speaking_path).expect("the speaking copy is removable");
        }
    }
    for (path, data) in edits {
        rewrite(&dir.join(path), data);
    }
    dir.join("bots/floppy/floppy.scx")
}

fn label_is(label: &'static str) -> impl Fn(&Json) -> bool {
    move |record| record["type"] == "log" && record["label"] == label
}

/// The texts of the logs labelled `label`, in order.
fn texts(records: &[Json], label: &str) -> Vec<String> {
    logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .collect()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Records until a log of each of `labels` has been seen (their order
/// across sessions is not fixed).
fn until_all(run: &Run, labels: &[&str]) -> Vec<Json> {
    let seen = std::cell::RefCell::new(std::collections::BTreeSet::new());
    run.until(WAIT, |record| {
        if record["type"] == "log"
            && let Some(label) = record["label"].as_str()
            && labels.contains(&label)
        {
            seen.borrow_mut().insert(label.to_owned());
        }
        seen.borrow().len() == labels.len()
    })
}

fn started(body: &Path) -> Run {
    let mut run = Run::start(&[body.display().to_string()], &[]);
    run.event("bot.activate", &json!({}));
    run.until(WAIT, label_is("bot.activated"));
    run
}

/// Closes the run and asserts its pinned trace.
fn close(run: Run, test: &str) -> Finished {
    let finished = run.close(WAIT);
    trace::assert_trace(test, &finished.all, &[]);
    finished
}

fn input(action: &str) -> Json {
    json!({"observation": {"event": action, "data": action}})
}

fn said(text: &str) -> Json {
    json!({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": text, "content_type": "text/plain"})
}

#[test]
fn a_routed_response_is_said_and_recorded_in_the_conversation() {
    let body = layout(&Speaking::Real, &[]);
    let mut run = started(&body);
    run.event("bot.input", &input("reply"));
    let records = until_all(
        &run,
        &["communication.history", "bot.ability.speaking.output"],
    );
    assert_eq!(
        texts(&records, "bot.route"),
        strings(&["bot.ability.communication.respond -> communication"])
    );
    assert_eq!(texts(&records, "bot.say"), strings(&["reply 1"]));
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[r#"{"text":"reply 1"}"#])
    );
    // The outbound message is history only: no product, no new turn.
    assert_eq!(texts(&records, "conversation"), strings(&["outbound 0"]));
    assert_eq!(texts(&records, "communication.history"), strings(&["1"]));
    assert!(texts(&records, "communication.product").is_empty());
    // Back in active, the communication offers again (its first offers
    // came before the activation).
    let finished = close(
        run,
        "a_routed_response_is_said_and_recorded_in_the_conversation",
    );
    let mut all = records;
    all.extend(finished.records);
    assert_eq!(
        texts(&all, "communication.offers"),
        strings(&["bot.ability.communication.respond bot.ability.communication.activate"]),
        "{all:?}"
    );
    assert_eq!(
        texts(&all, "double.input"),
        strings(&[r#"1 reply "reply""#])
    );
}

#[test]
fn communication_input_is_a_turn_whose_stimulus_carries_the_history() {
    let body = layout(&Speaking::Real, &[]);
    let mut run = started(&body);
    run.event("bot.ability.communication.input", &said("reply"));
    let first = run.until(WAIT, label_is("communication.history"));
    run.event("bot.ability.communication.input", &said("reply"));
    let second = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "communication.history"
            && record["text"] == "4"
    });
    let mut all = first;
    all.extend(second);
    // Each inbound message is a product, so a turn whose stimulus is the
    // conversation's output: the double sees 1, then 3 messages.
    assert_eq!(
        texts(&all, "double.input"),
        strings(&["1 reply 1", "2 reply 3"])
    );
    assert_eq!(
        texts(&all, "conversation"),
        strings(&[
            r#"inbound 0 from ["gabe"] (1 in its transcript)"#,
            "outbound 1",
            r#"inbound 2 from ["gabe"] (2 in its transcript)"#,
            "outbound 3",
        ])
    );
    // The body gives the conversation a database: each inbound message
    // recalls the relationship's memories, then is remembered.
    assert!(texts(&all, "memory").is_empty(), "{all:?}");
    assert_eq!(
        texts(&all, "conversation.memories"),
        strings(&["recalled 0, remembered 1", "recalled 1, remembered 1"])
    );
    assert_eq!(texts(&all, "communication.product"), strings(&["0", "2"]));
    assert_eq!(texts(&all, "bot.say"), strings(&["reply 1", "reply 2"]));
    close(
        run,
        "communication_input_is_a_turn_whose_stimulus_carries_the_history",
    );
}

#[test]
fn the_history_reaching_a_turn_is_bounded_to_the_latest_messages() {
    let body = layout(
        &Speaking::Real,
        &[("abilities/conversation.scx", &[("max_messages", "2")])],
    );
    let mut run = started(&body);
    for _ in 0..2 {
        run.event("bot.ability.communication.input", &said("reply"));
        run.until(WAIT, label_is("communication.history"));
    }
    run.event("bot.ability.communication.input", &said("ignore"));
    let records = run.until(WAIT, label_is("double.ignore"));
    // The third input is sequence 4; the double sees its last 2 messages.
    assert_eq!(texts(&records, "double.input"), strings(&["3 ignore 2"]));
    assert_eq!(texts(&records, "communication.product"), strings(&["4"]));
    close(
        run,
        "the_history_reaching_a_turn_is_bounded_to_the_latest_messages",
    );
}

#[test]
fn a_blank_response_fails_without_speaking() {
    let body = layout(&Speaking::Real, &[]);
    let mut run = started(&body);
    run.event("bot.input", &input("blank"));
    let records = run.until(WAIT, label_is("bot.ability.communication.failed"));
    assert_eq!(
        texts(&records, "bot.ability.communication.failed"),
        strings(&[r#"{"message":"Communication response text must be non-blank."}"#])
    );
    assert!(texts(&records, "bot.say").is_empty(), "{records:?}");
    // Still active: the next response is said.
    run.event("bot.input", &input("reply"));
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(texts(&records, "bot.say"), strings(&["reply 2"]));
    close(run, "a_blank_response_fails_without_speaking");
}

#[test]
fn a_blank_response_that_waited_fails_without_speaking() {
    let body = layout(&Speaking::Real, &[]);
    let mut run = started(&body);
    run.event("bot.input", &input("then_blank"));
    let records = run.until(WAIT, label_is("bot.ability.communication.failed"));
    assert_eq!(
        texts(&records, "communication.deferred"),
        strings(&["bot.ability.communication.respond"])
    );
    assert_eq!(
        texts(&records, "bot.ability.communication.failed"),
        strings(&[r#"{"message":"Communication response text must be non-blank."}"#])
    );
    assert_eq!(texts(&records, "bot.say"), strings(&["first 1"]));
    // Still active: the next response is said.
    run.event("bot.input", &input("reply"));
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(texts(&records, "bot.say"), strings(&["reply 2"]));
    close(run, "a_blank_response_that_waited_fails_without_speaking");
}

#[test]
fn a_response_waits_for_the_one_in_flight_and_each_hop_times_out() {
    let body = layout(
        &Speaking::Deaf,
        &[(
            "abilities/communication.scx",
            &[("response_timeout_ms", "500")],
        )],
    );
    let mut run = started(&body);
    run.event("bot.input", &input("twice"));
    let first = run.until(WAIT, label_is("bot.ability.communication.failed"));
    let second = run.until(WAIT, label_is("bot.ability.communication.failed"));
    let mut all = first;
    all.extend(second);
    assert_eq!(
        texts(&all, "communication.deferred"),
        strings(&["bot.ability.communication.respond"])
    );
    assert_eq!(texts(&all, "deaf.input"), strings(&["first 1", "second 1"]));
    assert_eq!(
        texts(&all, "communication.hop.timed_out"),
        strings(&["response 1 after 500 ms", "response 2 after 500 ms"])
    );
    assert_eq!(
        texts(&all, "bot.ability.communication.failed"),
        strings(&[
            r#"{"message":"Speaking response timed out."}"#,
            r#"{"message":"Speaking response timed out."}"#,
        ])
    );
    close(
        run,
        "a_response_waits_for_the_one_in_flight_and_each_hop_times_out",
    );
}

#[test]
fn malformed_communication_input_is_refused_before_the_body() {
    let body = layout(&Speaking::Real, &[]);
    let mut run = started(&body);
    run.event(
        "bot.ability.communication.input",
        &json!({"source_ids": [], "target_ids": [], "content": "x", "content_type": "text/plain"}),
    );
    // The body's transition carries the input's schema (its twin), so scx
    // run refuses the event before the session sees it.
    let records = run.until(WAIT, |record| {
        record["type"] == "error" && record["code"] == "machine-invalid-input"
    });
    assert!(texts(&records, "bot.rejected").is_empty(), "{records:?}");
    assert!(texts(&records, "conversation").is_empty(), "{records:?}");
    close(
        run,
        "malformed_communication_input_is_refused_before_the_body",
    );
}

#[test]
fn a_communication_whose_speaking_cannot_start_fails_the_activation() {
    let body = layout(&Speaking::Missing, &[]);
    let mut run = Run::start(&[body.display().to_string()], &[]);
    run.event("bot.activate", &json!({}));
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    let degraded = texts(&records, "communication.degraded");
    assert_eq!(degraded.len(), 1, "{records:?}");
    assert!(
        degraded[0].starts_with("a child could not start (error."),
        "{degraded:?}"
    );
    assert_eq!(
        texts(&records, "bot.diagnostic"),
        strings(&["the ability communication ended while activating"])
    );
    close(
        run,
        "a_communication_whose_speaking_cannot_start_fails_the_activation",
    );
}

/// One external event for the conversation harness.
fn conversation_event(name: &str, data: &Json) -> (String, Json) {
    (name.to_owned(), data.clone())
}

#[test]
fn the_conversation_commits_each_message_into_its_history() {
    let dir = unique_scratch("conversation");
    copy_dir(
        &workspace().join("abilities/schemas"),
        &dir.join("abilities/schemas"),
    );
    std::fs::copy(
        workspace().join("abilities/conversation.scx"),
        dir.join("abilities/conversation.scx"),
    )
    .expect("the conversation is copyable");
    let harness = dir.join("abilities/harness.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/conversation_harness.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    let input = "bot.ability.conversation.input";
    let append = "bot.ability.conversation.append";
    // Unsorted string and embedding identities, an escaped quote, audio in
    // mixed case (dropped), structured content (kept), outbound references,
    // a repeated provenance id (not committed again), a record without
    // content or id, and the first participants again.
    let events = [
        conversation_event(
            input,
            &json!({"source_ids": ["zed", "alice", [0.5, 1]], "target_ids": ["bot", "a\"b"], "content": "hello", "content_type": "text/plain"}),
        ),
        conversation_event(
            input,
            &json!({"source_ids": ["alice"], "target_ids": [], "content": "AAAA", "content_type": "Audio/PCM", "sample_rate_hz": 16000, "channels": 1}),
        ),
        conversation_event(
            input,
            &json!({"source_ids": [[2, 1.25], "bob"], "target_ids": ["bot"], "content": {"b": 1, "a": [true, null]}, "content_type": "application/json"}),
        ),
        conversation_event(
            append,
            &json!({"message": {"sequence": 0, "direction": "outbound", "source_ids": ["bot"], "target_ids": ["zed", "alice"], "content": "hi", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "r-1", "session_ref": "s1", "turn_ref": "t1", "source": "q"}}}),
        ),
        conversation_event(
            append,
            &json!({"message": {"sequence": 0, "direction": "outbound", "source_ids": ["bot"], "target_ids": [], "content": "again", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "r-1"}}}),
        ),
        conversation_event(
            append,
            &json!({"message": {"sequence": 0, "direction": "outbound", "source_ids": [], "target_ids": [], "content_type": "text/plain", "provenance": {"event": append, "id": null}}}),
        ),
        conversation_event(
            input,
            &json!({"source_ids": ["alice", "zed", [0.5, 1]], "target_ids": ["a\"b", "bot"], "content": "again", "content_type": "text/plain"}),
        ),
    ];
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = Vec::new();
    for (name, data) in &events {
        run.event(name, data);
        // Each event ends in one conversation log (a commit, or the
        // repeated id).
        all.extend(run.until(WAIT, label_is("conversation")));
    }
    // Six commits, six outputs.
    let seen = std::cell::Cell::new(texts(&all, "harness.output").len());
    if seen.get() < 6 {
        all.extend(run.until(WAIT, |record| {
            if label_is("harness.output")(record) {
                seen.set(seen.get() + 1);
            }
            seen.get() == 6
        }));
    }
    assert_eq!(
        texts(&all, "conversation"),
        strings(&[
            r#"inbound 0 from ["alice","zed",[0.5,1]] (1 in its transcript)"#,
            r#"inbound 1 from ["alice"] (1 in its transcript)"#,
            r#"inbound 2 from ["bob",[2,1.25]] (1 in its transcript)"#,
            "outbound 3",
            "outbound r-1 already committed",
            "outbound 4",
            r#"inbound 5 from ["alice","zed",[0.5,1]] (2 in its transcript)"#,
        ])
    );
    let finished = run.close(WAIT);
    let outputs = texts(&finished.all, "harness.output");
    assert_eq!(outputs.len(), 6, "{outputs:?}");
    let last: Json = serde_json::from_str(&outputs[5]).expect("the output is JSON");
    assert_eq!(
        last["messages"][0]["provenance"]["session_ref"],
        json!(r#"relationship-["alice","zed",[0.5,1]]["a\"b","bot"]"#)
    );
    assert_eq!(last["messages"][1]["content"], Json::Null);
    trace::assert_trace(
        "the_conversation_commits_each_message_into_its_history",
        &finished.all,
        &[],
    );
}
