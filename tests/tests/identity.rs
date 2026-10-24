//! The floppy bot's identity (`abilities/identity/identity.scx`), run as the body's
//! child through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! under `bots/floppy/`, the communication, conversation, speaking,
//! identity, persona, memory, the short-term register and every schema
//! under `abilities/`) with
//! `tests/fixtures/phase4/cognition_double.scx` as `abilities/cognition/cognition.scx`
//! (no model; an addressed turn answers "I am <name>", a `bot.input`
//! observation `adopt` selects `bot.ability.identity.adopt` for its data).
//! A bot is born nameless; a test that needs a name rewrites the identity's
//! `<data id="name">`, which stands in for the invoker's `<param>`.
//!
//! The identity logs `identity.name`, `identity.heard` and
//! `identity.adopt.failed`.
//!
//! The persona (`abilities/persona/persona.scx`), the bot's other sense of
//! itself, is tested here too: it logs `persona.description`, and the body relays its `bot.ability.persona.output`
//! to the cognition, whose double a persona test rewrites to log it as
//! `double.persona` (and to select a revision for the persona).
//!
//! The short-term register's wiring through the body (a child's turn
//! relayed to it, its recent turns read before each turn and relayed to
//! the cognition) is tested here too, with a rewritten double.
//!
//! The body gives the identity its database, in a directory each run is
//! granted (`support::memory_grant`), so an adopted name outlives the run.
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
use support::{Run, hearing, logs, trace, unique_scratch, workspace};

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(300);

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

/// Lays out the bot with the cognition double; `name` (an ECMAScript
/// expression) replaces the identity's starting name. Returns the body.
fn layout(name: Option<&str>) -> PathBuf {
    let dir = unique_scratch("identity");
    copy_dir(&workspace().join("bots/floppy"), &dir.join("bots/floppy"));
    copy_dir(
        &workspace().join("bots/floppy/firmware"),
        &dir.join("bots/floppy/firmware"),
    );
    copy_dir(
        &workspace().join("bots/floppy/schemas"),
        &dir.join("bots/floppy/schemas"),
    );
    for name in [
        "communication",
        "conversation",
        "speaking",
        "identity",
        "memory",
        "stm",
        "persona",
        "seeing",
        "speech_decoding",
    ] {
        support::copy_ability(name, &dir.join("abilities"));
    }
    support::copy_listening(&dir.join("abilities"));
    support::copy_cognition_double(&dir.join("abilities"));
    support::double_speech(&dir.join("abilities"), "'encode'");
    if let Some(name) = name {
        let path = dir.join("abilities/identity/identity.scx");
        let text = std::fs::read_to_string(&path).expect("the identity is readable");
        let named = text.replacen(
            r#"<data id="name" expr="null"/>"#,
            &format!(r#"<data id="name" expr="{name}"/>"#),
            1,
        );
        assert_ne!(named, text, "the identity declares <data id=\"name\">");
        std::fs::write(&path, named).expect("the identity copy is writable");
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

/// Starts the body and activates it; returns the run and every record up
/// to the activation.
fn started(body: &Path) -> (Run, Vec<Json>) {
    started_with(body, &[])
}

/// [`started`] with `grant` (the memory service's directory) before the
/// body; an empty `grant` gets a fresh directory.
fn started_with(body: &Path, grant: &[String]) -> (Run, Vec<Json>) {
    let mut args = grant.to_vec();
    args.push(support::environment_for(body, &[]));
    let run = Run::start(&args, &[]);
    let records = run.until(WAIT, label_is("bot.activated"));
    (run, records)
}

fn said(text: &str) -> Json {
    json!({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": text, "content_type": "text/plain"})
}

fn input(action: &str, data: &str) -> Json {
    json!({"observation": {"event": action, "data": data}})
}

/// Closes the run and asserts its pinned trace; returns every record.
fn close(run: Run, test: &str) -> Vec<Json> {
    let finished = run.close(WAIT);
    trace::assert_trace(test, &finished.all, &[]);
    finished.all
}

#[test]
fn the_identity_is_ready_with_its_adopt_offer() {
    let (mut run, records) = started(&layout(None));
    // The identity's first offers are part of the activation; the body
    // gives it a database, so it recalls from memory.
    assert!(texts(&records, "memory").is_empty(), "{records:?}");
    assert_eq!(
        texts(&records, "identity.name"),
        strings(&["recalled null"])
    );
    // What it offers, the body reads from its live session each turn
    // (decision D242): adopt, under the identity's session id.
    run.event("bot.input", &input("ignore", "hello"));
    let records = run.until(WAIT, label_is("double.choices"));
    let choices = texts(&records, "double.choices");
    assert_eq!(choices.len(), 1, "{records:?}");
    assert!(
        support::choice_names(&choices[0])
            .contains("bot.ability.identity.adopt@session.floppy.identity"),
        "{choices:?}"
    );
    close(run, "the_identity_is_ready_with_its_adopt_offer");
}

#[test]
fn a_named_bot_is_addressed_by_its_name() {
    let (mut run, _) = started(&layout(Some("'Floppy'")));
    // The conversation's product is a turn too (the double ignores it:
    // the text's first word); the addressed turn answers.
    run.event(
        "bot.ability.communication.input",
        &said("ignore Floppy, what's your name?"),
    );
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(
        texts(&records, "identity.heard"),
        strings(&["addressed as Floppy"])
    );
    assert_eq!(texts(&records, "floppy.say"), strings(&["I am Floppy"]));
    let all = close(run, "a_named_bot_is_addressed_by_its_name");
    let mut turns: Vec<String> = texts(&all, "double.input")
        .into_iter()
        .map(|turn| {
            turn.split_once(' ')
                .map(|(_, rest)| rest.to_owned())
                .unwrap_or(turn)
        })
        .collect();
    turns.sort();
    assert_eq!(turns, strings(&[r#"addressed "Floppy""#, "ignore 1"]));
}

/// Name recognition in speech: a sound is decoded and the transcript
/// searched for the name. Only the identity hears the sound here (the
/// body's input list without the listening), so the one turn is the
/// addressed one.
#[test]
fn a_named_bot_hears_its_name_in_a_sound() {
    let test = "a_named_bot_hears_its_name_in_a_sound";
    let body = layout(Some("'Floppy'"));
    let document = std::fs::read_to_string(&body).expect("the body is readable");
    let inputs = r#"<data id="input" expr="['identity', 'listening']"/>"#;
    assert!(document.contains(inputs), "the body's input abilities");
    std::fs::write(
        &body,
        document.replacen(inputs, r#"<data id="input" expr="['identity']"/>"#, 1),
    )
    .expect("the body copy is writable");
    let backend = hearing::Backend::start();
    let granted = hearing::granted(
        test,
        &backend,
        &[("hey.wav", hearing::spoken_wav("hey floppy"))],
    );
    let (mut run, _) = started_with(&body, &granted.args);
    run.event(
        "environment.sound",
        &json!({"audio": "media/hey.wav", "media_type": "audio/wav", "sample_rate_hz": 16000, "channels": 1}),
    );
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(
        texts(&records, "identity.heard"),
        strings(&["addressed as Floppy"])
    );
    assert_eq!(texts(&records, "floppy.say"), strings(&["I am Floppy"]));
    // The bot hears its own playout too (every input ability hears the
    // environment); the speech double writes no audio, so decoding
    // it fails, and the identity says so. Playout lasts as long as
    // the audio (D179), so that can come before speaking's answer.
    if !records.iter().any(label_is("identity.recognition.failed")) {
        run.until(WAIT, label_is("identity.recognition.failed"));
    }
    assert_eq!(backend.decoded(), strings(&["hey floppy"]));
    close(run, test);
}

#[test]
fn a_name_is_heard_only_as_a_whole_word() {
    let (mut run, _) = started(&layout(Some("'Al'")));
    run.event("bot.ability.communication.input", &said("ignore always"));
    let records = run.until(WAIT, label_is("identity.heard"));
    assert_eq!(
        texts(&records, "identity.heard"),
        strings(&["not addressed"])
    );
    let all = close(run, "a_name_is_heard_only_as_a_whole_word");
    assert!(
        texts(&all, "double.input")
            .iter()
            .all(|turn| !turn.contains("addressed")),
        "{all:?}"
    );
}

#[test]
fn a_nameless_bot_only_hears_words() {
    let (mut run, _) = started(&layout(None));
    run.event(
        "bot.ability.communication.input",
        &said("ignore Floppy, hello"),
    );
    let records = run.until(WAIT, label_is("communication.product"));
    let all = close(run, "a_nameless_bot_only_hears_words");
    let mut every = records;
    every.extend(all);
    assert!(texts(&every, "identity.heard").is_empty(), "{every:?}");
}

#[test]
fn an_adopted_name_is_answered_to() {
    let (mut run, _) = started(&layout(None));
    run.event("bot.input", &input("adopt", "Floppy"));
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "identity.name"
            && record["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("adopted"))
    });
    assert_eq!(
        texts(&records, "identity.name"),
        strings(&[r#"adopted "Floppy""#])
    );
    // Back in knowing, it still offers adopt, and now answers to the name.
    run.event(
        "bot.ability.communication.input",
        &said("ignore hey floppy"),
    );
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(
        texts(&records, "identity.heard"),
        strings(&["addressed as Floppy"])
    );
    assert_eq!(texts(&records, "floppy.say"), strings(&["I am Floppy"]));
    let all = close(run, "an_adopted_name_is_answered_to");
    assert!(
        texts(&all, "double.choices")
            .iter()
            .all(|text| support::choice_names(text).contains("bot.ability.identity.adopt@")),
        "{all:?}"
    );
}

/// Adopts "Floppy", then "Pip", in one run and wakes the next run with
/// `grant` before the body; returns that run and its records up to the
/// activation.
fn adopted_then_recalled(body: &Path, grant: &[String]) -> (Run, Vec<Json>) {
    let (mut run, _) = started_with(body, grant);
    run.event("bot.input", &input("adopt", "Floppy"));
    run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "identity.name"
            && record["text"] == r#"adopted "Floppy""#
    });
    run.event("bot.input", &input("adopt", "Pip"));
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "identity.name"
            && record["text"] == r#"adopted "Pip""#
    });
    assert!(texts(&records, "identity.adopt.failed").is_empty());
    drop(run.close(WAIT));
    // The next run wakes up with the name it took last.
    let (run, records) = started_with(body, grant);
    assert_eq!(
        texts(&records, "identity.name"),
        strings(&[r#"recalled "Pip""#])
    );
    (run, records)
}

#[test]
fn an_adopted_name_is_recalled_by_the_next_run() {
    let body = layout(None);
    let (run, _) = adopted_then_recalled(&body, &support::memory_grant());
    close(run, "an_adopted_name_is_recalled_by_the_next_run");
}

/// The same on the Postgres engine: the body's database is a `PGlite`
/// server's connection URI.
#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_an_adopted_name_is_recalled_by_the_next_run() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    let body = layout(None);
    support::use_database(&body, &server.database());
    let (run, records) = adopted_then_recalled(&body, &support::pgsql_grant());
    assert!(texts(&records, "memory").is_empty(), "{records:?}");
    drop(run.close(WAIT));
}

/// The cognition asks for the bot's own name as it starts, while the bot
/// is activating; the body relays the ask to the identity, which answers
/// once it has recalled, and relays every `bot.ability.identity.named`
/// (each adoption's too) back. The double is
/// rewritten to ask on start and log each answer as `double.named`.
#[test]
fn the_cognition_is_told_the_bots_name_and_each_adoption() {
    let body = layout(None);
    let double = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the layout's root")
        .join("abilities/cognition/cognition.scx");
    let text = std::fs::read_to_string(&double).expect("the double is readable");
    let alive = r#"<state id="alive" initial="idle">"#;
    assert!(text.contains(alive), "the double's alive state");
    std::fs::write(
        &double,
        text.replacen(
            alive,
            &format!(
                r##"{alive}
    <onentry>
      <send event="bot.ability.identity.ask" target="#_parent"><content expr="({{}})"/></send>
    </onentry>
    <transition event="bot.ability.identity.named">
      <log label="double.named" expr="JSON.stringify(_event.data.name)"/>
    </transition>"##
            ),
            1,
        ),
    )
    .expect("the double copy is writable");
    // The answer may come before or after the activation.
    let (mut run, mut records) = started(&body);
    if texts(&records, "double.named").is_empty() {
        records.extend(run.until(WAIT, label_is("double.named")));
    }
    assert_eq!(texts(&records, "double.named"), strings(&["null"]));
    run.event("bot.input", &input("adopt", "Floppy"));
    let records = run.until(WAIT, label_is("double.named"));
    assert_eq!(texts(&records, "double.named"), strings(&[r#""Floppy""#]));
    close(run, "the_cognition_is_told_the_bots_name_and_each_adoption");
}

/// The short-term register through the body: a child's
/// `bot.ability.memory.stm.turn` is relayed to the register, and before a
/// turn the body reads the register's recent turns and relays them to the
/// cognition ahead of the turn's input. The double is rewritten to write one
/// heard turn on start and log each relayed read as `double.recent` (the
/// turns' texts).
#[test]
fn the_cognition_is_told_the_recent_turns_before_each_turn() {
    let body = layout(None);
    let double = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the layout's root")
        .join("abilities/cognition/cognition.scx");
    let text = std::fs::read_to_string(&double).expect("the double is readable");
    let alive = r#"<state id="alive" initial="idle">"#;
    assert!(text.contains(alive), "the double's alive state");
    std::fs::write(
        &double,
        text.replacen(
            alive,
            &format!(
                r##"{alive}
    <onentry>
      <send event="bot.ability.memory.stm.turn" target="#_parent">
        <content expr="({{id: 't1', direction: 'heard', speakers: [], text: 'Hi there, I am Maya.', cut: false, conversation: 'listening'}})"/>
      </send>
    </onentry>
    <transition event="bot.ability.memory.stm.recent_turns.output">
      <log label="double.recent" expr="JSON.stringify(_event.data.turns.map(function (t) {{ return t.text; }}))"/>
    </transition>"##
            ),
            1,
        ),
    )
    .expect("the double copy is writable");
    let (mut run, _) = started(&body);
    run.event("bot.input", &input("ignore", "nothing"));
    let records = run.until(WAIT, label_is("double.choices"));
    assert_eq!(
        texts(&records, "double.recent"),
        strings(&[r#"["Hi there, I am Maya."]"#])
    );
    close(
        run,
        "the_cognition_is_told_the_recent_turns_before_each_turn",
    );
}

/// The body's default traits (`<data id="traits">` in `bots/floppy/floppy.scx`).
const TRAITS: [&str; 4] = ["warm", "patient", "curious", "honest"];

/// The revision the rewritten double selects.
const REVISED_TEXT: &str = "I like to ask questions.";
const REVISED_REASON: &str = "I noticed I keep asking them.";

/// Rewrites the layout's cognition double to log each
/// `bot.ability.persona.output` the body relays as `double.persona` (its
/// data as JSON) and, with `revise`, to select a revision for the persona
/// when it is told the bot has no description yet: it sends
/// `bot.ability.persona.revise` to the persona's session (decision D110:
/// `<body _sessionid>.persona`, its own with `cognition` replaced), as
/// reflection dispatches its selection to the session that offers it.
fn persona_double(body: &Path, revise: bool) {
    let double = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the layout's root")
        .join("abilities/cognition/cognition.scx");
    let text = std::fs::read_to_string(&double).expect("the double is readable");
    let alive = r#"<state id="alive" initial="idle">"#;
    assert!(text.contains(alive), "the double's alive state");
    std::fs::write(
        &double,
        text.replacen(
            alive,
            &format!(
                r#"{alive}
    <transition event="bot.ability.persona.output">
      <log label="double.persona" expr="JSON.stringify(_event.data)"/>
      <if cond="{revise}">
        <if cond="_event.data.description === null">
          <send event="bot.ability.persona.revise" targetexpr="'#_scxml_' + _sessionid.replace(/cognition$/, 'persona')">
            <content expr="({{text: '{REVISED_TEXT}', reason: '{REVISED_REASON}'}})"/>
          </send>
        </if>
      </if>
    </transition>"#
            ),
            1,
        ),
    )
    .expect("the double copy is writable");
}

/// `bot.ability.persona.output` with the body's traits and `description`.
fn told(description: Option<&str>) -> Json {
    let description = description.map_or(
        Json::Null,
        |text| json!({"text": text, "reason": REVISED_REASON, "traits": TRAITS, "kind": "fact"}),
    );
    json!({"traits": TRAITS, "description": description})
}

/// The `double.persona` records among `records`, parsed.
fn persona_told(records: &[Json]) -> Vec<Json> {
    texts(records, "double.persona")
        .iter()
        .map(|text| serde_json::from_str(text).expect("the double logs JSON"))
        .collect()
}

/// Waits, if it has not come yet, for the `double.persona` the persona
/// tells on waking (before or after the activation); returns every record.
fn woken(run: &Run, mut records: Vec<Json>) -> Vec<Json> {
    if texts(&records, "double.persona").is_empty() {
        records.extend(run.until(WAIT, label_is("double.persona")));
    }
    records
}

/// The persona wakes with the owner's traits and no description, tells the
/// cognition so through the body, and is ready with its revise offer,
/// which the body keeps out of the turn's choices (only reflection may
/// select a revision).
#[test]
fn the_persona_tells_the_cognition_its_traits_and_no_description() {
    let body = layout(None);
    persona_double(&body, false);
    let (mut run, records) = started(&body);
    let records = woken(&run, records);
    assert_eq!(
        texts(&records, "persona.description"),
        strings(&["recalled null"])
    );
    assert_eq!(persona_told(&records), vec![told(None)]);
    // Its revise is never read into a turn's choices (decision D242).
    run.event("bot.input", &input("ignore", "hello"));
    let records = run.until(WAIT, label_is("double.choices"));
    let choices = texts(&records, "double.choices");
    assert_eq!(choices.len(), 1, "{records:?}");
    assert!(
        !support::choice_names(&choices[0]).contains("bot.ability.persona.revise"),
        "{choices:?}"
    );
    close(
        run,
        "the_persona_tells_the_cognition_its_traits_and_no_description",
    );
}

/// A revision the bot selects is written down before it is the bot's,
/// told to the cognition with the traits it was written under, and
/// recalled by the next run, which tells it on waking.
#[test]
fn a_revised_description_is_told_and_recalled_by_the_next_run() {
    let body = layout(None);
    persona_double(&body, true);
    let grant = support::memory_grant();
    let (run, records) = started_with(&body, &grant);
    let mut records = woken(&run, records);
    if persona_told(&records).len() < 2 {
        records.extend(run.until(WAIT, |record| {
            record["type"] == "log"
                && record["label"] == "double.persona"
                && record["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(REVISED_TEXT))
        }));
    }
    assert_eq!(
        persona_told(&records),
        vec![told(None), told(Some(REVISED_TEXT))]
    );
    assert_eq!(
        texts(&records, "persona.description"),
        strings(&["recalled null", &format!("revised {REVISED_TEXT:?}")])
    );
    assert!(texts(&records, "persona.revise.failed").is_empty());
    drop(run.close(WAIT));
    // The next run wakes up with the description it wrote last.
    let (run, records) = started_with(&body, &grant);
    let records = woken(&run, records);
    assert_eq!(
        texts(&records, "persona.description"),
        strings(&[&format!("recalled {REVISED_TEXT:?}")])
    );
    assert_eq!(persona_told(&records), vec![told(Some(REVISED_TEXT))]);
    close(
        run,
        "a_revised_description_is_told_and_recalled_by_the_next_run",
    );
}
