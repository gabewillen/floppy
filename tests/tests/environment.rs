//! The environment (docs/plans/environment.mdscript.md): an SCXML document
//! run as the root session whose invoked children are its participants and
//! its devices, and the `device` namespace a bot reads its devices with
//! (decisions D102 to D110).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Run, copy_tree, logs, unique_scratch, workspace, write};

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(300);

/// Lays out a scratch copy of the repository's shape with the floppy bot
/// (its `<data>` expressions replaced per `data`, `(id, expr)`), its
/// abilities, the speech double, and
/// `tests/fixtures/phase4/cognition_double.scx` as its cognition, and
/// returns the layout's root.
fn layout(data: &[(&str, &str)]) -> PathBuf {
    let dir = unique_scratch("environment");
    let body_dir = dir.join("bots/floppy");
    copy_tree(&workspace().join("bots/floppy"), &body_dir);
    let body_path = body_dir.join("floppy.scx");
    let mut body = std::fs::read_to_string(&body_path).expect("the body is readable");
    for (id, expr) in data {
        let start = body
            .find(&format!(r#"<data id="{id}" expr=""#))
            .unwrap_or_else(|| panic!("the body declares <data id=\"{id}\">"));
        let end = start + body[start..].find("/>").expect("the <data> element closes");
        body.replace_range(start..end, &format!(r#"<data id="{id}" expr="{expr}""#));
    }
    std::fs::write(&body_path, body).expect("the body copy is writable");
    let abilities = dir.join("abilities");
    copy_tree(
        &workspace().join("abilities/schemas"),
        &abilities.join("schemas"),
    );
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
    support::double_speech(&abilities, "'encode'");
    std::fs::create_dir_all(abilities.join("cognition")).expect("creatable");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/cognition_double.scx"),
        abilities.join("cognition/cognition.scx"),
    )
    .expect("the cognition double is copyable");
    dir
}

/// Puts `tests/fixtures/phase4/input_double.scx` in the identity's place in
/// the layout at `dir` (laid out with `input` `['identity']`): it logs
/// `input.sound` and `input.visual` for each stimulus it receives.
fn double_the_identity(dir: &Path) {
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/input_double.scx"),
        dir.join("abilities/identity/identity.scx"),
    )
    .expect("the input double is copyable");
}

/// The fuel of a run with more than one bot: the command that enters the
/// room starts every bot and all of their abilities, about 20 sessions
/// each, on one host-call budget (scxml.rs docs/limits.md, "Fuel": a chain
/// of invoked sessions started by one command needs fuel for every level),
/// and two bots' starts exceed the default 3 billion.
const MULTI_BOT_FUEL: &str = "12000000000";

/// Starts the room laid into `dir` with one floppy bot per id in `bots`.
fn start(dir: &Path, bots: &[&str]) -> Run {
    let room = support::environment(dir, bots, &[]);
    let mut args = Vec::new();
    if bots.len() > 1 {
        args.extend(["--fuel".to_owned(), MULTI_BOT_FUEL.to_owned()]);
    }
    args.push(room.display().to_string());
    Run::start(&args, &[])
}

fn label_is(label: &'static str) -> impl Fn(&Json) -> bool {
    move |record| record["type"] == "log" && record["label"] == label
}

/// Waits until every bot of `bots` has activated (the environment logs
/// each `bot.activated`).
fn all_activated(run: &Run, bots: &[&str]) {
    for _ in bots {
        run.until(WAIT, label_is("bot.activated"));
    }
}

/// The sessions, in order, of the `log` records labelled `label`.
fn sessions_of(records: &[Json], label: &str) -> Vec<String> {
    records
        .iter()
        .filter(|record| record["type"] == "log" && record["label"] == label)
        .map(|record| record["session"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// A `bot.input` whose observation names the cognition double's behavior.
fn input(action: &str, data: &str) -> Json {
    json!({"observation": {"event": action, "data": data}})
}

/// Communication input from `caller` addressed to `target_ids`.
fn said(text: &str, target_ids: &[&str]) -> Json {
    json!({"source_ids": ["caller"], "target_ids": target_ids, "content": text, "content_type": "text/plain"})
}

/// A root that invokes the lamp fixture as `lamp`, reads it with
/// `<device:snapshot>` by its address (`<parent _sessionid>.<invokeid>`,
/// D110) once it runs, turns it on, and reads it again.
const READER: &str = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:device="https://github.com/stateforward/floppy/namespaces/device"
       version="1.0" datamodel="ecmascript" initial="reading">
  <datamodel>
    <data id="read" expr="null"/>
  </datamodel>
  <state id="reading" initial="before">
    <invoke id="lamp" type="scxml" src="lamp/lamp.scx"/>
    <state id="before">
      <onentry><send event="read" delay="10ms"/></onentry>
      <transition event="read" target="after">
        <device:snapshot location="read" target="'#_scxml_' + _sessionid + '.lamp'"/>
        <log label="read" expr="JSON.stringify(read)"/>
        <send event="lamp.on" target="#_lamp"><param name="brightness" expr="40"/></send>
      </transition>
    </state>
    <state id="after">
      <onentry><send event="read" delay="10ms"/></onentry>
      <!-- Leaving reading would cancel the lamp before this content runs
           (SCXML 3.13), so the read stays inside it. -->
      <transition event="read" target="ending">
        <device:snapshot location="read" target="'#_lamp'"/>
        <log label="read" expr="JSON.stringify(read)"/>
      </transition>
    </state>
    <state id="ending">
      <transition target="done"/>
    </state>
  </state>
  <final id="done"/>
</scxml>"##;

/// Runs `document` as `name` beside a copy of the lamp fixture
/// (`lamp/lamp.scx`) on the logical clock until it ends.
fn run_beside_the_lamp(name: &str, document: &str) -> support::Finished {
    let dir = unique_scratch("environment");
    copy_tree(
        &workspace().join("tests/fixtures/environment/lamp"),
        &dir.join("lamp"),
    );
    let path = write(&dir, name, document);
    let mut run = Run::start(
        &[
            "--clock".to_owned(),
            "logical".to_owned(),
            path.display().to_string(),
        ],
        &[],
    );
    run.end_input();
    run.finish(Duration::from_secs(300))
}

/// The text of every `log` record labelled `label`, parsed as JSON.
fn logged(records: &[Json], label: &str) -> Vec<Json> {
    logs(records)
        .into_iter()
        .filter(|(logged, _)| logged == label)
        .map(|(_, text)| serde_json::from_str(&text).expect("the record is JSON"))
        .collect()
}

#[test]
fn a_device_snapshot_reads_another_session() {
    let finished = run_beside_the_lamp("reader.scx", READER);
    assert!(finished.status.success(), "{}", finished.stderr);
    assert_eq!(
        logged(&finished.records, "read"),
        [
            json!({"id": "session.lamp", "state": "off", "attributes": {"brightness": 0}, "events": ["lamp.on"]}),
            json!({"id": "session.lamp", "state": "on", "attributes": {"brightness": 40}, "events": ["lamp.off"]}),
        ],
        "{:?}",
        finished.records
    );
}

#[test]
fn a_bot_activates_on_joining_its_environment() {
    let dir = layout(&[]);
    let run = start(&dir, &["floppy"]);
    // Nothing sends bot.activate: the bot activates on joining, and the
    // environment logs the event it sends.
    let records = run.until(WAIT, label_is("bot.activated"));
    let activated = records.last().expect("the bot.activated record");
    assert_eq!(activated["session"], json!("session"), "{activated}");
    assert_eq!(activated["text"], json!("{}"));
    drop(run.close(WAIT));
}

#[test]
fn host_communication_input_goes_to_the_participants_it_targets() {
    let dir = layout(&[]);
    let mut run = start(&dir, &["a", "b"]);
    all_activated(&run, &["a", "b"]);
    run.event(
        "bot.ability.communication.input",
        &said("ignore this", &["a"]),
    );
    let mut records = run.until(WAIT, label_is("bot.processing.completed"));
    // Ambient input: every bot takes a turn.
    run.event("bot.ability.communication.input", &said("ignore that", &[]));
    records.extend(run.until(WAIT, label_is("bot.processing.completed")));
    records.extend(run.until(WAIT, label_is("bot.processing.completed")));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    let mut turns = sessions_of(&records, "double.input");
    turns.sort();
    assert_eq!(
        turns,
        [
            "session.a.cognition",
            "session.a.cognition",
            "session.b.cognition"
        ],
        "{records:?}"
    );
}

#[test]
fn a_host_sound_is_broadcast_to_every_participant_and_fans_out_to_the_input_abilities() {
    let dir = layout(&[("input", "['identity']")]);
    double_the_identity(&dir);
    let mut run = start(&dir, &["a", "b"]);
    all_activated(&run, &["a", "b"]);
    let sound = json!({"audio": "media/audio-1", "kind": "knock"});
    run.event("environment.sound", &sound);
    let visual = json!({"image": "media/image-1", "media_type": "image/png"});
    run.event("environment.visual", &visual);
    let mut records = run.until(WAIT, label_is("input.visual"));
    records.extend(run.until(WAIT, label_is("input.visual")));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    let mut heard = sessions_of(&records, "input.sound");
    heard.sort();
    assert_eq!(heard, ["session.a.identity", "session.b.identity"]);
    let mut seen = sessions_of(&records, "input.visual");
    seen.sort();
    assert_eq!(seen, ["session.a.identity", "session.b.identity"]);
    assert!(
        sessions_of(&records, "double.input").is_empty(),
        "no cognition hears a stimulus: {records:?}"
    );
}

#[test]
fn host_bot_input_goes_to_every_bot() {
    let dir = layout(&[]);
    let mut run = start(&dir, &["a", "b"]);
    all_activated(&run, &["a", "b"]);
    run.event("bot.input", &input("ignore", "x"));
    let mut records = run.until(WAIT, label_is("bot.processing.completed"));
    records.extend(run.until(WAIT, label_is("bot.processing.completed")));
    let mut turns = sessions_of(&records, "double.input");
    turns.sort();
    assert_eq!(turns, ["session.a.cognition", "session.b.cognition"]);
    drop(run.close(WAIT));
}

#[test]
fn a_deactivated_bot_leaves_its_environment() {
    let dir = layout(&[]);
    let mut run = start(&dir, &["floppy"]);
    all_activated(&run, &["floppy"]);
    run.event("bot.deactivate", &json!({}));
    let records = run.until(WAIT, label_is("done.invoke"));
    assert_eq!(
        logs(&records)
            .into_iter()
            .filter(|(label, _)| label == "bot.deactivated" || label == "done.invoke")
            .collect::<Vec<_>>(),
        support::labelled(&[
            ("bot.deactivated", "{}"),
            ("done.invoke", r#"{"invokeid":"floppy"}"#)
        ]),
        "the bot's session ended in inactive: {records:?}"
    );
    // participants dropped it: the next broadcast is sent to nobody.
    run.event("environment.sound", &json!({"audio": "media/audio-1"}));
    let finished = run.close(WAIT);
    assert!(
        sessions_of(&finished.records, "error.communication").is_empty(),
        "{:?}",
        finished.records
    );
}

#[test]
fn the_environment_refuses_host_input_its_schemas_refuse() {
    let dir = layout(&[]);
    let mut run = start(&dir, &["floppy"]);
    all_activated(&run, &["floppy"]);
    run.event(
        "bot.ability.communication.input",
        &json!({"source_ids": [], "target_ids": [], "content": "x", "content_type": "text/plain"}),
    );
    run.until(WAIT, |record| {
        record["type"] == "error" && record["code"] == "machine-invalid-input"
    });
    let finished = run.close(WAIT);
    assert!(
        sessions_of(&finished.records, "double.input").is_empty(),
        "{:?}",
        finished.records
    );
}

/// Starts the room laid into `dir` with one floppy bot per id in `bots`,
/// hearing through the fake hearing backend (`support::hearing`): the
/// speech double's first utterance, `media/speaking-1.pcm`, spells
/// `words` (24 kHz mono PCM, as the double reports it), and
/// `media/silence.wav` is silence.
fn start_hearing(
    dir: &Path,
    bots: &[&str],
    test: &str,
    words: &str,
) -> (Run, support::hearing::Backend) {
    use support::hearing;
    let backend = hearing::Backend::start();
    let mut args = hearing::granted(
        test,
        &backend,
        &[
            ("speaking-1.pcm", hearing::pcm(&hearing::voiced(words))),
            ("silence.wav", hearing::silent_wav()),
        ],
    )
    .args;
    if bots.len() > 1 {
        args.extend(["--fuel".to_owned(), MULTI_BOT_FUEL.to_owned()]);
    }
    args.push(support::environment(dir, bots, &[]).display().to_string());
    (Run::start(&args, &[]), backend)
}

/// The silence that ends an utterance, from no source.
fn silence() -> Json {
    json!({"audio": "media/silence.wav", "media_type": "audio/wav"})
}

#[test]
fn a_bots_speech_reaches_the_other_participant() {
    let dir = layout(&[]);
    let (mut run, backend) = start_hearing(
        &dir,
        &["a", "b"],
        "a_bots_speech_reaches_the_other_participant",
        "ignore this",
    );
    all_activated(&run, &["a", "b"]);
    // a answers, through the speech double; its sound goes into the
    // environment, which gives it to both bots.
    run.event(
        "bot.ability.communication.input",
        &said("reply now", &["a"]),
    );
    let mut all = run.until(WAIT, label_is("bot.ability.speaking.output"));
    run.event("environment.sound", &silence());
    all.extend(run.until(WAIT, |record| {
        label_is("double.input")(record) && record["session"] == "session.b.cognition"
    }));
    all.extend(run.close(WAIT).records);
    let turns: Vec<(String, String)> = all
        .iter()
        .filter(|record| record["type"] == "log" && record["label"] == "double.input")
        .map(|record| {
            (
                record["session"].as_str().unwrap_or_default().to_owned(),
                record["text"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        turns,
        [
            ("session.a.cognition".to_owned(), "1 reply 1".to_owned()),
            (
                "session.b.cognition".to_owned(),
                r#"1 ignore "ignore this""#.to_owned()
            ),
        ],
        "{all:?}"
    );
    assert!(backend.decoded().contains(&"ignore this".to_owned()));
}

#[test]
fn a_bot_hears_its_own_speech_through_the_environment_and_withholds_it() {
    let dir = layout(&[]);
    let (mut run, backend) = start_hearing(
        &dir,
        &["floppy"],
        "a_bot_hears_its_own_speech_through_the_environment_and_withholds_it",
        "ignore this",
    );
    all_activated(&run, &["floppy"]);
    run.event("bot.ability.communication.input", &said("reply now", &[]));
    let spoken = run.until(WAIT, label_is("bot.ability.speaking.output"));
    run.event("environment.sound", &silence());
    // The listening decodes the bot's own words, after its efference
    // copy, and hands over no product (decision D98).
    let mut seen = Vec::new();
    while !backend.decoded().contains(&"ignore this".to_owned()) {
        seen.extend(run.until(WAIT, |record| record["type"] == "call"));
    }
    let finished = run.close(WAIT);
    seen.extend(finished.records);
    assert_eq!(
        sessions_of(&seen, "double.input"),
        Vec::<String>::new(),
        "{seen:?}"
    );
    let mut all = spoken;
    all.extend(seen);
    assert!(
        logs(&all)
            .iter()
            .any(|(label, text)| label == "environment.sound" && text.contains("speaking-1.pcm")),
        "the environment delivered the bot's own sound: {all:?}"
    );
}
