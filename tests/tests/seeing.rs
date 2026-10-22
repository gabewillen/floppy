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
//! (`tests/fixtures/total-due.png`) as `frame.png`. Seeing's own suite
//! (`abilities/seeing/irp`, run by `document_irp.rs`) covers what seeing
//! does with what it is sent; this covers only what the body sends it and
//! takes from it.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Run, logs, service_uri, unique_scratch, workspace};

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

/// Lays out the body with the cognition double and the input double (the
/// body's `input` set to `['identity']`), and returns the body's path.
fn layout() -> PathBuf {
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
    let body_path = body_dir.join("floppy.scx");
    std::fs::write(
        &body_path,
        body.replacen(declared, r#"<data id="input" expr="['identity']"/>"#, 1),
    )
    .expect("the body copy is writable");
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
    std::fs::create_dir_all(abilities.join("cognition")).expect("creatable");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/cognition_double.scx"),
        abilities.join("cognition/cognition.scx"),
    )
    .expect("the cognition double is copyable");
    body_path
}

/// Starts the room with the laid-out body as its one bot, `params` its
/// activation settings, and the vision service granted a media directory
/// holding `frame.png`.
fn start(body: &Path, params: &[(&str, &str)]) -> Run {
    let media = unique_scratch("seeing-media");
    std::fs::copy(
        workspace().join("tests/fixtures/total-due.png"),
        media.join("frame.png"),
    )
    .expect("the frame is copyable");
    let args = vec![
        "--dir".to_owned(),
        format!("{}={}::/media", service_uri("vision"), media.display()),
        support::environment_for(body, params),
    ];
    Run::start(&args, &[])
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
    let body = layout();
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
    let body = layout();
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
