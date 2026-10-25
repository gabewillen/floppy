//! The floppy bot's communication (`abilities/communication/communication.scx` with its
//! children `abilities/conversation/conversation.scx` and `abilities/speaking/speaking.scx`),
//! run as the body's child through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! under `bots/floppy/`, the communication, conversation, speaking,
//! identity, memory, the short-term register and every schema under
//! `abilities/`) with
//! `tests/fixtures/phase4/cognition_double.scx` as `abilities/cognition/cognition.scx`
//! (no model; its behavior per turn is the `bot.input` observation's name,
//! or a conversation product's last text). No backend is involved: the
//! communication path makes no service call.
//!
//! The speaking child logs `floppy.say`, the conversation each commit
//! (`conversation`) and what memory recalled (`conversation.memories`),
//! the communication what it forwards (`communication.product`,
//! `communication.exchange`, `communication.deferred`), and the body each
//! event it emits (`bot.ability.speaking.output`,
//! `bot.ability.communication.failed`).
//!
//! `the_conversation_commits_each_message_into_its_exchange` runs the
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
      <send event="bot.ability.ready" target="#_parent"/>
    </onentry>
    <transition event="bot.ability.speaking.input">
      <log label="deaf.input" expr="_event.data.parts.map(function (part) { return part.words; }).join('')"/>
    </transition>
  </state>
</scxml>
"##;

/// A parent of the communication alone (`abilities/harness.scx`): it
/// sends each `harness.<id>` as a `bot.ability.communication.respond`
/// whose sendid is `<id>` (an SCXML `<send id>` is literal), and
/// `harness.stop` as `bot.ability.communication.stop`, forwards outside
/// `bot.ability.communication.input`, and logs what the communication
/// answers (`harness.<event>`, its JSON) and each efference copy's duration
/// (`harness.efference`). It is the environment speaking's sound goes into.
const COMMUNICATION_HARNESS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="harness" initial="run">
  <state id="run">
    <invoke id="communication" type="scxml" src="communication/communication.scx">
      <param name="environment" expr="_sessionid"/>
    </invoke>
    <transition event="bot.ability.ready" cond="_event.invokeid === 'communication'">
      <log label="harness.ready" expr="'communication'"/>
    </transition>
    <transition event="bot.ability.communication.failed bot.ability.speaking.output" cond="_event.invokeid === 'communication'">
      <log label="harness.answer" expr="_event.name + ' ' + JSON.stringify(_event.data)"/>
    </transition>
    <transition event="bot.ability.cognition.input" cond="_event.invokeid === 'communication'">
      <log label="harness.product" expr="_event.data.stimulus.data.messages.length"/>
    </transition>
    <transition event="bot.ability.speaking.efference" cond="_event.invokeid === 'communication'">
      <log label="harness.efference" expr="_event.data.duration"/>
    </transition>
    <transition event="environment.sound">
      <log label="harness.sound" expr="_event.data.audio"/>
    </transition>
    <transition event="harness.stop" cond="_event.invokeid === undefined">
      <send event="bot.ability.communication.stop" target="#_communication"><content expr="({})"/></send>
    </transition>
    <transition event="bot.ability.communication.input" cond="_event.invokeid === undefined">
      <send event="bot.ability.communication.input" target="#_communication">
        <content expr="_event.data"/>
      </send>
    </transition>
    <transition event="harness.r1" cond="_event.invokeid === undefined">
      <send id="r1" event="bot.ability.communication.respond" target="#_communication"><content expr="_event.data"/></send>
    </transition>
    <transition event="harness.r2" cond="_event.invokeid === undefined">
      <send id="r2" event="bot.ability.communication.respond" target="#_communication"><content expr="_event.data"/></send>
    </transition>
    <transition event="harness.r3" cond="_event.invokeid === undefined">
      <send id="r3" event="bot.ability.communication.respond" target="#_communication"><content expr="_event.data"/></send>
    </transition>
    <transition event="harness.r4" cond="_event.invokeid === undefined">
      <send id="r4" event="bot.ability.communication.respond" target="#_communication"><content expr="_event.data"/></send>
    </transition>
  </state>
</scxml>
"##;

/// Lays out the communication under [`COMMUNICATION_HARNESS`] with a deaf
/// speaking and its `<data>` `edits`; returns the harness.
fn communication_alone(edits: &[(&str, &str)]) -> PathBuf {
    let dir = unique_scratch("communication-alone");
    let abilities = dir.join("abilities");
    for name in ["communication", "conversation", "speaking", "memory"] {
        support::copy_ability(name, &abilities);
    }
    std::fs::write(abilities.join("speaking/speaking.scx"), DEAF_SPEAKING)
        .expect("the deaf speaking is writable");
    rewrite(&abilities.join("communication/communication.scx"), edits);
    let harness = abilities.join("harness.scx");
    std::fs::write(&harness, COMMUNICATION_HARNESS).expect("the harness is writable");
    harness
}

/// Lays out the communication under [`COMMUNICATION_HARNESS`] with the
/// repository's speaking (its speech service the double) and the `<data>`
/// `edits` of the communication and of speaking; returns the harness.
fn communication_speaking(edits: &[(&str, &str)], speaking: &[(&str, &str)]) -> PathBuf {
    let harness = communication_alone(edits);
    let abilities = harness.parent().expect("the harness is in abilities/");
    support::copy_ability("speaking", abilities);
    support::double_speech(abilities, "'encode'");
    rewrite(&abilities.join("speaking/speaking.scx"), speaking);
    harness
}

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
        &workspace().join("bots/floppy/firmware"),
        &dir.join("bots/floppy/firmware"),
    );
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
        "stm",
        "persona",
        "seeing",
    ] {
        support::copy_ability(name, &abilities);
    }
    support::copy_listening(&abilities);
    support::copy_cognition_double(&abilities);
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

/// Whether `record` is the listening's log of the efference copy of
/// nothing left (`duration` 0).
fn is_silent_copy(record: &Json) -> bool {
    label_is("input.efference")(record)
        && record["text"]
            .as_str()
            .is_some_and(|text| text.contains(r#""duration":0,"#))
}

/// Adds records until the copy of nothing left is among them: it can reach
/// the listening after speaking's answer reaches the body.
fn until_silent_copy(run: &Run, records: &mut Vec<Json>) {
    if !records.iter().any(is_silent_copy) {
        records.extend(run.until(WAIT, is_silent_copy));
    }
}

/// Starts the environment with the laid-out body as its bot, which activates on
/// joining.
fn started(body: &Path) -> Run {
    let run = Run::start(&[support::environment_for(body, &[])], &[]);
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
        &["communication.exchange", "bot.ability.speaking.output"],
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
            r#"{"audio":"media/speaking-1.0.pcm","channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"source":"speaking"}"#
        ])
    );
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[
            r#"{"channels":1,"media_type":"audio/pcm","outcome":"completed","sample_rate_hz":24000,"spoken":"reply 1","text":"reply 1","unspoken":"","voice_id":null}"#
        ])
    );
    // The outbound message joins the exchange only: no product, no new
    // turn.
    assert_eq!(texts(&records, "conversation"), strings(&["outbound 0"]));
    assert_eq!(texts(&records, "communication.exchange"), strings(&["1"]));
    assert!(texts(&records, "communication.product").is_empty());
    let finished = close(
        run,
        "a_routed_response_is_said_and_recorded_in_the_conversation",
    );
    let mut all = records;
    all.extend(finished.records);
    assert_eq!(
        texts(&all, "double.input"),
        strings(&[r#"1 reply "reply""#])
    );
}

#[test]
fn communication_input_is_a_turn_whose_stimulus_carries_its_exchange() {
    let body = layout(&Speaking::Real, &[]);
    let run = exchange_carrying_turns(&body);
    close(
        run,
        "communication_input_is_a_turn_whose_stimulus_carries_its_exchange",
    );
}

/// The same on the Postgres engine: the body's database is a `PGlite`
/// server's, so the conversation recalls and remembers there.
#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_communication_input_is_a_turn_whose_stimulus_carries_its_exchange() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    let body = layout(&Speaking::Real, &[]);
    support::use_database(&body, &server.database());
    drop(exchange_carrying_turns(&body).close(WAIT));
}

/// [`communication_input_is_a_turn_whose_stimulus_carries_its_exchange`]
/// up to its close: two inputs, each a turn that sees its exchange.
fn exchange_carrying_turns(body: &Path) -> Run {
    let mut run = started(body);
    run.event("bot.ability.communication.input", &said("reply"));
    let first = run.until(WAIT, label_is("communication.exchange"));
    run.event("bot.ability.communication.input", &said("reply"));
    let second = run.until(WAIT, label_is("communication.exchange"));
    let mut all = first;
    all.extend(second);
    // Each inbound message is a product, so a turn whose stimulus is the
    // conversation's output, its exchange: an inbound commit opens a new
    // exchange, so the double sees one message each time.
    assert_eq!(
        texts(&all, "double.input"),
        strings(&["1 reply 1", "2 reply 1"])
    );
    assert_eq!(texts(&all, "communication.exchange"), strings(&["2", "2"]));
    // The conversation's handoff stamps the turn's names as participants
    // (decision D119), and the body copies them into the turn.
    let stamped = r#"[{"ref":"gabe","role":"source"},{"ref":"floppy","role":"target"}]"#;
    assert_eq!(
        texts(&all, "double.participants"),
        strings(&[stamped, stamped])
    );
    assert_eq!(
        texts(&all, "conversation"),
        strings(&[
            r#"inbound from ["gabe"] in relationship 0"#,
            "outbound 1",
            r#"inbound from ["gabe"] in relationship 0"#,
            "outbound 1",
        ])
    );
    // The body gives the conversation a database: each inbound message
    // recalls the relationship's memories, then is remembered.
    assert!(texts(&all, "memory").is_empty(), "{all:?}");
    assert_eq!(
        texts(&all, "conversation.memories"),
        strings(&["recalled 0, remembered 1", "recalled 1, remembered 1"])
    );
    assert_eq!(
        texts(&all, "communication.product"),
        strings(&["input-1:turn:gabe", "input-2:turn:gabe"])
    );
    assert_eq!(texts(&all, "floppy.say"), strings(&["reply 1", "reply 2"]));
    run
}

/// A product that is not text is no turn: the communication forwards the
/// conversation's output itself, whose latest message is inbound, and
/// the body logs it.
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
        strings(&[
            r#"{"message":"A reply must say something: every part needs words, a sound or a pause, and the words must not be blank."}"#
        ])
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
        strings(&[
            r#"{"message":"A reply must say something: every part needs words, a sound or a pause, and the words must not be blank."}"#
        ])
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

/// Hearing goes on while a response is said, and every response that
/// finished stays finished (hearing in `responding`, the ring of closed
/// response ids, and the bound on held responses; decision D175).
#[test]
fn communication_hears_while_responding_and_never_reopens_a_finished_response() {
    let harness = communication_alone(&[
        ("response_timeout_ms", "4000"),
        ("held_responses_bound", "1"),
    ]);
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = run.until(WAIT, label_is("harness.ready"));
    run.event("harness.r1", &json!({"parts": [{"words": "one"}]}));
    all.extend(run.until(WAIT, label_is("deaf.input")));
    // Heard while r1 is being said: the conversation commits it and its
    // product reaches the parent before r1's hop times out.
    run.event("bot.ability.communication.input", &said("hello"));
    all.extend(run.until(WAIT, label_is("harness.product")));
    all.extend(run.until(WAIT, label_is("harness.answer")));
    // A late copy of r1 is dropped.
    run.event("harness.r1", &json!({"parts": [{"words": "one again"}]}));
    all.extend(run.until(WAIT, label_is("communication.late")));
    // r2 in hand: another copy of it is dropped, r3 is held, and r4 is
    // past the bound of one held response.
    run.event("harness.r2", &json!({"parts": [{"words": "two"}]}));
    all.extend(run.until(WAIT, |record| {
        label_is("deaf.input")(record) && record["text"] == "two"
    }));
    run.event("harness.r2", &json!({"parts": [{"words": "two again"}]}));
    all.extend(run.until(WAIT, |record| {
        label_is("communication.late")(record) && record["text"] == "r2"
    }));
    run.event("harness.r3", &json!({"parts": [{"words": "three"}]}));
    all.extend(run.until(WAIT, label_is("communication.deferred")));
    run.event("harness.r4", &json!({"parts": [{"words": "four"}]}));
    all.extend(run.until(WAIT, label_is("harness.answer")));
    // r2 times out, and the held r3 is said.
    all.extend(run.until(WAIT, |record| {
        label_is("deaf.input")(record) && record["text"] == "three"
    }));
    all.extend(run.until(WAIT, |record| {
        label_is("communication.hop.timed_out")(record)
            && record["text"] == "response 3 after 4000 ms"
    }));
    let finished = run.close(WAIT);
    trace::assert_trace(
        "communication_hears_while_responding_and_never_reopens_a_finished_response",
        &finished.all,
        &[],
    );
    all.extend(finished.records);
    assert_eq!(texts(&all, "deaf.input"), strings(&["one", "two", "three"]));
    assert_eq!(
        texts(&all, "conversation"),
        strings(&[r#"inbound from ["gabe"] in relationship 0"#])
    );
    let labels: Vec<String> = logs(&all)
        .into_iter()
        .map(|(label, _)| label)
        .filter(|label| label == "conversation" || label == "communication.hop.timed_out")
        .collect();
    assert_eq!(
        labels.first().map(String::as_str),
        Some("conversation"),
        "heard before the first response timed out: {labels:?}"
    );
    assert_eq!(texts(&all, "communication.late"), strings(&["r1", "r2"]));
    assert_eq!(
        texts(&all, "harness.answer"),
        strings(&[
            r#"bot.ability.communication.failed {"message":"Speaking response timed out."}"#,
            r#"bot.ability.communication.failed {"message":"Communication is holding too many responses; this one is refused."}"#,
            r#"bot.ability.communication.failed {"message":"Speaking response timed out."}"#,
            r#"bot.ability.communication.failed {"message":"Speaking response timed out."}"#,
        ])
    );
}

/// A parent of the speaking alone (`abilities/harness.scx`): it sends each
/// `harness.<id>` as a `bot.ability.speaking.input` whose sendid is `<id>`
/// and logs what the speaking answers (`harness.answer`).
const SPEAKING_HARNESS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="harness" initial="run">
  <state id="run">
    <invoke id="speaking" type="scxml" src="speaking/speaking.scx"/>
    <transition event="bot.ability.ready" cond="_event.invokeid === 'speaking'">
      <log label="harness.ready" expr="'speaking'"/>
    </transition>
    <transition event="bot.ability.failed bot.ability.speaking.output" cond="_event.invokeid === 'speaking'">
      <log label="harness.answer" expr="_event.name + ' ' + JSON.stringify(_event.data)"/>
    </transition>
    <transition event="harness.u1" cond="_event.invokeid === undefined">
      <send id="u1" event="bot.ability.speaking.input" target="#_speaking"><content expr="_event.data"/></send>
    </transition>
    <transition event="harness.u2" cond="_event.invokeid === undefined">
      <send id="u2" event="bot.ability.speaking.input" target="#_speaking"><content expr="_event.data"/></send>
    </transition>
    <transition event="harness.u3" cond="_event.invokeid === undefined">
      <send id="u3" event="bot.ability.speaking.input" target="#_speaking"><content expr="_event.data"/></send>
    </transition>
  </state>
</scxml>
"##;

/// A stop while a response is said (offered only while responding,
/// decision D179) stops speaking: the copy of nothing left closes the
/// listening's window, and the response ends `stopped` with what was
/// played. With 2 s pieces, the 4 s reply is stopped after its first piece:
/// half its audio, so half its words.
#[test]
fn a_stop_ends_the_response_with_what_was_played() {
    let harness = communication_speaking(&[], &[("lookahead_ms", "4000")]);
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = run.until(WAIT, label_is("harness.ready"));
    // 40 characters: the double measures 4 s.
    let text = "alpha bravo charlie delta echo foxtrot!!";
    run.event("harness.r1", &json!({"parts": [{"words": text}]}));
    all.extend(run.until(WAIT, |record| {
        label_is("harness.efference")(record) && record["text"] == "2"
    }));
    run.event("harness.stop", &json!({}));
    all.extend(run.until(WAIT, label_is("harness.answer")));
    assert_eq!(texts(&all, "harness.efference"), strings(&["4", "2", "0"]));
    assert_eq!(texts(&all, "communication.stop"), strings(&["1"]));
    assert_eq!(
        texts(&all, "harness.answer"),
        strings(&[
            r#"bot.ability.speaking.output {"channels":1,"media_type":"audio/pcm","outcome":"stopped","sample_rate_hz":24000,"spoken":"alpha bravo charlie","text":"alpha bravo charlie delta echo foxtrot!!","unspoken":" delta echo foxtrot!!","voice_id":null}"#
        ])
    );
    close(run, "a_stop_ends_the_response_with_what_was_played");
}

/// Lays out the communication with the repository's speaking and a speech
/// double that streams (decision D203): two chunks of 0.5 s at once, the
/// third and the output 3 s later.
fn communication_streaming() -> PathBuf {
    let harness = communication_speaking(&[], &[]);
    let abilities = harness.parent().expect("the harness is in abilities/");
    rewrite(
        &abilities.join("vocal/speech_double.scx"),
        &[("mode", "'stream'")],
    );
    harness
}

/// A streamed response plays chunk by chunk as the chunks arrive (decision
/// D203): the first sound starts on the first chunk, each chunk is its own
/// sound, the copy before each sound covers the audio received, and when
/// the received audio has played before the next chunk arrives speaking
/// waits for it, sending no copy until it comes.
#[test]
fn a_streamed_response_plays_each_chunk_as_it_arrives() {
    let harness = communication_streaming();
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = run.until(WAIT, label_is("harness.ready"));
    let text = "alpha bravo charlie delta";
    run.event("harness.r1", &json!({"parts": [{"words": text}]}));
    all.extend(run.until(WAIT, label_is("harness.answer")));
    assert_eq!(
        texts(&all, "harness.sound"),
        strings(&[
            "media/speaking-1.0.pcm",
            "media/speaking-1.1.pcm",
            "media/speaking-1.2.pcm"
        ])
    );
    // Chunk 0 (all that was received), chunk 1 queued behind it, then
    // nothing until chunk 2 arrives, and nothing left after it.
    assert_eq!(
        texts(&all, "harness.efference"),
        strings(&["0.5", "0.5", "0.5", "0"])
    );
    let order: Vec<String> = logs(&all)
        .into_iter()
        .filter(|(label, _)| ["harness.efference", "harness.sound"].contains(&label.as_str()))
        .map(|(label, text)| format!("{label} {text}"))
        .collect();
    assert_eq!(
        order,
        strings(&[
            "harness.efference 0.5",
            "harness.sound media/speaking-1.0.pcm",
            "harness.efference 0.5",
            "harness.sound media/speaking-1.1.pcm",
            "harness.efference 0.5",
            "harness.sound media/speaking-1.2.pcm",
            "harness.efference 0"
        ])
    );
    assert_eq!(
        texts(&all, "harness.answer"),
        strings(&[
            r#"bot.ability.speaking.output {"channels":1,"media_type":"audio/pcm","outcome":"completed","sample_rate_hz":24000,"spoken":"alpha bravo charlie delta","text":"alpha bravo charlie delta","unspoken":"","voice_id":null}"#
        ])
    );
    close(run, "a_streamed_response_plays_each_chunk_as_it_arrives");
}

/// A stop while the reply still streams (decision D203) stops the playout
/// and cancels the encoding: the chunk still to come never sounds, and
/// what was said is the share of the received audio that played (0.5 s of
/// the 1 s received: half the words).
#[test]
fn a_stop_mid_stream_says_what_played_and_cancels_the_rest() {
    let harness = communication_streaming();
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = run.until(WAIT, label_is("harness.ready"));
    let text = "alpha bravo charlie delta";
    run.event("harness.r1", &json!({"parts": [{"words": text}]}));
    all.extend(run.until(WAIT, |record| {
        label_is("harness.sound")(record) && record["text"] == "media/speaking-1.1.pcm"
    }));
    run.event("harness.stop", &json!({}));
    all.extend(run.until(WAIT, label_is("harness.answer")));
    assert_eq!(
        texts(&all, "harness.answer"),
        strings(&[
            r#"bot.ability.speaking.output {"channels":1,"media_type":"audio/pcm","outcome":"stopped","sample_rate_hz":24000,"spoken":"alpha bravo","text":"alpha bravo charlie delta","unspoken":" charlie delta","voice_id":null}"#
        ])
    );
    // Past the double's 3 s: the third chunk was never sent.
    std::thread::sleep(Duration::from_secs(4));
    let finished = close(
        run,
        "a_stop_mid_stream_says_what_played_and_cancels_the_rest",
    );
    assert_eq!(
        texts(&finished.all, "harness.sound"),
        strings(&["media/speaking-1.0.pcm", "media/speaking-1.1.pcm"])
    );
    assert_eq!(
        texts(&finished.all, "harness.efference"),
        strings(&["0.5", "0.5", "0"])
    );
    assert!(
        texts(&finished.all, "speech.encode").len() == 1,
        "one encoding, cancelled: {:?}",
        texts(&finished.all, "speech.encode")
    );
}

/// Speaking's progress re-arms the response's hop timer (decision D179): a
/// 4 s reply played in 0.5 s pieces is said whole under a 1.5 s hop.
#[test]
fn speaking_progress_keeps_a_long_response_from_timing_out() {
    let harness = communication_speaking(&[("response_timeout_ms", "1500")], &[]);
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = run.until(WAIT, label_is("harness.ready"));
    let text = "alpha bravo charlie delta echo foxtrot!!";
    run.event("harness.r1", &json!({"parts": [{"words": text}]}));
    all.extend(run.until(WAIT, label_is("harness.answer")));
    assert!(
        texts(&all, "communication.hop.timed_out").is_empty(),
        "{all:?}"
    );
    assert_eq!(texts(&all, "harness.efference").len(), 9);
    assert_eq!(
        texts(&all, "harness.answer"),
        strings(&[
            r#"bot.ability.speaking.output {"channels":1,"media_type":"audio/pcm","outcome":"completed","sample_rate_hz":24000,"spoken":"alpha bravo charlie delta echo foxtrot!!","text":"alpha bravo charlie delta echo foxtrot!!","unspoken":"","voice_id":null}"#
        ])
    );
    close(
        run,
        "speaking_progress_keeps_a_long_response_from_timing_out",
    );
}

/// One utterance at a time: another copy of the one in hand, or of one
/// already said, is dropped, and past the bound of held utterances a new
/// one is refused (decision D175).
#[test]
fn speaking_never_reopens_a_finished_utterance_and_bounds_the_ones_it_holds() {
    let dir = unique_scratch("speaking-alone");
    let abilities = dir.join("abilities");
    let speaking = support::copy_ability("speaking", &abilities);
    support::double_speech(&abilities, "'silent'");
    rewrite(
        &speaking,
        &[
            ("encoding_timeout_ms", "2000"),
            ("held_utterances_bound", "1"),
        ],
    );
    let harness = abilities.join("harness.scx");
    std::fs::write(&harness, SPEAKING_HARNESS).expect("the harness is writable");
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let mut all = run.until(WAIT, label_is("harness.ready"));
    run.event("harness.u1", &json!({"parts": [{"words": "one"}]}));
    all.extend(run.until(WAIT, label_is("speech.encode")));
    run.event("harness.u1", &json!({"parts": [{"words": "one again"}]}));
    all.extend(run.until(WAIT, label_is("speaking.late")));
    run.event("harness.u2", &json!({"parts": [{"words": "two"}]}));
    all.extend(run.until(WAIT, label_is("speaking.deferred")));
    run.event("harness.u3", &json!({"parts": [{"words": "three"}]}));
    all.extend(run.until(WAIT, label_is("harness.answer")));
    // u1's encoding times out; the held u2 is encoded and times out too.
    all.extend(run.until(WAIT, |record| {
        label_is("speaking.timed_out")(record) && record["text"] == "encoding 2 after 2000 ms"
    }));
    run.event("harness.u1", &json!({"parts": [{"words": "one late"}]}));
    all.extend(run.until(WAIT, label_is("speaking.late")));
    let finished = run.close(WAIT);
    trace::assert_trace(
        "speaking_never_reopens_a_finished_utterance_and_bounds_the_ones_it_holds",
        &finished.all,
        &[],
    );
    all.extend(finished.records);
    assert_eq!(
        texts(&all, "speech.encode"),
        strings(&[
            "encode one -> media/speaking-1.pcm",
            "encode two -> media/speaking-2.pcm",
        ])
    );
    assert_eq!(texts(&all, "speaking.late"), strings(&["u1", "u1"]));
    assert_eq!(
        texts(&all, "harness.answer"),
        strings(&[
            r#"bot.ability.failed {"message":"Speaking is holding too many utterances; this one is refused."}"#,
            r#"bot.ability.failed {"message":"Speech encoding timed out."}"#,
            r#"bot.ability.failed {"message":"Speech encoding timed out."}"#,
        ])
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
    // The body's transition carries the input's schema, so scx
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
    let run = Run::start(&[support::environment_for(&body, &[])], &[]);
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
/// [`the_conversation_commits_each_message_into_its_exchange`]: each
/// relationship's context reference, the same in either direction.
const FIRST: &str = r#"relationship-{"identity_groups":[["a\"b","bot"],["alice","zed",[0.447213595499958,0.894427190999916]]]}"#;
const AMBIENT: &str = r#"relationship-{"identity_groups":[[],["alice"]]}"#;
const THIRD: &str =
    r#"relationship-{"identity_groups":[["bob",[0.847998304005088,0.52999894000318]],["bot"]]}"#;

/// The events of [`the_conversation_commits_each_message_into_its_exchange`]:
/// unsorted string and embedding identities, an escaped quote, audio in
/// mixed case (dropped), structured content (kept), outbound references,
/// the same provenance id from another source (committed) and from the
/// same one (not committed again, still answered), a record without
/// content or id, the first participants again, then in the other
/// direction, another targetless source (the ambient relationship), and
/// input addressed to an ambient participant.
fn conversation_exchange_events() -> [(String, Json); 11] {
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
            &json!({"message": {"direction": "outbound", "source_ids": ["bot"], "target_ids": ["zed", "alice"], "content": "hi", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "r-1", "session_ref": "s1", "turn_ref": "t1", "source": "q"}}}),
        ),
        conversation_event(
            append,
            &json!({"message": {"direction": "outbound", "source_ids": ["bot"], "target_ids": [], "content": "again", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "r-1"}}}),
        ),
        conversation_event(
            append,
            &json!({"message": {"direction": "outbound", "source_ids": ["bot"], "target_ids": [], "content": "once", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "r-1", "source": "q"}}}),
        ),
        conversation_event(
            append,
            &json!({"message": {"direction": "outbound", "source_ids": [], "target_ids": [], "content_type": "text/plain", "provenance": {"event": append, "id": null}}}),
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

/// The snapshot after [`the_conversation_commits_each_message_into_its_exchange`]:
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
fn the_conversation_commits_each_message_into_its_exchange() {
    let dir = unique_scratch("conversation");
    support::copy_ability("conversation", &dir.join("abilities"));
    let harness = dir.join("abilities/harness.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/conversation_harness.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    let events = conversation_exchange_events();
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
    // A voice embedding is never logged (`<biometric:VoiceEmbedding>`).
    assert_eq!(
        texts(&all, "conversation"),
        strings(&[
            r#"inbound from ["alice","zed","<biometric:VoiceEmbedding>"] in relationship 0"#,
            r#"inbound from ["alice"] in relationship 1"#,
            r#"inbound from ["bob","<biometric:VoiceEmbedding>"] in relationship 2"#,
            "outbound 1",
            "outbound 2",
            "outbound r-1 already committed",
            "outbound 3",
            r#"inbound from ["alice","zed","<biometric:VoiceEmbedding>"] in relationship 0"#,
            r#"inbound from ["a\"b","bot"] in relationship 0"#,
            r#"inbound from ["bob"] in relationship 1"#,
            r#"inbound from ["carol"] in relationship 1"#,
        ])
    );
    assert_snapshot(&all, FIRST, AMBIENT, THIRD);
    let finished = run.close(WAIT);
    let outputs: Vec<Json> = texts(&finished.all, "harness.output")
        .iter()
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .collect();
    assert_eq!(outputs.len(), 11, "{outputs:?}");
    // Each inbound commit opens a new exchange of that one message; each
    // append adds to the exchange in hand.
    for (output, session_ref) in [
        (0, FIRST),
        (1, AMBIENT),
        (2, THIRD),
        (7, FIRST),
        (8, FIRST),
        (9, AMBIENT),
        (10, AMBIENT),
    ] {
        let messages = &outputs[output]["messages"];
        assert_eq!(messages.as_array().map(Vec::len), Some(1), "{output}");
        assert_eq!(
            messages[0]["provenance"]["session_ref"],
            json!(session_ref),
            "{output}"
        );
    }
    let first = &outputs[0]["messages"][0];
    let second = &outputs[1]["messages"][0];
    let third = &outputs[2]["messages"][0];
    assert_eq!(second["content"], Json::Null);
    // Each source's turn (in identity order: names, then embeddings) is
    // exchanged with its turn detector, and the turns'
    // texts are committed joined, each distinct text once (D79).
    assert_eq!(first["content"], json!("hello"));
    assert_eq!(first["provenance"]["turn_ref"], json!("input-1:turn:alice"));
    let second_turn = &second["provenance"]["turn_ref"];
    assert_eq!(second_turn, &json!("input-2:turn:alice"));
    assert_eq!(third["content"], json!({"b": 1, "a": [true, null]}));
    assert_eq!(third["provenance"]["turn_ref"], json!("input-3:turn:bob"));
    // An append's provenance keeps its id, source and target.
    let appended = &outputs[4]["messages"];
    assert_eq!(appended.as_array().map(Vec::len), Some(3));
    assert_eq!(
        (
            &appended[1]["provenance"]["source"],
            &appended[2]["provenance"]["source"]
        ),
        (&json!("q"), &Json::Null)
    );
    assert_eq!(outputs[7]["messages"][0]["content"], json!("again"));
    assert_eq!(
        outputs[8]["messages"][0]["provenance"]["turn_ref"],
        json!("input-5:turn:a\"b")
    );
    // The duplicate is answered with the exchange it did not change.
    assert_eq!(outputs[5]["messages"].as_array().map(Vec::len), Some(3));
    trace::assert_trace(
        "the_conversation_commits_each_message_into_its_exchange",
        &finished.all,
        &[],
    );
}

/// The remembered input is one JSON text as `<memory:to_json>` writes it
/// (the memory namespace's statement memory-2): sorted keys, lists and
/// mappings tagged, identities sorted.
#[test]
fn the_remembered_input_is_one_memory_json_text() {
    let dir = unique_scratch("conversation-payload");
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
            r#"{"content":{"__stateforward_memory_content__":"mapping","value":[["a",{"__stateforward_memory_content__":"list","value":[true,null,1.5]}],["b",1],["é","x"]]},"content_type":"application/json","source_ids":["alice","zed",[0.5,1]],"target_ids":["bot","é"]}"#,
            "{\"content\":{\"__stateforward_memory_content__\":\"list\",\"value\":[\"\u{1f600}\",1e-7,12345678901234567000,{\"__stateforward_memory_content__\":\"mapping\",\"value\":[[\"z\",{\"__stateforward_memory_content__\":\"mapping\",\"value\":[]}],[\"\u{7f}\",{\"__stateforward_memory_content__\":\"list\",\"value\":[]}]]}]},\"content_type\":\"text/x\",\"source_ids\":[\"q\\\"\\n\",[0.00001,2],[3,4]],\"target_ids\":[]}",
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
    // Sent in one macrostep, so the second and third wait for the first.
    // White space only: the detector closes the turn with
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
    let mut records = until_all(&run, &["input.sound", "bot.ability.speaking.output"]);
    until_silent_copy(&run, &mut records);
    // "reply 1" is 7 characters: the double measures 0.7 s, played as two
    // pieces (0.5 s and 0.2 s), each report followed by a copy of what is
    // still to come.
    assert_eq!(
        texts(&records, "input.efference"),
        strings(&[
            r#"{"channels":1,"duration":0.7,"media_type":"audio/pcm","mouth":"speaking","sample_rate_hz":24000}"#,
            r#"{"channels":1,"duration":0.2,"media_type":"audio/pcm","mouth":"speaking","sample_rate_hz":24000}"#,
            r#"{"channels":1,"duration":0,"media_type":"audio/pcm","mouth":"speaking","sample_rate_hz":24000}"#
        ])
    );
    assert_eq!(
        texts(&records, "input.sound"),
        strings(&[
            r#"{"audio":"media/speaking-1.0.pcm","channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"source":"speaking"}"#
        ])
    );
    let order: Vec<String> = logs(&records)
        .into_iter()
        .map(|(label, _)| label)
        .filter(|label| ["input.efference", "input.sound"].contains(&label.as_str()))
        .collect();
    assert_eq!(
        order,
        strings(&[
            "input.efference",
            "input.sound",
            "input.efference",
            "input.efference"
        ])
    );
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
            r#"{"channels":1,"media_type":"audio/pcm","outcome":"completed","sample_rate_hz":24000,"spoken":"reply 1","text":"reply 1","unspoken":"","voice_id":null}"#
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

/// A fake OpenAI-compatible speech backend on 127.0.0.1: every
/// `POST /v1/audio/speech` answers, after `latency`, a tenth of a second
/// of raw 24 kHz mono 16-bit PCM (4800 bytes), and its request bodies are
/// recorded. The answer comes once the whole clip is synthesized, so
/// `latency` stands for a sentence's synthesis time.
fn speech_backend(latency: Duration) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Json>>>) {
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
            std::thread::sleep(latency);
            let answer = vec![0_u8; 4800];
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: audio/pcm\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                answer.len()
            );
            let _ = stream.write_all(&answer);
        }
    });
    (origin, requests)
}

/// The bot with the real vocal ability (which invokes the speech service)
/// and `with_listening`'s input double, started against
/// [`speech_backend`] with `latency` and activated; returns the run, the
/// backend's request log, and the media directory the run grants the
/// speech service.
fn spoken_through_the_speech_service(
    latency: Duration,
) -> (Run, std::sync::Arc<std::sync::Mutex<Vec<Json>>>, PathBuf) {
    let body = layout(&Speaking::Real, &[]);
    with_listening(&body);
    let abilities = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the body is under bots/floppy/")
        .join("abilities");
    std::fs::copy(
        workspace().join("abilities/vocal/vocal.scx"),
        abilities.join("vocal/vocal.scx"),
    )
    .expect("the vocal ability is copyable");
    std::fs::remove_file(abilities.join("vocal/speech_double.scx"))
        .expect("the double is removable");
    let (origin, requests) = speech_backend(latency);
    let media = unique_scratch("speaking-media");
    let env = media.join("speech.env");
    std::fs::write(
        &env,
        format!(
            "FLOPPY_SPEECH_BASE_URL={origin}/v1\nFLOPPY_SPEECH_API_KEY=sk-floppy-speech-fake\n"
        ),
    )
    .expect("the env file is writable");
    let run = Run::start(
        &[
            "--env-file".to_owned(),
            env.display().to_string(),
            "--dir".to_owned(),
            format!(
                "{}={}::/media",
                support::service_uri("speech"),
                media.display()
            ),
            support::environment_for(&body, &[]),
        ],
        &[],
    );
    run.until(WAIT, label_is("bot.activated"));
    (run, requests, media)
}

#[test]
fn a_response_is_spoken_through_the_speech_service() {
    let (mut run, requests, media) = spoken_through_the_speech_service(Duration::ZERO);
    run.event("bot.input", &input("reply"));
    let mut records = until_all(&run, &["input.sound", "bot.ability.speaking.output"]);
    until_silent_copy(&run, &mut records);
    // 4800 bytes of 24 kHz mono 16-bit PCM: a tenth of a second, measured,
    // and played as one piece.
    assert_eq!(
        texts(&records, "input.efference"),
        strings(&[
            r#"{"channels":1,"duration":0.1,"media_type":"audio/pcm","mouth":"speaking","sample_rate_hz":24000}"#,
            r#"{"channels":1,"duration":0,"media_type":"audio/pcm","mouth":"speaking","sample_rate_hz":24000}"#
        ])
    );
    // The sound's source is the copy's mouth: the listening's sensitivity
    // scores it as the bot's own.
    assert_eq!(
        texts(&records, "input.sound"),
        strings(&[
            r#"{"audio":"media/speaking-1.0.pcm","channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"source":"speaking"}"#
        ])
    );
    assert_eq!(texts(&records, "floppy.say"), strings(&["reply 1"]));
    let written = std::fs::read(media.join("speaking-1.pcm")).expect("the audio was written");
    assert_eq!(written.len(), 4800);
    let requests = requests.lock().expect("the request log").clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    // speaking's defaults: the OpenAI-compatible speech API with the lab's
    // Breeze engine and its warm voice.
    assert_eq!(requests[0]["model"], "sidekick-tts-engine-breeze");
    assert_eq!(requests[0]["input"], "reply 1");
    assert_eq!(requests[0]["voice"], "warm");
    assert_eq!(requests[0]["response_format"], "pcm");
    close(run, "a_response_is_spoken_through_the_speech_service");
}

/// The 2026-10-12 live run (docs/live-proof.md): the first reply, a
/// sentence, failed with "Speech encoding timed out." because Gemini's
/// unstreamed synthesis outlasted the 5 s bound. With the shipped bounds
/// (speaking's encoding through the communication, and the communication's
/// response hop around it) a reply whose synthesis takes 8 s, as a
/// sentence's can, is spoken.
#[test]
fn a_reply_whose_synthesis_takes_seconds_is_spoken_with_the_default_bounds() {
    let (mut run, requests, _media) = spoken_through_the_speech_service(Duration::from_secs(8));
    run.event("bot.input", &input("reply"));
    let records = run.until(WAIT, |record| {
        label_is("bot.ability.speaking.output")(record)
            || label_is("bot.ability.communication.failed")(record)
    });
    assert!(
        texts(&records, "bot.ability.communication.failed").is_empty(),
        "{:?}",
        texts(&records, "bot.ability.communication.failed")
    );
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[
            r#"{"channels":1,"media_type":"audio/pcm","outcome":"completed","sample_rate_hz":24000,"spoken":"reply 1","text":"reply 1","unspoken":"","voice_id":null}"#
        ])
    );
    assert_eq!(requests.lock().expect("the request log").len(), 1);
    close(
        run,
        "a_reply_whose_synthesis_takes_seconds_is_spoken_with_the_default_bounds",
    );
}
