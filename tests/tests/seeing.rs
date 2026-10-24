//! The floppy bot's seeing switch (`bots/floppy/floppy.scx`'s
//! `seeing_enabled`, decision D189) with the eye, `abilities/seeing/seeing.scx`,
//! run through `scx run` on the wall clock as the one participant
//! (`floppy`) of the room (`environments/room/room.scx`), which activates
//! it on joining.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! and its schemas, the abilities' schemas, the communication with its
//! conversation and speaking, the listening, and seeing with its stage
//! children) with `tests/fixtures/phase4/cognition_double.scx` as the
//! cognition, which answers a sight's turn with an ignore and logs
//! `double.input` (`<turn> ignore "<cause>"`), `double.choices` and
//! `double.participants`, and `tests/fixtures/phase4/input_double.scx` as
//! the one input ability (`input` is `['identity']`), which logs
//! `input.visual` for each `environment.visual` the body fans out to it.
//! Seeing logs `seeing.view`, `seeing.measure` and `seeing.sight`.
//!
//! The vision service is granted a scratch media directory holding a PNG
//! (`tests/fixtures/total-due.png`) as `frame.png`. The lifecycle tests put
//! `tests/fixtures/phase4/seeing_double.scx` in seeing's place: it is
//! ready unless its `mode` is `'silent'`, and the first `environment.visual` it
//! is sent ends it (it logs `double.end`), so the body gets
//! `done.invoke.seeing` once it is active; with `mode` `'ending'` it ends
//! right after its readiness, while the body activates.
//!
//! Seeing's own suite (`abilities/seeing/irp`, run by `document_irp.rs`)
//! covers what seeing does with what it is sent; this covers what the body
//! sends it and takes from it, and the one path that suite cannot reach:
//! the vision service answers within its `<invoke>`, so a measurement past
//! `measure_timeout_ms` is driven here, on the logical clock, by a copy of
//! seeing whose vision `<invoke>` reaches
//! `tests/fixtures/phase4/vision_double.scx`, which never answers.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Run, logs, service_uri, unique_scratch, workspace};

/// What the layout puts at `abilities/seeing/seeing.scx`.
enum Seeing {
    /// The repository's seeing.
    Real,
    /// The seeing double, with `mode`'s expression replaced (`'ready'`,
    /// `'silent'` or `'ending'`).
    Double(&'static str),
}

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(300);

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("the directory is creatable");
    for entry in std::fs::read_dir(from).expect("the directory is readable") {
        let path = entry.expect("a directory entry").path();
        std::fs::copy(&path, to.join(path.file_name().expect("a file name")))
            .expect("the file is copyable");
    }
}

/// Lays out the body with the cognition double, the input double (the
/// body's `input` set to `['identity']`) and `seeing`, its `<data>`
/// expressions then replaced per `data` (`(id, expr)`), and returns the
/// body's path.
fn layout(seeing: &Seeing, data: &[(&str, &str)]) -> PathBuf {
    let dir = unique_scratch("seeing");
    let body_dir = dir.join("bots/floppy");
    copy_dir(
        &workspace().join("bots/floppy/schemas"),
        &body_dir.join("schemas"),
    );
    let body = std::fs::read_to_string(workspace().join("bots/floppy/floppy.scx"))
        .expect("the body is readable");
    let declared = r#"<data id="input" expr="['identity', 'listening']"/>"#;
    assert!(body.contains(declared), "the body declares its input");
    let mut body = body.replacen(declared, r#"<data id="input" expr="['identity']"/>"#, 1);
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
        "seeing",
    ] {
        support::copy_ability(name, &abilities);
    }
    support::copy_listening(&abilities);
    support::double_speech(&abilities, "'encode'");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/input_double.scx"),
        abilities.join("identity/identity.scx"),
    )
    .expect("the input double is copyable");
    support::copy_cognition_double(&abilities);
    if let Seeing::Double(mode) = seeing {
        let double =
            std::fs::read_to_string(workspace().join("tests/fixtures/phase4/seeing_double.scx"))
                .expect("the seeing double is readable");
        let declared = r#"<data id="mode" expr="'ready'"/>"#;
        assert!(
            double.contains(declared),
            "the seeing double declares its mode"
        );
        std::fs::write(
            abilities.join("seeing/seeing.scx"),
            double.replacen(declared, &format!(r#"<data id="mode" expr="{mode}"/>"#), 1),
        )
        .expect("the seeing double copy is writable");
    }
    body_path
}

/// Starts the room with the laid-out body as its one bot, `params` its
/// activation settings, and, when the layout binds it (the real seeing
/// does, the double does not), the vision service granted a media
/// directory holding `frame.png`.
fn start(body: &Path, params: &[(&str, &str)]) -> Run {
    let mut args = Vec::new();
    if support::binds(body, "vision") {
        let media = unique_scratch("seeing-media");
        std::fs::copy(
            workspace().join("tests/fixtures/total-due.png"),
            media.join("frame.png"),
        )
        .expect("the frame is copyable");
        args.extend([
            "--dir".to_owned(),
            format!("{}={}::/media", service_uri("vision"), media.display()),
        ]);
    }
    args.push(support::environment_for(body, params));
    Run::start(&args, &[])
}

/// The data of the last `label` log, as JSON.
fn last_json(records: &[Json], label: &str) -> Json {
    let text = texts(records, label)
        .pop()
        .unwrap_or_else(|| panic!("a {label} log"));
    serde_json::from_str(&text).expect("the logged data is JSON")
}

fn label_is(label: &'static str) -> impl Fn(&Json) -> bool {
    move |record| record["type"] == "log" && record["label"] == label
}

/// The texts of the `label` logs, in order.
fn texts(records: &[Json], label: &str) -> Vec<String> {
    logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .collect()
}

/// The `(label, text)` logs whose label starts with `prefix`, in order.
fn starting(records: &[Json], prefix: &str) -> Vec<(String, String)> {
    logs(records)
        .into_iter()
        .filter(|(label, _)| label.starts_with(prefix))
        .collect()
}

fn input(action: &str, data: &str) -> Json {
    json!({"observation": {"event": action, "data": data}})
}

/// The events of the choices one `floppy.choices` log lists.
fn choice_events(text: &str) -> Vec<String> {
    let choices: Json = serde_json::from_str(text).expect("the choices are JSON");
    choices
        .as_array()
        .expect("the choices are an array")
        .iter()
        .map(|choice| choice["event"].as_str().expect("an event").to_owned())
        .collect()
}

#[test]
fn with_seeing_off_a_visual_reaches_only_the_input_abilities_and_starts_no_turn() {
    let body = layout(&Seeing::Real, &[]);
    let mut run = start(&body, &[]);
    run.until(WAIT, label_is("bot.activated"));
    let visual =
        json!({"image": "media/frame.png", "media_type": "image/png", "source": "camera-1"});
    run.event("environment.visual", &visual);
    run.until(WAIT, label_is("input.visual"));
    // The visual starts no turn: the next turn is the first, and its
    // choices hold no look.
    run.event("bot.input", &input("ignore", "after"));
    run.until(WAIT, label_is("bot.processing.completed"));
    let finished = run.close(WAIT);
    assert_eq!(
        texts(&finished.all, "input.visual"),
        vec![visual.to_string()],
        "the input ability gets the visual"
    );
    assert_eq!(
        texts(&finished.all, "double.input"),
        vec![r#"1 ignore "after""#],
        "the only turn is the bot.input's"
    );
    assert!(
        starting(&finished.all, "seeing.").is_empty(),
        "nothing reaches seeing: {:?}",
        starting(&finished.all, "seeing.")
    );
    let choices = texts(&finished.all, "floppy.choices");
    assert_eq!(choices.len(), 1, "one turn's choices");
    assert_eq!(
        choice_events(&choices[0]),
        vec![
            "bot.ability.cognition.ignore",
            "bot.ability.communication.respond"
        ],
        "seeing's offer is not taken"
    );
}

#[test]
fn with_seeing_on_a_visual_with_a_source_becomes_a_turn_whose_stimulus_is_a_sight() {
    let body = layout(&Seeing::Real, &[]);
    let mut run = start(&body, &[("seeing_enabled", "true")]);
    run.until(WAIT, label_is("bot.activated"));
    // A visual without a source is dropped: no measurement, no turn.
    let unsourced = json!({"image": "media/frame.png", "media_type": "image/png"});
    run.event("environment.visual", &unsourced);
    run.until(WAIT, label_is("seeing.view"));
    let participant = json!({"ref": "person-42", "role": "source"});
    let visual = json!({
        "image": "media/frame.png",
        "media_type": "image/png",
        "kind": "camera",
        "source": "camera-1",
        "participants": [participant]
    });
    run.event("environment.visual", &visual);
    run.until(WAIT, label_is("bot.processing.completed"));
    let finished = run.close(WAIT);
    // Both visuals reach the input ability as well.
    assert_eq!(
        texts(&finished.all, "input.visual"),
        vec![unsourced.to_string(), visual.to_string()]
    );
    assert_eq!(
        starting(&finished.all, "seeing."),
        support::labelled(&[
            ("seeing.view", "dropped"),
            ("seeing.view", "held"),
            ("seeing.measure", "first"),
            ("seeing.sight", "change"),
        ])
    );
    // The sight is the turn's stimulus, its participants the view's.
    assert_eq!(
        texts(&finished.all, "double.input"),
        vec![r#"1 ignore "change""#]
    );
    assert_eq!(
        texts(&finished.all, "double.participants"),
        vec![json!([participant]).to_string()]
    );
    // The turn's choices include seeing's look, tagged with its session.
    let choices: Json = serde_json::from_str(
        texts(&finished.all, "floppy.choices")
            .first()
            .expect("the turn's choices"),
    )
    .expect("the choices are JSON");
    let look = choices
        .as_array()
        .expect("the choices are an array")
        .iter()
        .find(|choice| choice["event"] == "bot.ability.seeing.look")
        .expect("the look is offered");
    assert_eq!(look["target"], "session.floppy.seeing");
    assert_eq!(look["schema"]["additionalProperties"], false);
}

#[test]
fn with_seeing_on_activation_waits_for_its_readiness() {
    // The silent double is never ready: the activation times out naming it.
    let body = layout(
        &Seeing::Double("'silent'"),
        &[("activation_timeout_ms", "3000")],
    );
    let run = start(&body, &[("seeing_enabled", "true")]);
    let records = run.until(WAIT, label_is("bot.activating.failed"));
    assert_eq!(
        texts(&records, "floppy.diagnostic"),
        vec![r#"the abilities ["seeing"] were not ready within 3000 ms"#],
        "{records:?}"
    );
    assert!(texts(&records, "bot.activated").is_empty(), "{records:?}");
    run.close(WAIT);
}

#[test]
fn with_seeing_on_its_ending_degrades_the_bot() {
    let body = layout(&Seeing::Double("'ready'"), &[]);
    let mut run = start(&body, &[("seeing_enabled", "true")]);
    // The bot activates once the double, expected while seeing is on, has
    // offered; the visual the body then sends it ends it.
    run.until(WAIT, label_is("bot.activated"));
    let visual =
        json!({"image": "media/frame.png", "media_type": "image/png", "source": "camera-1"});
    run.event("environment.visual", &visual);
    run.until(WAIT, label_is("done.invoke"));
    let finished = run.close(WAIT);
    assert_eq!(texts(&finished.all, "double.end"), vec!["seeing"]);
    assert_eq!(
        texts(&finished.all, "floppy.degraded"),
        vec!["the ability seeing ended while active"]
    );
    // degraded is a final sink: the bot's session ends with its reason,
    // and the bot leaves its environment.
    assert_eq!(
        last_json(&finished.all, "done.invoke"),
        json!({"invokeid": "floppy", "data": {"reason": "the ability seeing ended while active"}})
    );
}

#[test]
fn with_seeing_off_its_ending_while_activating_is_ignored() {
    // The ending double is ready and ends at once, while the body still
    // waits for its other abilities: the activation does not fail.
    let body = layout(&Seeing::Double("'ending'"), &[]);
    let mut run = start(&body, &[]);
    run.until(WAIT, label_is("bot.activated"));
    run.event("bot.input", &input("ignore", "after"));
    run.until(WAIT, label_is("bot.processing.completed"));
    let finished = run.close(WAIT);
    let order: Vec<String> = logs(&finished.all)
        .into_iter()
        .filter(|(label, _)| label == "double.end" || label == "bot.activated")
        .map(|(label, _)| label)
        .collect();
    assert_eq!(
        order,
        vec!["double.end", "bot.activated"],
        "seeing ended while the bot activated"
    );
    assert!(
        texts(&finished.all, "bot.activating.failed").is_empty(),
        "{:?}",
        finished.all
    );
    assert_eq!(
        texts(&finished.all, "double.input"),
        vec![r#"1 ignore "after""#]
    );
}

#[test]
fn with_seeing_off_its_ending_is_ignored() {
    // Off, the body sends seeing no visual of its own, so this layout also
    // lists it among the input abilities: the visual sent once the bot is
    // active then reaches it and ends it.
    let body = layout(
        &Seeing::Double("'ready'"),
        &[("input", "['identity', 'seeing']")],
    );
    let mut run = start(&body, &[]);
    run.until(WAIT, label_is("bot.activated"));
    let visual =
        json!({"image": "media/frame.png", "media_type": "image/png", "source": "camera-1"});
    run.event("environment.visual", &visual);
    run.until(WAIT, label_is("double.end"));
    // The bot is still active: it takes the next turn.
    run.event("bot.input", &input("ignore", "after"));
    run.until(WAIT, label_is("bot.processing.completed"));
    let finished = run.close(WAIT);
    assert_eq!(texts(&finished.all, "double.end"), vec!["seeing"]);
    assert_eq!(
        texts(&finished.all, "double.input"),
        vec![r#"1 ignore "after""#]
    );
    assert!(
        texts(&finished.all, "floppy.degraded").is_empty(),
        "{:?}",
        finished.all
    );
    assert!(
        texts(&finished.all, "done.invoke").is_empty(),
        "the bot stays in its environment: {:?}",
        finished.all
    );
}

/// The vision service `<invoke>` opening tag in `abilities/seeing/seeing.scx`.
const VISION_INVOKE: &str =
    r#"<invoke id="vision" type="https://github.com/stateforward/floppy/services/vision">"#;

/// A harness that invokes the seeing beside it and, once it is ready, sends
/// it a view and a look, then logs the sight it hands on (`harness.sight`)
/// and ends.
const TIMEOUT_HARNESS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="harness">
  <state id="harness">
    <invoke id="seeing" type="scxml" src="seeing.scx"/>
    <transition event="bot.ability.ready">
      <send event="environment.visual" target="#_seeing">
        <content expr="({image: 'media/frame.png', media_type: 'image/png', source: 'camera-1'})"/>
      </send>
      <send event="bot.ability.seeing.look" target="#_seeing">
        <content expr="({})"/>
      </send>
    </transition>
    <transition event="bot.ability.cognition.input" target="done">
      <log label="harness.sight" expr="JSON.stringify(_event.data.stimulus.data)"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>
"##;

#[test]
fn a_measurement_past_measure_timeout_ms_hands_nothing_on() {
    let abilities = unique_scratch("seeing-timeout").join("abilities");
    let seeing = support::copy_ability("seeing", &abilities);
    let document = std::fs::read_to_string(&seeing).expect("seeing is readable");
    assert!(
        document.contains(VISION_INVOKE),
        "abilities/seeing/seeing.scx invokes the vision service"
    );
    std::fs::write(
        &seeing,
        document.replacen(
            VISION_INVOKE,
            r#"<invoke id="vision" type="scxml" src="vision_double.scx">"#,
            1,
        ),
    )
    .expect("the seeing copy is writable");
    let dir = seeing.parent().expect("seeing's directory");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/vision_double.scx"),
        dir.join("vision_double.scx"),
    )
    .expect("the vision double is copyable");
    let harness = support::write(dir, "harness.scx", TIMEOUT_HARNESS);
    let mut run = Run::start(
        &[
            "--clock".to_owned(),
            "logical".to_owned(),
            harness.display().to_string(),
        ],
        &[],
    );
    run.end_input();
    let finished = run.finish(WAIT);
    assert!(
        support::finished(&finished.all),
        "the harness ends: {:?}\n{}",
        finished.all,
        finished.stderr
    );
    // The measurement never answers: its bound (2000 ms) passes first,
    // nothing is handed on, and the eye paces (500 ms) before it serves the
    // look, which still sees the view.
    assert_eq!(
        texts(&finished.all, "double.measure"),
        vec!["byte_identity"]
    );
    assert_eq!(texts(&finished.all, "seeing.measure"), vec!["timeout"]);
    assert_eq!(
        last_json(&finished.all, "harness.sight"),
        json!({
            "cause": "look",
            "views": [{
                "image": "media/frame.png",
                "media_type": "image/png",
                "kind": null,
                "participants": [],
                "change": null,
                "age_ms": 2500,
                "objects": null,
                "description": null
            }],
            "participants": []
        })
    );
}
