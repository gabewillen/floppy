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
        "stm",
        "persona",
        "seeing",
    ] {
        support::copy_ability(name, &abilities);
    }
    support::copy_listening(&abilities);
    support::double_speech(&abilities, "'encode'");
    support::copy_cognition_double(&abilities);
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

/// Starts the environment laid into `dir` with one floppy bot per id in `bots`.
fn start(dir: &Path, bots: &[&str]) -> Run {
    let environment = support::environment(dir, bots, &[]);
    Run::start(&[environment.display().to_string()], &[])
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
            json!({"id": "session.lamp", "state": "off", "attributes": {"brightness": 0, "click_db": 60}, "events": ["lamp.on"]}),
            json!({"id": "session.lamp", "state": "on", "attributes": {"brightness": 40, "click_db": 60}, "events": ["lamp.off"]}),
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

/// A voice embedding is biometric and never reaches the run's records:
/// the environment logs a conversation output whose input's source is an
/// embedding with the marker `<biometric:VoiceEmbedding>` in its place, and
/// whose message names the person memory holds for the voice.
#[test]
fn a_voice_embedding_is_never_written_to_the_records() {
    let dir = layout(&[]);
    let mut run = start(&dir, &["floppy"]);
    all_activated(&run, &["floppy"]);
    // A product that is not text is no turn: the conversation's output
    // reaches the environment, which logs it.
    run.event(
        "bot.ability.communication.input",
        &json!({"source_ids": [[0.28, 0.96]], "target_ids": ["floppy"], "content": {"a": 1}, "content_type": "application/json"}),
    );
    let mut records = run.until(WAIT, label_is("bot.ability.conversation.output"));
    records.extend(run.close(WAIT).records);
    let logged = logged(&records, "bot.ability.conversation.output");
    let [output] = logged.as_slice() else {
        panic!("one conversation output: {records:?}");
    };
    let marker = json!(["<biometric:VoiceEmbedding>"]);
    assert_eq!(output["parent"]["data"]["source_ids"], marker, "{output}");
    // The message names the person the voice is (decision D194).
    assert_eq!(
        output["messages"][0]["source_ids"],
        json!(["person-1"]),
        "{output}"
    );
    assert_eq!(
        output["messages"][0]["target_ids"],
        json!(["floppy"]),
        "{output}"
    );
    let text = serde_json::to_string(&records).expect("the records are JSON");
    assert!(!text.contains("0.28") && !text.contains("0.96"), "{text}");
}

/// Decision D194: long-term memory holds people. The same voice twice is
/// one person, a different voice another, and a person outlives the bot:
/// the bot started again on the same database knows the voice. Each input
/// is a product that is not text, so the environment logs the conversation's
/// output, whose message names the person in the voice's place.
#[test]
fn a_voice_is_one_person_across_turns_and_restarts() {
    let dir = layout(&[]);
    let memory = unique_scratch("people");
    let environment = support::environment(&dir, &["floppy"], &[]);
    let start_on_memory = || {
        let run = Run::start(
            &[
                "--dir".to_owned(),
                format!(
                    "{}={}::/memory",
                    support::service_uri("sql"),
                    memory.display()
                ),
                environment.display().to_string(),
            ],
            &[],
        );
        all_activated(&run, &["floppy"]);
        run
    };
    let person_of = |run: &mut Run, records: &mut Vec<Json>, voice: [f64; 2]| {
        run.event(
            "bot.ability.communication.input",
            &json!({"source_ids": [voice], "target_ids": ["floppy"], "content": {"a": 1}, "content_type": "application/json"}),
        );
        let heard = run.until(WAIT, label_is("bot.ability.conversation.output"));
        let output = logged(&heard, "bot.ability.conversation.output")
            .pop()
            .expect("the conversation's output");
        records.extend(heard);
        output["messages"][0]["source_ids"].clone()
    };
    let mut records = Vec::new();
    let mut run = start_on_memory();
    let first = person_of(&mut run, &mut records, [0.28, 0.96]);
    let again = person_of(&mut run, &mut records, [0.3, 0.95]);
    let other = person_of(&mut run, &mut records, [0.96, -0.28]);
    records.extend(run.close(WAIT).records);
    let mut run = start_on_memory();
    let restarted = person_of(&mut run, &mut records, [0.27, 0.96]);
    let newcomer = person_of(&mut run, &mut records, [-0.7, 0.7]);
    records.extend(run.close(WAIT).records);
    assert_eq!(
        [first, again, other, restarted, newcomer],
        [
            json!(["person-1"]),
            json!(["person-1"]),
            json!(["person-2"]),
            json!(["person-1"]),
            json!(["person-3"]),
        ],
        "{records:?}"
    );
    // The voices are stored, never written to the records.
    let text = serde_json::to_string(&records).expect("the records are JSON");
    for voice in [
        "0.28,0.96",
        "0.3,0.95",
        "0.96,-0.28",
        "0.27,0.96",
        "-0.7,0.7",
    ] {
        assert!(!text.contains(voice), "{voice}: {text}");
    }
}

/// The compact text of the representative voice of 256 dimensions along
/// `axis` (`i8:` and the base64 of its signed bytes: 127 there, 0
/// elsewhere), as the conversation namespace writes it.
fn axis_representative(axis: usize) -> String {
    const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = vec![0_u8; 256];
    bytes[axis] = 127;
    let mut text = String::from("i8:");
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|i| chunk.get(i).copied().unwrap_or(0));
        let sextets = [
            a >> 2,
            ((a & 3) << 4) | (b >> 4),
            ((b & 15) << 2) | (c >> 6),
            c & 63,
        ];
        for (i, sextet) in sextets.iter().enumerate() {
            text.push(if i <= chunk.len() {
                char::from(BASE64[usize::from(*sextet)])
            } else {
                '='
            });
        }
    }
    text
}

/// Runs `statements` in one transaction of the memory ability on the
/// SQLite database in `memory`.
fn seed_memory(memory: &Path, statements: &Json) {
    let dir = unique_scratch("seed");
    support::copy_ability("memory", &dir);
    let document = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="applying">
  <datamodel><data id="statements" expr="{}"/></datamodel>
  <state id="applying">
    <invoke id="memory" type="scxml" src="memory/memory.scx">
      <param name="database" expr="'/memory/floppy.sqlite'"/>
      <param name="statements" expr="statements"/>
    </invoke>
    <transition event="bot.ability.memory.output" target="done"><log label="output" expr="'seeded'"/></transition>
    <transition event="*" target="done"><log label="failed" expr="_event.name"/></transition>
  </state>
  <final id="done"/>
</scxml>"#,
        statements
            .to_string()
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;"),
    );
    let path = write(&dir, "seed.scx", &document);
    let run = Run::start(
        &[
            "--dir".to_owned(),
            format!(
                "{}={}::/memory",
                support::service_uri("sql"),
                memory.display()
            ),
            path.display().to_string(),
        ],
        &[],
    );
    let finished = run.finish(WAIT);
    assert!(
        logs(&finished.all).contains(&("output".to_owned(), "seeded".to_owned())),
        "{:?}",
        finished.all
    );
}

/// Decision D205: identification reads one compact representative a
/// person, so memory holding 96 people at 256 dimensions (D204 read every
/// voice heard and failed at about a dozen) still identifies each, and a
/// new voice is numbered from the people sequence, after them.
#[test]
fn a_voice_is_identified_among_ninety_six_people() {
    let dir = layout(&[]);
    let memory = unique_scratch("people");
    let insert_people = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 96) INSERT INTO bot_person (person_id) SELECT 'person-' || i FROM n";
    let mut statements = vec![
        json!({"sql": insert_people}),
        json!({"sql": "INSERT INTO bot_sequence (name, value) VALUES ('person', 96)"}),
    ];
    for batch in 0..4 {
        let people: Vec<usize> = (batch * 24..batch * 24 + 24).collect();
        let values = vec!["(?, 'voice', ?, 1, ?, ?)"; people.len()].join(", ");
        let parameters: Vec<Json> = people
            .iter()
            .flat_map(|axis| {
                [
                    json!(format!("person-{}", axis + 1)),
                    json!(axis_representative(*axis)),
                    json!(axis + 1),
                    json!(axis + 1),
                ]
            })
            .collect();
        statements.push(json!({
            "sql": format!("INSERT INTO bot_person_centroid (person_id, kind, centroid, weight, first_seq, through_seq) VALUES {values}"),
            "parameters": parameters,
        }));
    }
    // One statement a transaction: each invocation's statements stay
    // well within what one invoke carries.
    for statement in statements {
        seed_memory(&memory, &json!([statement]));
    }
    let environment = support::environment(&dir, &["floppy"], &[]);
    let mut run = Run::start(
        &[
            "--dir".to_owned(),
            format!(
                "{}={}::/memory",
                support::service_uri("sql"),
                memory.display()
            ),
            environment.display().to_string(),
        ],
        &[],
    );
    all_activated(&run, &["floppy"]);
    let mut person_of = |voice: Vec<f64>| {
        run.event(
            "bot.ability.communication.input",
            &json!({"source_ids": [voice], "target_ids": ["floppy"], "content": {"a": 1}, "content_type": "application/json"}),
        );
        let heard = run.until(WAIT, label_is("bot.ability.conversation.output"));
        let output = logged(&heard, "bot.ability.conversation.output")
            .pop()
            .expect("the conversation's output");
        output["messages"][0]["source_ids"].clone()
    };
    let near = |axis: usize| -> Vec<f64> {
        (0..256)
            .map(|d| {
                if d == axis {
                    0.95
                } else if d == (axis + 1) % 256 {
                    0.2
                } else {
                    0.0
                }
            })
            .collect()
    };
    let first = person_of(near(0));
    let late = person_of(near(79));
    let last = person_of(near(95));
    let newcomer = person_of(vec![0.0625; 256]);
    drop(run.close(WAIT));
    assert_eq!(
        [first, late, last, newcomer],
        [
            json!(["person-1"]),
            json!(["person-80"]),
            json!(["person-96"]),
            json!(["person-97"]),
        ]
    );
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
    // A bot that leaves on bot.deactivate is not restarted (D158).
    assert!(
        sessions_of(&finished.records, "supervision.restarted").is_empty(),
        "{:?}",
        finished.records
    );
}

/// Sets the environment at `environment`'s supervision (decision D158) to `supervision`,
/// an ECMAScript object expression.
fn supervise(environment: &Path, supervision: &str) {
    let text = std::fs::read_to_string(environment).expect("the environment is readable");
    let start = text
        .find(r#"<data id="supervision" expr=""#)
        .expect("the environment declares its supervision");
    let end = start + text[start..].find("/>").expect("the <data> element closes");
    let mut text = text;
    text.replace_range(
        start..end,
        &format!(r#"<data id="supervision" expr="({supervision})""#),
    );
    std::fs::write(environment, text).expect("the environment copy is writable");
}

#[test]
fn a_bot_sent_bot_deactivate_in_its_backoff_is_not_restarted() {
    let dir = layout(&[("processing_timeout_ms", "300")]);
    let environment = support::environment(&dir, &["floppy"], &[]);
    supervise(
        &environment,
        "{max_restarts: 5, backoff_initial_ms: 2000, backoff_max_ms: 2000, stable_after_ms: 600000}",
    );
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["floppy"]);
    run.event("bot.input", &input("deaf", "x"));
    let mut records = run.until(WAIT, label_is("done.invoke"));
    // The host stops it while it waits: the backoff goes to stopped, so
    // the 2 s restart never comes.
    run.event("bot.deactivate", &json!({}));
    std::thread::sleep(Duration::from_millis(4000));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    assert!(
        logs(&records)
            .iter()
            .all(|(label, _)| label != "supervision.restarted"),
        "{records:?}"
    );
}

#[test]
fn a_bot_that_runs_stable_after_ms_has_its_attempts_reset() {
    let dir = layout(&[("processing_timeout_ms", "300")]);
    let environment = support::environment(&dir, &["floppy"], &[]);
    // With max_restarts 2 a second failed attempt would be its last; a
    // bot active for stable_after_ms between them starts counting again.
    supervise(
        &environment,
        "{max_restarts: 2, backoff_initial_ms: 200, backoff_max_ms: 200, stable_after_ms: 1500}",
    );
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["floppy"]);
    run.event("bot.input", &input("deaf", "x"));
    let mut records = run.until(WAIT, label_is("supervision.restarted"));
    records.extend(run.until(WAIT, label_is("bot.activated")));
    std::thread::sleep(Duration::from_millis(3000));
    run.event("bot.input", &input("deaf", "x"));
    records.extend(run.until(WAIT, label_is("supervision.restarted")));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    assert_eq!(
        logs(&records)
            .into_iter()
            .filter(|(label, _)| (label == "supervision.restarted" || label == "supervision.exhausted"))
            .collect::<Vec<_>>(),
        support::labelled(&[
            (
                "supervision.restarted",
                r#"{"participant":"floppy","attempt":1,"delay_ms":200}"#
            ),
            (
                "supervision.restarted",
                r#"{"participant":"floppy","attempt":1,"delay_ms":200}"#
            ),
        ])
    );
}

#[test]
fn an_unfocused_body_focuses_a_held_percepts_device_at_the_turns_end() {
    let dir = layout(&[("processing_timeout_ms", "1500")]);
    let mut run = start_with_lamp(&dir, &["floppy"], &[("floppy", &["lamp"])]);
    all_activated(&run, &["floppy"]);
    // A host input with no device: the turn starts unfocused and hangs.
    run.event("bot.input", &input("hang", "x"));
    run.until(WAIT, label_is("double.participants"));
    // The lamp's input is held; when the turn ends, the reflex floor
    // focuses its device before the next turn takes it.
    run.event("bot.input", &lamp_input("reply", &json!("y")));
    let records = run.until(WAIT, |record| {
        label_is("double.input")(record)
            && record["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("2 "))
    });
    assert_eq!(
        logs(&records)
            .into_iter()
            .filter(|(label, _)| label == "floppy.focus" || label == "double.input")
            .collect::<Vec<_>>(),
        support::labelled(&[("floppy.focus", "lamp"), ("double.input", r#"2 reply "y""#)])
    );
    drop(run.close(WAIT));
}

#[test]
fn a_degraded_bot_is_restarted_after_a_backoff_until_its_budget_is_spent() {
    let dir = layout(&[("processing_timeout_ms", "300")]);
    let environment = support::environment(&dir, &["floppy"], &[]);
    supervise(
        &environment,
        "{max_restarts: 3, backoff_initial_ms: 200, backoff_max_ms: 300, stable_after_ms: 600000}",
    );
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["floppy"]);
    // An unacknowledged cancel degrades the bot; the environment restarts it
    // after 200 ms, then 300 ms (doubled, capped), and leaves it once a
    // third failed attempt would not fit max_restarts 3.
    let mut records = Vec::new();
    for _ in 0..2 {
        run.event("bot.input", &input("deaf", "x"));
        records.extend(run.until(WAIT, label_is("supervision.restarted")));
        records.extend(run.until(WAIT, label_is("bot.activated")));
    }
    run.event("bot.input", &input("deaf", "x"));
    records.extend(run.until(WAIT, label_is("supervision.exhausted")));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    assert_eq!(
        logs(&records)
            .into_iter()
            .filter(
                |(label, _)| (label == "supervision.restarted" || label == "supervision.exhausted")
                    || label == "bot.degraded"
            )
            .collect::<Vec<_>>(),
        support::labelled(&[
            ("bot.degraded", r#"{"stage":"processing_cancel"}"#),
            (
                "supervision.restarted",
                r#"{"participant":"floppy","attempt":1,"delay_ms":200}"#
            ),
            ("bot.degraded", r#"{"stage":"processing_cancel"}"#),
            (
                "supervision.restarted",
                r#"{"participant":"floppy","attempt":2,"delay_ms":300}"#
            ),
            ("bot.degraded", r#"{"stage":"processing_cancel"}"#),
            ("supervision.exhausted", r#"{"participant":"floppy","attempts":3}"#),
        ])
    );
    // A restarted bot is a participant again: host input reached it.
    assert_eq!(
        sessions_of(&records, "double.input"),
        [
            "session.floppy.cognition",
            "session.floppy.cognition",
            "session.floppy.cognition"
        ]
    );
}

#[test]
fn a_bot_whose_activation_fails_is_restarted() {
    // An activation timeout (an expected ability never offers) fails the
    // activation, and the environment restarts the bot.
    let dir = layout(&[
        ("activation_timeout_ms", "200"),
        (
            "expected",
            "['cognition', 'communication', 'identity', 'listening', 'ghost']",
        ),
    ]);
    let environment = support::environment(&dir, &["floppy"], &[]);
    supervise(
        &environment,
        "{max_restarts: 2, backoff_initial_ms: 100, backoff_max_ms: 100, stable_after_ms: 600000}",
    );
    let run = Run::start(&[environment.display().to_string()], &[]);
    let mut records = run.until(WAIT, label_is("supervision.restarted"));
    records.extend(run.until(WAIT, label_is("supervision.exhausted")));
    drop(run.close(WAIT));
    assert_eq!(
        logs(&records)
            .into_iter()
            .filter(
                |(label, _)| (label == "supervision.restarted" || label == "supervision.exhausted")
                    || label == "bot.activating.failed"
            )
            .collect::<Vec<_>>(),
        support::labelled(&[
            ("bot.activating.failed", "{}"),
            (
                "supervision.restarted",
                r#"{"participant":"floppy","attempt":1,"delay_ms":100}"#
            ),
            ("bot.activating.failed", "{}"),
            ("supervision.exhausted", r#"{"participant":"floppy","attempts":2}"#),
        ])
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

/// Starts the environment laid into `dir` with one floppy bot per id in `bots`,
/// hearing through the fake hearing backend (`support::hearing`): the
/// speech double's first utterance's one chunk, `media/speaking-1.0.pcm`, spells
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
            ("speaking-1.0.pcm", hearing::pcm(&hearing::voiced(words))),
            ("silence.wav", hearing::silent_wav()),
        ],
    )
    .args;
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
            .any(|(label, text)| label == "environment.sound" && text.contains("speaking-1.0.pcm")),
        "the environment delivered the bot's own sound: {all:?}"
    );
}

/// Starts the environment laid into `dir` with the lamp fixture (`lamp`) and one
/// floppy bot per id in `bots`, each bot holding the devices `holdings`
/// names.
fn start_with_lamp(dir: &Path, bots: &[&str], holdings: &[(&str, &[&str])]) -> Run {
    let environment = support::environment_with_devices(dir, bots, &["lamp"], holdings, &[]);
    Run::start(&[environment.display().to_string()], &[])
}

/// A `bot.input` from the host stamped with the lamp as its device.
fn lamp_input(action: &str, data: &Json) -> Json {
    json!({"observation": {"event": action, "data": data}, "target_device": "lamp"})
}

/// The session object `id` of a `scxml.session-snapshot/v2` document.
fn session_in<'a>(session: &'a Json, id: &str) -> Option<&'a Json> {
    if session["session_id"] == id {
        return Some(session);
    }
    session["invocations"]
        .as_array()?
        .iter()
        .find_map(|invocation| session_in(&invocation["session"], id))
}

/// The value of the `<data id="name">` of the session `id`, from a
/// snapshot the run writes now.
fn data_of(run: &mut Run, id: &str, name: &str) -> Json {
    run.send(&json!({"type": "snapshot"}));
    let records = run.until(WAIT, |record| record["type"] == "snapshot");
    let record = records.last().expect("the snapshot record");
    let document: Json = match &record["document"] {
        Json::String(text) => serde_json::from_str(text).expect("the snapshot is JSON"),
        other => other.clone(),
    };
    let session = session_in(&document["snapshot"]["root"], id)
        .unwrap_or_else(|| panic!("the snapshot holds {id}: {document}"));
    let value = session["data_model"]
        .as_array()
        .expect("a data model")
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("{id} declares {name}"))["value"]
        .clone();
    match value["type"].as_str() {
        Some("json-object" | "json-array") => {
            serde_json::from_str(value["value"].as_str().expect("JSON text")).expect("JSON")
        }
        _ => value["value"].clone(),
    }
}

/// The text of every `log` record labelled `label` of the session
/// `session`.
fn texts_of(records: &[Json], session: &str, label: &str) -> Vec<String> {
    records
        .iter()
        .filter(|record| {
            record["type"] == "log" && record["label"] == label && record["session"] == session
        })
        .map(|record| record["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Waits for the environment's next `bot.processing.completed` and returns every
/// record until it.
fn completed(run: &Run) -> Vec<Json> {
    run.until(WAIT, label_is("bot.processing.completed"))
}

#[test]
fn a_bot_is_given_the_ids_of_the_devices_it_holds() {
    let dir = layout(&[]);
    let mut run = start_with_lamp(&dir, &["floppy"], &[("floppy", &["lamp"])]);
    all_activated(&run, &["floppy"]);
    assert_eq!(
        data_of(&mut run, "session.floppy", "owned_devices"),
        json!({"lamp": "session.lamp"})
    );
    assert_eq!(
        data_of(&mut run, "session.floppy", "devices"),
        json!(["lamp"])
    );
    drop(run.close(WAIT));
}

#[test]
fn a_held_devices_input_grounds_the_turns_focus() {
    let dir = layout(&[]);
    let mut run = start_with_lamp(&dir, &["floppy"], &[("floppy", &["lamp"])]);
    all_activated(&run, &["floppy"]);
    // Focused on the lamp (the host's input names it), the bot turns it
    // on; the lamp reports the change, which reaches the bot with the
    // lamp as its target_device.
    run.event("bot.input", &lamp_input("focus", &json!("lamp")));
    let mut records = completed(&run);
    run.event(
        "bot.input",
        &json!({"observation": {"event": "act", "data": {"event": "lamp.on", "data": {}}}}),
    );
    records.extend(completed(&run));
    records.extend(completed(&run));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    // The reflex floor (unfocused, the lamp's input turns the bot to the
    // lamp), then the cognition's selection.
    assert_eq!(
        texts_of(&records, "session.floppy", "floppy.focus"),
        ["lamp", "lamp"]
    );
    assert_eq!(texts_of(&records, "session.lamp", "lamp.on"), ["100"]);
    let turns = texts_of(&records, "session", "bot.processing.completed");
    let last: Json = serde_json::from_str(turns.last().expect("the lamp's turn")).expect("JSON");
    assert_eq!(last["turn"], json!(3), "{turns:?}");
    assert_eq!(last["focus_candidates"], json!(["lamp"]), "{turns:?}");
    assert_eq!(
        texts_of(&records, "session.floppy.cognition", "double.input").last(),
        Some(&r#"3 ignore "on""#.to_owned())
    );
}

#[test]
fn a_focused_devices_choices_are_read_from_its_snapshot_and_a_selection_reaches_it() {
    let dir = layout(&[]);
    let mut run = start_with_lamp(&dir, &["floppy"], &[("floppy", &["lamp"])]);
    all_activated(&run, &["floppy"]);
    run.event("bot.input", &lamp_input("focus", &json!("lamp")));
    let mut records = completed(&run);
    run.event("bot.input", &json!({"observation": {"event": "act", "data": {"event": "lamp.on", "data": {"brightness": 40}}}}));
    records.extend(completed(&run));
    // The lamp's report is the next turn, focused on the lamp, now on.
    records.extend(completed(&run));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    let choices: Vec<String> = texts_of(&records, "session.floppy", "floppy.choices")
        .iter()
        .map(|text| support::choice_names(text))
        .collect();
    assert_eq!(choices.len(), 3, "{choices:?}");
    // Unfocused, the lamp's input turns the bot to the lamp
    // (decision D155), so its events are that very turn's
    // choices. Focused on the lamp while it is off: lamp.on and not
    // lamp.off; then on: lamp.off and not lamp.on.
    for off in &choices[..2] {
        assert!(
            off.contains("lamp.on@session.lamp") && !off.contains("lamp.off"),
            "{choices:?}"
        );
    }
    assert!(
        choices[2].contains("lamp.off@session.lamp") && !choices[2].contains("lamp.on"),
        "{choices:?}"
    );
    // The cognition sent the focus to the body and the action to the
    // lamp, at their session ids.
    // The reflex floor (unfocused, the lamp's input turns the bot to the
    // lamp), then the cognition's selection.
    assert_eq!(
        texts_of(&records, "session.floppy", "floppy.focus"),
        ["lamp", "lamp"]
    );
    assert_eq!(texts_of(&records, "session.lamp", "lamp.on"), ["40"]);
}

#[test]
fn only_its_holders_are_offered_a_devices_choices() {
    let dir = layout(&[]);
    let mut run = start_with_lamp(&dir, &["a", "b"], &[("a", &["lamp"])]);
    all_activated(&run, &["a", "b"]);
    assert_eq!(data_of(&mut run, "session.b", "owned_devices"), json!({}));
    // Both bots get the host's input naming the lamp: only a holds it, so
    // only a can focus it (b is offered no focus, and takes none) and is
    // offered its events; the lamp's report goes to a alone.
    run.event("bot.input", &lamp_input("focus", &json!("lamp")));
    let mut records = Vec::new();
    let outcomes = |records: &[Json]| {
        logs(records)
            .iter()
            .filter(|(label, _)| {
                label == "bot.processing.completed" || label == "bot.processing.failed"
            })
            .count()
    };
    while outcomes(&records) < 2 {
        records.extend(run.until(WAIT, |record| {
            label_is("bot.processing.completed")(record)
                || label_is("bot.processing.failed")(record)
        }));
    }
    run.event(
        "bot.input",
        &json!({"observation": {"event": "act", "data": {"event": "lamp.on", "data": {}}}}),
    );
    while outcomes(&records) < 5 {
        records.extend(run.until(WAIT, |record| {
            label_is("bot.processing.completed")(record)
                || label_is("bot.processing.failed")(record)
        }));
    }
    let finished = run.close(WAIT);
    records.extend(finished.records);
    assert!(
        texts_of(&records, "session.b", "floppy.choices")
            .iter()
            .all(|choices| !choices.contains("lamp.")),
        "{records:?}"
    );
    assert!(texts_of(&records, "session.b", "floppy.focus").is_empty());
    assert_eq!(texts_of(&records, "session.lamp", "lamp.on"), ["100"]);
    assert_eq!(
        texts_of(&records, "session.a.cognition", "double.input").last(),
        Some(&r#"3 ignore "on""#.to_owned())
    );
    assert_eq!(
        texts_of(&records, "session.b.cognition", "double.input").len(),
        2
    );
}

#[test]
fn any_participant_can_act_on_a_device() {
    let dir = layout(&[]);
    // A participant that holds nothing turns the lamp on by its address
    // (D110), on the first visual it sees.
    let actor = dir.join("environments/environment/actor");
    std::fs::create_dir_all(&actor).expect("creatable");
    write(
        &actor,
        "actor.scx",
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="waiting">
  <state id="waiting">
    <transition event="environment.visual" target="done">
      <send event="lamp.on" target="#_scxml_session.lamp"><content expr="({brightness: 7})"/></send>
    </transition>
  </state>
  <final id="done"/>
</scxml>"##,
    );
    let environment = support::environment_with_devices(&dir, &[], &["lamp", "actor"], &[], &[]);
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    run.event("environment.visual", &json!({"image": "media/image-1"}));
    let records = run.until(WAIT, label_is("lamp.on"));
    drop(run.close(WAIT));
    assert_eq!(texts_of(&records, "session.lamp", "lamp.on"), ["7"]);
}

#[test]
fn a_device_action_the_device_refuses_is_an_error_in_the_sender() {
    let dir = layout(&[]);
    let mut run = start_with_lamp(&dir, &["floppy"], &[("floppy", &["lamp"])]);
    all_activated(&run, &["floppy"]);
    run.event("bot.input", &lamp_input("focus", &json!("lamp")));
    let mut records = completed(&run);
    // A brightness the lamp's schema refuses: not delivered.
    run.event("bot.input", &json!({"observation": {"event": "act", "data": {"event": "lamp.on", "data": {"brightness": 500}}}}));
    records.extend(completed(&run));
    // The bot still takes its next turn.
    run.event(
        "bot.input",
        &json!({"observation": {"event": "ignore", "data": "x"}}),
    );
    records.extend(completed(&run));
    let finished = run.close(WAIT);
    records.extend(finished.records);
    // The reflex floor (unfocused, the lamp's input turns the bot to the
    // lamp), then the cognition's selection.
    assert_eq!(
        texts_of(&records, "session.floppy", "floppy.focus"),
        ["lamp", "lamp"]
    );
    assert!(
        texts_of(&records, "session.lamp", "lamp.on").is_empty(),
        "{records:?}"
    );
    assert_eq!(
        texts_of(&records, "session", "bot.processing.completed").len(),
        3
    );
}

/// One placed environment: the floppy bot (its identity the input double, so it
/// logs `input.sound` and `input.visual` for each stimulus) holding the
/// lamp, which clicks at `click_db`, placed per `placements` (an
/// ECMAScript object expression). The bot focuses the lamp and turns it
/// on: the lamp clicks, then reports (turn 3). Returns every record.
fn click(click_db: &str, placements: &str) -> Vec<Json> {
    let dir = layout(&[("input", "['identity']")]);
    double_the_identity(&dir);
    let environment = support::environment_with_devices(
        &dir,
        &["floppy"],
        &["lamp"],
        &[("floppy", &["lamp"])],
        &[],
    );
    support::place(&environment, placements);
    let lamp = dir.join("environments/environment/lamp/lamp.scx");
    let text = std::fs::read_to_string(&lamp).expect("the lamp copy is readable");
    std::fs::write(
        &lamp,
        text.replace(
            r#"<data id="click_db" expr="60"/>"#,
            &format!(r#"<data id="click_db" expr="{click_db}"/>"#),
        ),
    )
    .expect("the lamp copy is writable");
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["floppy"]);
    run.event("bot.input", &lamp_input("focus", &json!("lamp")));
    let mut records = completed(&run);
    run.event(
        "bot.input",
        &json!({"observation": {"event": "act", "data": {"event": "lamp.on", "data": {}}}}),
    );
    records.extend(completed(&run));
    records.extend(completed(&run));
    records.extend(run.close(WAIT).records);
    records
}

/// The lamp's clicks the bot heard: the data of each `input.sound` its
/// input ability logged for a sound of kind `lamp.on`.
fn heard_clicks(records: &[Json]) -> Vec<Json> {
    texts_of(records, "session.floppy.identity", "input.sound")
        .into_iter()
        .map(|text| serde_json::from_str::<Json>(&text).expect("JSON"))
        .filter(|sound| sound["kind"] == "lamp.on")
        .collect()
}

const LAMP_AT_ORIGIN: &str = "lamp: {position: {x: 0, y: 0, z: 0}, threshold_db: 0}";

#[test]
fn a_sound_carries_by_distance() {
    // 2 m away: 60 - 20 log10(2) dB.
    let records = click(
        "60",
        &format!("{LAMP_AT_ORIGIN}, floppy: {{position: {{x: 2, y: 0, z: 0}}, threshold_db: 0}}"),
    );
    let clicks = heard_clicks(&records);
    assert_eq!(clicks.len(), 1, "{records:?}");
    let level = clicks[0]["received_level_db"]
        .as_f64()
        .expect("a received level");
    assert!(
        (level - (60.0 - 20.0 * 2f64.log10())).abs() < 1e-9,
        "{level}"
    );
    assert!((level - 53.979_400_086_720_37).abs() < 1e-9, "{level}");
    // The emitter's amplitude is untouched.
    assert_eq!(clicks[0]["amplitude_db"], json!(60));
    // At the lamp: the 1 cm clamp, 100 + 40 dB.
    let records = click(
        "100",
        &format!("{LAMP_AT_ORIGIN}, floppy: {{position: {{x: 0, y: 0, z: 0}}, threshold_db: 0}}"),
    );
    let clicks = heard_clicks(&records);
    assert_eq!(clicks.len(), 1, "{records:?}");
    assert_eq!(clicks[0]["received_level_db"].as_f64(), Some(140.0));
}

#[test]
fn a_participant_below_its_threshold_does_not_hear() {
    let records = click(
        "60",
        &format!("{LAMP_AT_ORIGIN}, floppy: {{position: {{x: 2, y: 0, z: 0}}, threshold_db: 55}}"),
    );
    assert!(heard_clicks(&records).is_empty(), "{records:?}");
    // The click was made: the environment logged it.
    assert!(
        texts_of(&records, "session", "environment.sound")
            .iter()
            .any(|text| text.contains("lamp.on")),
        "{records:?}"
    );
}

#[test]
fn a_sound_too_quiet_for_everyone_reaches_nobody() {
    // Nobody's threshold is met, not even the lamp's own: the click is
    // sent to nobody (not to every participant).
    let records = click(
        "60",
        "lamp: {position: {x: 0, y: 0, z: 0}, threshold_db: 200}, floppy: {position: {x: 2, y: 0, z: 0}, threshold_db: 70}",
    );
    assert!(heard_clicks(&records).is_empty(), "{records:?}");
    assert_eq!(texts_of(&records, "session.lamp", "lamp.on"), ["100"]);
}

#[test]
fn speech_and_host_sounds_have_no_origin_and_reach_everyone() {
    let dir = layout(&[("input", "['identity']")]);
    double_the_identity(&dir);
    let environment = support::environment(&dir, &["floppy"], &[]);
    support::place(
        &environment,
        "floppy: {position: {x: 9, y: 0, z: 0}, threshold_db: 90}",
    );
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["floppy"]);
    // A loud host sound and a quiet one: neither has an origin, so both
    // reach the bot, its level unmeasured.
    run.event(
        "environment.sound",
        &json!({"audio": "media/a.wav", "amplitude_db": 20}),
    );
    run.event(
        "environment.sound",
        &json!({"audio": "media/b.wav", "amplitude_db": 100}),
    );
    let mut records = run.until(WAIT, label_is("input.sound"));
    records.extend(run.until(WAIT, label_is("input.sound")));
    drop(run.close(WAIT));
    let heard: Vec<Json> = texts_of(&records, "session.floppy.identity", "input.sound")
        .into_iter()
        .map(|text| serde_json::from_str(&text).expect("JSON"))
        .collect();
    assert_eq!(
        heard,
        [
            json!({"audio": "media/a.wav", "amplitude_db": 20}),
            json!({"audio": "media/b.wav", "amplitude_db": 100})
        ]
    );
}

#[test]
fn a_visual_reaches_every_participant() {
    let dir = layout(&[("input", "['identity']")]);
    double_the_identity(&dir);
    let environment = support::environment(&dir, &["a", "b"], &[]);
    support::place(
        &environment,
        "a: {position: {x: 0, y: 0, z: 0}, threshold_db: 200}, b: {position: {x: 50, y: 0, z: 0}, threshold_db: 200}",
    );
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["a", "b"]);
    run.event(
        "environment.visual",
        &json!({"image": "media/image-1", "media_type": "image/png"}),
    );
    let mut records = run.until(WAIT, label_is("input.visual"));
    records.extend(run.until(WAIT, label_is("input.visual")));
    drop(run.close(WAIT));
    let mut seen = sessions_of(&records, "input.visual");
    seen.sort();
    assert_eq!(seen, ["session.a.identity", "session.b.identity"]);
}

/// The `environment` the bot sent the cognition double on each turn.
fn environments(records: &[Json]) -> Vec<Json> {
    texts_of(records, "session.floppy.cognition", "double.environment")
        .into_iter()
        .map(|text| serde_json::from_str(&text).expect("JSON"))
        .collect()
}

/// One turn of a bot holding `held` in an environment running `devices`; returns
/// every record. `edit` rewrites the laid-out environment before it runs.
fn one_turn(held: &[&str], devices: &[&str], edit: impl FnOnce(&Path)) -> Vec<Json> {
    let dir = layout(&[]);
    let environment =
        support::environment_with_devices(&dir, &["floppy"], devices, &[("floppy", held)], &[]);
    edit(&dir.join("environments/environment"));
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["floppy"]);
    run.event("bot.input", &input("ignore", "x"));
    let mut records = completed(&run);
    records.extend(run.close(WAIT).records);
    records
}

#[test]
fn the_bot_sends_its_environment_to_the_cognition() {
    // No devices: the environment's id, the bot's state and its (empty)
    // holdings.
    let records = one_turn(&[], &[], |_| {});
    assert_eq!(
        environments(&records),
        [
            json!({"id": "session", "self": {"state": "processing", "owned_devices": {}, "held_percepts": 0}})
        ]
    );
    // The lamp, with a nested object and a null in its data model.
    let records = one_turn(&["lamp"], &["lamp"], |environment| {
        let lamp = environment.join("lamp/lamp.scx");
        let text = std::fs::read_to_string(&lamp).expect("readable");
        std::fs::write(
            &lamp,
            text.replace(
                r#"<data id="click_db" expr="60"/>"#,
                r#"<data id="click_db" expr="60"/><data id="settings" expr="({color: {hue: 30}, owner: null})"/>"#,
            ),
        )
        .expect("writable");
    });
    assert_eq!(
        environments(&records),
        [json!({
            "id": "session",
            "self": {"state": "processing", "held_percepts": 0, "owned_devices": {"lamp": {
                "ref": "lamp",
                "id": "session.lamp",
                "state": "off",
                "attributes": {"brightness": 0, "click_db": 60, "settings": {"color": {"hue": 30}, "owner": null}},
            }}},
        })]
    );
}

#[test]
fn a_device_that_left_is_not_in_the_environment_block() {
    // The bot holds the lamp, whose session is not running: nothing is
    // read, and the lamp is left out.
    let records = one_turn(&["lamp"], &[], |_| {});
    assert_eq!(
        environments(&records),
        [
            json!({"id": "session", "self": {"state": "processing", "owned_devices": {}, "held_percepts": 0}})
        ]
    );
}

#[test]
fn a_held_device_whose_read_fails_drops_only_that_device() {
    // The bot holds `gone`, which the environment does not run, before the lamp:
    // nothing is read of `gone`, and only `gone` is left out.
    let records = one_turn(&["gone", "lamp"], &["lamp"], |_| {});
    let sent = environments(&records);
    assert_eq!(sent.len(), 1, "{records:?}");
    let owned = sent[0]["self"]["owned_devices"]
        .as_object()
        .expect("owned_devices");
    assert_eq!(owned.keys().collect::<Vec<_>>(), ["lamp"], "{sent:?}");
    assert_eq!(owned["lamp"]["ref"], json!("lamp"));
    assert_eq!(owned["lamp"]["id"], json!("session.lamp"));
}

#[test]
fn a_device_on_an_object_reaches_its_bot_with_what_it_sits_on() {
    let dir = layout(&[]);
    let environment = support::environment_with_devices(
        &dir,
        &["floppy"],
        &["lamp"],
        &[("floppy", &["lamp"])],
        &[],
    );
    // The lamp sits on the desk (D159).
    support::place(
        &environment,
        "lamp: {position: {x: 0, y: 0, z: 0}, on: 'desk'}",
    );
    let text = std::fs::read_to_string(&environment).expect("the environment is readable");
    std::fs::write(
        &environment,
        text.replacen(
            r#"<data id="objects" expr="[]"/>"#,
            r#"<data id="objects" expr="['desk']"/>"#,
            1,
        ),
    )
    .expect("the environment copy is writable");
    let mut run = Run::start(&[environment.display().to_string()], &[]);
    all_activated(&run, &["floppy"]);
    assert_eq!(
        data_of(&mut run, "session.floppy", "placed_on"),
        json!({"lamp": "desk"})
    );
    run.event("bot.input", &lamp_input("ignore", &json!("x")));
    let records = completed(&run);
    drop(run.close(WAIT));
    let environment: Json = serde_json::from_str(
        texts_of(&records, "session.floppy.cognition", "double.environment")
            .last()
            .expect("the turn's environment"),
    )
    .expect("JSON");
    assert_eq!(
        environment["self"]["owned_devices"]["lamp"]["on"],
        json!("desk")
    );
    assert_eq!(
        environment["self"]["owned_devices"]["lamp"]["ref"],
        json!("lamp")
    );
}
