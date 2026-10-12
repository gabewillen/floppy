//! The floppy bot's communication (`abilities/communication/communication.scx` with its
//! children `abilities/conversation/conversation.scx` and `abilities/speaking/speaking.scx`),
//! run as the body's child through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! under `bots/floppy/`, the communication, conversation, speaking,
//! identity, memory and every schema under `abilities/`) with
//! `tests/fixtures/phase4/cognition_double.scx` as `abilities/cognition/cognition.scx`
//! (no model; its behavior per turn is the `bot.input` observation's name,
//! or a conversation product's last text). No backend is involved: the
//! communication path makes no service call.
//!
//! The speaking child logs `floppy.say`, the conversation each commit
//! (`conversation`) and what memory recalled (`conversation.memories`),
//! the communication what it forwards (`communication.product`,
//! `communication.history`, `communication.deferred`), and the body each
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
    /// No `abilities/speaking/speaking.scx` at all.
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
    support::copy_listening(&abilities);
    std::fs::create_dir_all(abilities.join("cognition")).expect("creatable");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/cognition_double.scx"),
        abilities.join("cognition/cognition.scx"),
    )
    .expect("the double is copyable");
    support::double_speech(&abilities, "'encode'");
    let speaking_path = abilities.join("speaking/speaking.scx");
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
        texts(&records, "floppy.route"),
        strings(&["bot.ability.communication.respond -> communication"])
    );
    assert_eq!(texts(&records, "floppy.say"), strings(&["reply 1"]));
    // Encoded by the speech service (its double), played out into the
    // environment by reference, then said.
    assert_eq!(
        texts(&records, "speech.encode"),
        strings(&["encode reply 1 -> media/speaking-1.pcm"])
    );
    assert_eq!(
        texts(&records, "environment.sound"),
        strings(&[
            r#"{"audio":"media/speaking-1.pcm","channels":1,"media_type":"audio/pcm","sample_rate_hz":24000}"#
        ])
    );
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[
            r#"{"channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"text":"reply 1"}"#
        ])
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
        strings(&["bot.ability.communication.respond"]),
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
    let run = history_carrying_turns(&body);
    close(
        run,
        "communication_input_is_a_turn_whose_stimulus_carries_the_history",
    );
}

/// The same on the Postgres engine: the body's database is a `PGlite`
/// server's, so the conversation recalls and remembers there.
#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_communication_input_is_a_turn_whose_stimulus_carries_the_history() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    let body = layout(&Speaking::Real, &[]);
    support::use_database(&body, &server.database());
    drop(history_carrying_turns(&body).close(WAIT));
}

/// [`communication_input_is_a_turn_whose_stimulus_carries_the_history`]
/// up to its close: two inputs, each a turn that sees the history.
fn history_carrying_turns(body: &Path) -> Run {
    let mut run = started(body);
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
            r#"inbound 0 from ["gabe"] (1 in its relationship)"#,
            "outbound 1",
            r#"inbound 2 from ["gabe"] (2 in its relationship)"#,
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
    assert_eq!(texts(&all, "floppy.say"), strings(&["reply 1", "reply 2"]));
    run
}

/// A product that is not text is no turn: the communication forwards the
/// conversation's output itself, whose latest message is inbound (the original
/// `_has_conversation_product`), and the body logs it.
#[test]
fn a_product_that_is_not_text_reaches_the_body_as_the_conversation_output() {
    let body = layout(&Speaking::Real, &[]);
    let mut run = started(&body);
    run.event(
        "bot.ability.communication.input",
        &json!({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": {"a": 1}, "content_type": "application/json"}),
    );
    let records = run.until(WAIT, label_is("bot.ability.conversation.output"));
    let output: Json = serde_json::from_str(&texts(&records, "bot.ability.conversation.output")[0])
        .expect("the output is JSON");
    assert_eq!(output["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(output["messages"][0]["direction"], json!("inbound"));
    assert_eq!(output["messages"][0]["content"], json!({"a": 1}));
    assert!(texts(&records, "communication.product").is_empty());
    assert!(texts(&records, "double.input").is_empty());
    drop(run.close(WAIT));
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
    assert!(texts(&records, "floppy.say").is_empty(), "{records:?}");
    // Still active: the next response is said.
    run.event("bot.input", &input("reply"));
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(texts(&records, "floppy.say"), strings(&["reply 2"]));
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
    assert_eq!(texts(&records, "floppy.say"), strings(&["first 1"]));
    // Still active: the next response is said.
    run.event("bot.input", &input("reply"));
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(texts(&records, "floppy.say"), strings(&["reply 2"]));
    close(run, "a_blank_response_that_waited_fails_without_speaking");
}

#[test]
fn a_response_waits_for_the_one_in_flight_and_each_hop_times_out() {
    let body = layout(
        &Speaking::Deaf,
        &[(
            "abilities/communication/communication.scx",
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
    assert!(texts(&records, "floppy.rejected").is_empty(), "{records:?}");
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
        texts(&records, "floppy.diagnostic"),
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

/// The relationships' references in
/// [`the_conversation_commits_each_message_into_its_history`]: the original's
/// `relationship_context_ref`, the same in either direction.
const FIRST: &str = r#"relationship-{"identity_groups":[["a\"b","bot"],["alice","zed",[0.447213595499958,0.894427190999916]]]}"#;
const AMBIENT: &str = r#"relationship-{"identity_groups":[[],["alice"]]}"#;
const THIRD: &str =
    r#"relationship-{"identity_groups":[["bob",[0.847998304005088,0.52999894000318]],["bot"]]}"#;

/// The events of [`the_conversation_commits_each_message_into_its_history`]:
/// unsorted string and embedding identities, an escaped quote, audio in
/// mixed case (dropped), structured content (kept), outbound references,
/// the same provenance id from another source (committed) and from the
/// same one (not committed again, still answered), a record without
/// content or id, the first participants again, then in the other
/// direction, another targetless source (the ambient relationship), and
/// input addressed to an ambient participant.
fn conversation_history_events() -> [(String, Json); 11] {
    let input = "bot.ability.conversation.input";
    let append = "bot.ability.conversation.append";
    [
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
            &json!({"message": {"sequence": 0, "direction": "outbound", "source_ids": ["bot"], "target_ids": [], "content": "once", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "r-1", "source": "q"}}}),
        ),
        conversation_event(
            append,
            &json!({"message": {"sequence": 0, "direction": "outbound", "source_ids": [], "target_ids": [], "content_type": "text/plain", "provenance": {"event": append, "id": null}}}),
        ),
        conversation_event(
            input,
            &json!({"source_ids": ["alice", "zed", [0.5, 1]], "target_ids": ["a\"b", "bot"], "content": "again", "content_type": "text/plain"}),
        ),
        conversation_event(
            input,
            &json!({"source_ids": ["bot", "a\"b"], "target_ids": ["zed", [0.5, 1], "alice"], "content": "back", "content_type": "text/plain"}),
        ),
        conversation_event(
            input,
            &json!({"source_ids": ["bob"], "target_ids": [], "content": "aside", "content_type": "text/plain"}),
        ),
        conversation_event(
            input,
            &json!({"source_ids": ["carol"], "target_ids": ["alice"], "content": "to alice", "content_type": "text/plain"}),
        ),
    ]
}

/// The snapshot after [`the_conversation_commits_each_message_into_its_history`]:
/// each relationship's participants, by relationship and then identity.
fn assert_snapshot(all: &[Json], first: &str, ambient: &str, third: &str) {
    let snapshot: Json =
        serde_json::from_str(&texts(all, "harness.snapshot")[0]).expect("the snapshot is JSON");
    assert_eq!(
        snapshot,
        json!({"request_ref": "s-1", "detector_refs": [
            [first, "a\"b"], [first, "alice"], [first, "bot"], [first, "zed"], [first, [0.5, 1]],
            [third, "bob"], [third, [2, 1.25]],
            [ambient, "alice"], [ambient, "bob"], [ambient, "carol"],
        ]})
    );
}

#[test]
fn the_conversation_commits_each_message_into_its_history() {
    let dir = unique_scratch("conversation");
    support::copy_ability("conversation", &dir.join("abilities"));
    let harness = dir.join("abilities/harness.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/conversation_harness.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    let events = conversation_history_events();
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = Vec::new();
    for (name, data) in &events {
        run.event(name, data);
        // Each event ends in one conversation log (a commit, or the
        // repeated id).
        all.extend(run.until(WAIT, label_is("conversation")));
    }
    // Eleven answers.
    let seen = std::cell::Cell::new(texts(&all, "harness.output").len());
    if seen.get() < 11 {
        all.extend(run.until(WAIT, |record| {
            if label_is("harness.output")(record) {
                seen.set(seen.get() + 1);
            }
            seen.get() == 11
        }));
    }
    run.event(
        "bot.ability.conversation.snapshot.request",
        &json!({"request_ref": "s-1"}),
    );
    all.extend(run.until(WAIT, label_is("harness.snapshot")));
    assert_eq!(
        texts(&all, "conversation"),
        strings(&[
            r#"inbound 0 from ["alice","zed",[0.5,1]] (1 in its relationship)"#,
            r#"inbound 1 from ["alice"] (1 in its relationship)"#,
            r#"inbound 2 from ["bob",[2,1.25]] (1 in its relationship)"#,
            "outbound 3",
            "outbound 4",
            "outbound r-1 already committed",
            "outbound 5",
            r#"inbound 6 from ["alice","zed",[0.5,1]] (2 in its relationship)"#,
            r#"inbound 7 from ["a\"b","bot"] (3 in its relationship)"#,
            r#"inbound 8 from ["bob"] (2 in its relationship)"#,
            r#"inbound 9 from ["carol"] (3 in its relationship)"#,
        ])
    );
    assert_snapshot(&all, FIRST, AMBIENT, THIRD);
    let finished = run.close(WAIT);
    let outputs = texts(&finished.all, "harness.output");
    assert_eq!(outputs.len(), 11, "{outputs:?}");
    let last: Json = serde_json::from_str(&outputs[10]).expect("the output is JSON");
    let messages = &last["messages"];
    for (sequence, session_ref) in [
        (0, FIRST),
        (1, AMBIENT),
        (2, THIRD),
        (6, FIRST),
        (7, FIRST),
        (8, AMBIENT),
        (9, AMBIENT),
    ] {
        assert_eq!(
            messages[sequence]["provenance"]["session_ref"],
            json!(session_ref),
            "{sequence}"
        );
    }
    assert_eq!(messages[1]["content"], Json::Null);
    // Each source's turn (the original's identity order: names, then
    // embeddings) is exchanged with its turn detector, and the turns'
    // texts are committed joined, each distinct text once (D79).
    assert_eq!(messages[0]["content"], json!("hello"));
    assert_eq!(
        messages[0]["provenance"]["turn_ref"],
        json!("input-1:turn:alice")
    );
    assert_eq!(
        messages[1]["provenance"]["turn_ref"],
        json!("input-2:turn:alice")
    );
    assert_eq!(messages[2]["content"], json!({"b": 1, "a": [true, null]}));
    assert_eq!(
        messages[2]["provenance"]["turn_ref"],
        json!("input-3:turn:bob")
    );
    // An append's provenance keeps its id, source and target.
    assert_eq!(
        (
            &messages[3]["provenance"]["source"],
            &messages[4]["provenance"]["source"]
        ),
        (&json!("q"), &Json::Null)
    );
    assert_eq!(messages[6]["content"], json!("again"));
    assert_eq!(
        messages[7]["provenance"]["turn_ref"],
        json!("input-5:turn:a\"b")
    );
    // The duplicate is answered with the history it did not change.
    let duplicate: Json = serde_json::from_str(&outputs[5]).expect("the output is JSON");
    assert_eq!(duplicate["messages"].as_array().map(Vec::len), Some(5));
    trace::assert_trace(
        "the_conversation_commits_each_message_into_its_history",
        &finished.all,
        &[],
    );
}

/// The remembered input is the original's `Memory.to_json` (the expected texts
/// are its own).
#[test]
fn the_remembered_input_is_the_original_memory_json() {
    let dir = unique_scratch("conversation-payload");
    std::fs::copy(
        workspace().join("abilities/conversation/conversation.js"),
        dir.join("conversation.js"),
    )
    .expect("the script is copyable");
    let harness = dir.join("payload.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/conversation_payload.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = Vec::new();
    for input in [
        json!({"source_ids": ["zed", "alice", [0.5, 1]], "target_ids": ["bot", "\u{e9}"], "content": {"b": 1, "a": [true, null, 1.5], "\u{e9}": "x"}, "content_type": "application/json"}),
        json!({"source_ids": [[1e-5, 2], [3, 4], "q\"\n"], "target_ids": [], "content": ["\u{1f600}", 1e-7, 12_345_678_901_234_567_890.5_f64, {"\u{7f}": [], "z": {}}], "content_type": "text/x"}),
    ] {
        run.event("payload", &input);
        all.extend(run.until(WAIT, label_is("payload")));
    }
    drop(run.close(WAIT));
    assert_eq!(
        texts(&all, "payload"),
        strings(&[
            r#"{"content":{"__stateforward_memory_content__":"mapping","value":[["é","x"],["a",{"__stateforward_memory_content__":"list","value":[true,null,1.5]}],["b",1]]},"content_type":"application/json","source_ids":["alice","zed",[0.5,1.0]],"target_ids":["bot","é"]}"#,
            "{\"content\":{\"__stateforward_memory_content__\":\"list\",\"value\":[\"\u{1f600}\",1e-07,1.2345678901234567e+19,{\"__stateforward_memory_content__\":\"mapping\",\"value\":[[\"\u{7f}\",{\"__stateforward_memory_content__\":\"list\",\"value\":[]}],[\"z\",{\"__stateforward_memory_content__\":\"mapping\",\"value\":[]}]]}]},\"content_type\":\"text/x\",\"source_ids\":[\"q\\\"\\n\",[1e-05,2.0],[3.0,4.0]],\"target_ids\":[]}",
        ])
    );
}

#[test]
fn a_turn_its_detector_cannot_close_fails_the_input_and_commits_nothing() {
    let dir = unique_scratch("conversation");
    support::copy_ability("conversation", &dir.join("abilities"));
    let harness = dir.join("abilities/harness.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/conversation_harness.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    let input = "bot.ability.conversation.input";
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    // Sent in one macrostep, so the second and third wait for the first (the original
    // defers them). White space only: the detector closes the turn with
    // nothing to show. Empty text: the start's text stimulus is refused
    // before the detector. Then text, stripped by the detector before it
    // is committed once, not once per source.
    let inputs: Vec<Json> = ["   ", "", "  hi  "]
        .iter()
        .map(|content| {
            json!({"source_ids": ["alice", [3, 4]], "target_ids": [], "content": content, "content_type": "text/plain"})
        })
        .collect();
    run.event("harness.burst", &json!({ "inputs": inputs }));
    let seen = std::cell::Cell::new(0);
    let all = run.until(WAIT, |record| {
        if label_is("harness.failed")(record) || label_is("harness.output")(record) {
            seen.set(seen.get() + 1);
        }
        seen.get() == 3
    });
    // The second and third wait for the first, then the third waits again
    // for the second.
    assert_eq!(
        texts(&all, "conversation.deferred"),
        strings(&[input, input, input])
    );
    let finished = run.close(WAIT);
    let failures: Vec<Json> = texts(&all, "harness.failed")
        .iter()
        .map(|text| serde_json::from_str(text).expect("a failure is JSON"))
        .collect();
    assert_eq!(
        failures,
        vec![
            json!({"stage": "turn_detector", "message": "Turn closed with neither text, audio, nor modality-neutral content."}),
            json!({"stage": "turn_detector", "message": "The turn detector could not take the turn (error.execution)"}),
        ]
    );
    let outputs = texts(&finished.all, "harness.output");
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    let output: Json = serde_json::from_str(&outputs[0]).expect("the output is JSON");
    assert_eq!(output["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(output["messages"][0]["sequence"], json!(0));
    assert_eq!(output["messages"][0]["content"], json!("hi"));
    assert_eq!(
        output["messages"][0]["provenance"]["turn_ref"],
        json!("input-3:turn:alice")
    );
}

/// The body's input and Listening abilities are `['identity']`, and the
/// identity is `tests/fixtures/phase4/input_double.scx`, which logs what it
/// hears (`input.sound`) and its efference copies (`input.efference`).
fn with_listening(body: &Path) {
    let root = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the body is under bots/floppy/");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/input_double.scx"),
        root.join("abilities/identity/identity.scx"),
    )
    .expect("the input double is copyable");
    rewrite(
        body,
        &[
            ("input", "['identity']"),
            ("listening_abilities", "['identity']"),
        ],
    );
}

#[test]
fn a_spoken_response_reaches_listening_as_its_efference_copy_before_its_sound() {
    let body = layout(&Speaking::Real, &[]);
    with_listening(&body);
    let mut run = started(&body);
    run.event("bot.input", &input("reply"));
    let records = until_all(&run, &["input.sound", "bot.ability.speaking.output"]);
    // "reply 1" is 7 characters: the double measures 0.7 s.
    assert_eq!(
        texts(&records, "input.efference"),
        strings(&[
            r#"{"channels":1,"duration":0.7,"media_type":"audio/pcm","mouth":"speaking","sample_rate_hz":24000}"#
        ])
    );
    assert_eq!(
        texts(&records, "input.sound"),
        strings(&[
            r#"{"audio":"media/speaking-1.pcm","channels":1,"media_type":"audio/pcm","sample_rate_hz":24000}"#
        ])
    );
    let order: Vec<String> = logs(&records)
        .into_iter()
        .map(|(label, _)| label)
        .filter(|label| ["input.efference", "input.sound"].contains(&label.as_str()))
        .collect();
    assert_eq!(order, strings(&["input.efference", "input.sound"]));
    close(
        run,
        "a_spoken_response_reaches_listening_as_its_efference_copy_before_its_sound",
    );
}

#[test]
fn unmeasured_speech_sends_no_efference_copy_and_takes_speakings_format() {
    let body = layout(
        &Speaking::Real,
        &[(
            "abilities/vocal/speech_double.scx",
            &[("mode", "'unmeasured'")],
        )],
    );
    with_listening(&body);
    let mut run = started(&body);
    run.event("bot.input", &input("reply"));
    let records = until_all(&run, &["input.sound", "bot.ability.speaking.output"]);
    assert!(texts(&records, "input.efference").is_empty(), "{records:?}");
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[
            r#"{"channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"text":"reply 1"}"#
        ])
    );
    close(
        run,
        "unmeasured_speech_sends_no_efference_copy_and_takes_speakings_format",
    );
}

#[test]
fn a_speech_service_failure_fails_the_response_unsaid_and_unrecorded() {
    let body = layout(
        &Speaking::Real,
        &[("abilities/vocal/speech_double.scx", &[("mode", "'fail'")])],
    );
    let mut run = started(&body);
    run.event("bot.input", &input("reply"));
    let records = run.until(WAIT, label_is("bot.ability.communication.failed"));
    assert_eq!(
        texts(&records, "bot.ability.communication.failed"),
        strings(&[r#"{"message":"unconfigured"}"#])
    );
    assert!(texts(&records, "floppy.say").is_empty());
    assert!(texts(&records, "environment.sound").is_empty());
    assert!(texts(&records, "conversation").is_empty());
    close(
        run,
        "a_speech_service_failure_fails_the_response_unsaid_and_unrecorded",
    );
}

#[test]
fn speech_encoding_is_bounded_by_its_timeout() {
    let body = layout(
        &Speaking::Real,
        &[
            ("abilities/vocal/speech_double.scx", &[("mode", "'silent'")]),
            (
                "abilities/communication/communication.scx",
                &[("encoding_timeout_ms", "300")],
            ),
        ],
    );
    let mut run = started(&body);
    run.event("bot.input", &input("reply"));
    let records = run.until(WAIT, label_is("bot.ability.communication.failed"));
    assert_eq!(
        texts(&records, "speaking.timed_out"),
        strings(&["encoding 1 after 300 ms"])
    );
    assert_eq!(
        texts(&records, "bot.ability.communication.failed"),
        strings(&[r#"{"message":"Speech encoding timed out."}"#])
    );
    assert!(texts(&records, "floppy.say").is_empty());
    close(run, "speech_encoding_is_bounded_by_its_timeout");
}

/// A fake Gemini speech backend on 127.0.0.1: every
/// `POST /v1/interactions` answers a tenth of a second of raw 24 kHz mono
/// 16-bit PCM (4800 bytes, base64 in its audio part), and its request
/// bodies are recorded.
fn speech_backend() -> (String, std::sync::Arc<std::sync::Mutex<Vec<Json>>>) {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("the backend binds a loopback port");
    let origin = format!(
        "http://{}",
        listener.local_addr().expect("the backend has an address")
    );
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
            let mut line = String::new();
            let mut length = 0;
            while reader.read_line(&mut line).is_ok() && line != "\r\n" && !line.is_empty() {
                if let Some((name, value)) = line.trim_end().split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            log.lock()
                .expect("the request log")
                .push(serde_json::from_slice(&body).unwrap_or(Json::Null));
            // 4800 zero bytes in base64.
            let answer = json!({"steps": [{"type": "model_output", "content": [{
                "type": "audio", "data": "A".repeat(6400), "sample_rate": 24000,
                "channels": 1, "mime_type": "audio/l16; rate=24000; channels=1",
            }]}]})
            .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (origin, requests)
}

#[test]
fn a_response_is_spoken_through_the_speech_service() {
    let body = layout(&Speaking::Real, &[]);
    with_listening(&body);
    let abilities = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the body is under bots/floppy/")
        .join("abilities");
    // The real vocal ability, which invokes the speech service.
    std::fs::copy(
        workspace().join("abilities/vocal/vocal.scx"),
        abilities.join("vocal/vocal.scx"),
    )
    .expect("the vocal ability is copyable");
    std::fs::remove_file(abilities.join("vocal/speech_double.scx"))
        .expect("the double is removable");
    let (origin, requests) = speech_backend();
    let media = unique_scratch("speaking-media");
    let env = media.join("speech.env");
    std::fs::write(
        &env,
        format!(
            "FLOPPY_SPEECH_BASE_URL={origin}/v1\nFLOPPY_SPEECH_API_KEY=sk-floppy-speech-fake\n"
        ),
    )
    .expect("the env file is writable");
    let mut run = Run::start(
        &[
            "--env-file".to_owned(),
            env.display().to_string(),
            "--dir".to_owned(),
            format!(
                "{}={}::/media",
                support::service_uri("speech"),
                media.display()
            ),
            body.display().to_string(),
        ],
        &[],
    );
    run.event("bot.activate", &json!({}));
    run.until(WAIT, label_is("bot.activated"));
    run.event("bot.input", &input("reply"));
    let records = until_all(&run, &["input.sound", "bot.ability.speaking.output"]);
    // 4800 bytes of 24 kHz mono 16-bit PCM: a tenth of a second, measured.
    assert_eq!(
        texts(&records, "input.efference"),
        strings(&[
            r#"{"channels":1,"duration":0.1,"media_type":"audio/pcm","mouth":"speaking","sample_rate_hz":24000}"#
        ])
    );
    assert_eq!(
        texts(&records, "input.sound"),
        strings(&[
            r#"{"audio":"media/speaking-1.pcm","channels":1,"media_type":"audio/pcm","sample_rate_hz":24000}"#
        ])
    );
    assert_eq!(texts(&records, "floppy.say"), strings(&["reply 1"]));
    let written = std::fs::read(media.join("speaking-1.pcm")).expect("the audio was written");
    assert_eq!(written.len(), 4800);
    let requests = requests.lock().expect("the request log").clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    // speaking's defaults, the original's gemini speech encoder, model and voice.
    assert_eq!(requests[0]["model"], "gemini-3.1-flash-tts-preview");
    assert_eq!(requests[0]["input"], "reply 1");
    assert_eq!(
        requests[0]["generation_config"],
        json!({"speech_config": [{"voice": "Kore"}]})
    );
    assert_eq!(requests[0]["response_format"], json!({"type": "audio"}));
    close(run, "a_response_is_spoken_through_the_speech_service");
}
