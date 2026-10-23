//! floppy's `TurnDetector` (`abilities/turn_detector/turn_detector.scx`)
//! run through `scx run` on the wall clock under
//! `tests/fixtures/turn_detector/turn_detector_harness.scx`, which forwards
//! each external turn event and heard speech to it and logs what it
//! answers (`harness.ready`, `harness.complete`, `harness.failed`, their
//! data as JSON). No service is involved.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Run, logs, unique_scratch, workspace};

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(300);

const START: &str = "bot.ability.turn_detector.turn.start";
const UPDATE: &str = "bot.ability.turn_detector.turn.update";
const PAUSE: &str = "bot.ability.turn_detector.turn.pause";
const END: &str = "bot.ability.turn_detector.turn.end";
const SPEECH: &str = "bot.ability.listening.speech.output";

/// Lays out the detector and the harness with an end-of-turn silence of
/// `silence` seconds; returns the harness.
fn layout(silence: &str) -> PathBuf {
    let dir = unique_scratch("turn_detector");
    let abilities = dir.join("abilities");
    support::copy_ability("turn_detector", &abilities);
    let harness = std::fs::read_to_string(
        workspace().join("tests/fixtures/turn_detector/turn_detector_harness.scx"),
    )
    .expect("the harness is readable")
    .replace(
        r#"<data id="silence" expr="60"/>"#,
        &format!(r#"<data id="silence" expr="{silence}"/>"#),
    );
    let path = abilities.join("harness.scx");
    std::fs::write(&path, harness).expect("the harness is writable");
    path
}

/// A started harness, once the detector announced itself.
fn started(harness: &Path) -> (Run, Json) {
    let run = Run::start(&[harness.display().to_string()], &[]);
    let records = run.until(WAIT, |record| is_answer(record, "harness.ready"));
    let ready = answers(&records, "harness.ready");
    (
        run,
        ready.into_iter().next().expect("the detector is ready"),
    )
}

fn is_answer(record: &Json, label: &str) -> bool {
    record["type"] == "log" && record["label"] == label
}

/// The next answer, completed or failed.
fn next_answer(run: &Run) -> (String, Json) {
    let records = run.until(WAIT, |record| {
        is_answer(record, "harness.complete")
            || is_answer(record, "harness.failed")
            || is_answer(record, "harness.refused")
    });
    let (label, text) = logs(&records).pop().expect("one answer");
    let data = serde_json::from_str(&text).unwrap_or(Json::String(text));
    (label, data)
}

/// The data of the logs labelled `label`, parsed.
fn answers(records: &[Json], label: &str) -> Vec<Json> {
    logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| serde_json::from_str(&text).expect("an answer is JSON"))
        .collect()
}

fn text(source: &str, content: &str) -> Json {
    json!({"kind": "text", "source_participant_ref": source, "content": content})
}

#[test]
fn a_started_turn_assembles_its_content_until_it_ends() {
    let harness = layout("60");
    let (mut run, ready) = started(&harness);
    assert_eq!(
        ready,
        json!({"participant_ref": "bot", "conversation_ref": "conversation"})
    );
    let turn = |turn_ref: &str| json!({"conversation_ref": "call", "turn_ref": turn_ref});

    // Text is stripped and joined; an update, pause or end for another
    // turn is ignored; a pause keeps the turn open.
    run.event(
        START,
        &json!({"conversation_ref": "call", "turn_ref": "t1", "self_participant_ref": "bot", "source_participant_ref": "caller", "content": text("caller", "  hello ")}),
    );
    run.event(
        UPDATE,
        &json!({"conversation_ref": "call", "turn_ref": "t1", "content": text("caller", "world")}),
    );
    run.event(
        UPDATE,
        &json!({"conversation_ref": "call", "turn_ref": "t2", "content": text("caller", "ignored")}),
    );
    run.event(PAUSE, &turn("t1"));
    run.event(END, &turn("t2"));
    run.event(
        END,
        &json!({"conversation_ref": "call", "turn_ref": "t1", "content": text("caller", "again ")}),
    );
    assert_eq!(
        next_answer(&run),
        (
            "harness.complete".to_owned(),
            json!({
                "conversation_ref": "call", "turn_ref": "t1", "participant_ref": "bot",
                "self_participant_ref": "bot", "source_participant_ref": "caller",
                "text": "hello world again", "audio": [], "content": "hello world again",
                "content_type": "text/plain", "sample_rate_hz": null, "channels": null
            })
        )
    );

    // A start for another participant opens nothing; modality-neutral
    // content is handed on as given.
    run.event(
        START,
        &json!({"turn_ref": "t3", "self_participant_ref": "other", "content": text("caller", "not mine")}),
    );
    run.event(END, &json!({"turn_ref": "t3"}));
    run.event(
        START,
        &json!({"turn_ref": "t4", "content": {"kind": "content", "source_participant_ref": "caller", "content": {"a": [1, null]}, "content_type": "application/x-sign-language"}}),
    );
    run.event(END, &json!({"turn_ref": "t4"}));
    let (label, data) = next_answer(&run);
    assert_eq!(label, "harness.complete");
    assert_eq!(data["turn_ref"], "t4");
    assert_eq!(data["conversation_ref"], "conversation");
    assert_eq!(data["content"], json!({"a": [1, null]}));
    assert_eq!(data["content_type"], "application/x-sign-language");

    // An event stimulus is its name and payload.
    run.event(
        START,
        &json!({"turn_ref": "t5", "content": {"kind": "event", "source_participant_ref": "caller", "event": "phone.ring"}}),
    );
    run.event(END, &json!({"turn_ref": "t5"}));
    let (_, data) = next_answer(&run);
    assert_eq!(
        data["content"],
        json!({"event": "phone.ring", "payload": {}})
    );
    assert_eq!(data["content_type"], "application/event");

    // Audio is kept by reference, in order, with the first chunk's format.
    run.event(
        START,
        &json!({"turn_ref": "t6", "source_participant_ref": "alice", "content": {"kind": "audio", "source_participant_ref": "alice", "content": "media/a.pcm", "sample_rate_hz": 16000, "channels": 1}}),
    );
    run.event(
        UPDATE,
        &json!({"turn_ref": "t6", "source_participant_ref": "alice", "content": {"kind": "audio", "source_participant_ref": "alice", "content": "media/b.pcm", "sample_rate_hz": 8000, "channels": 2}}),
    );
    run.event(
        END,
        &json!({"turn_ref": "t6", "source_participant_ref": "alice"}),
    );
    let (_, data) = next_answer(&run);
    assert_eq!(data["source_participant_ref"], "alice");
    assert_eq!(data["audio"], json!(["media/a.pcm", "media/b.pcm"]));
    assert_eq!(data["content"], json!(["media/a.pcm", "media/b.pcm"]));
    assert_eq!(data["content_type"], "audio/pcm");
    assert_eq!(data["text"], "");
    assert_eq!(data["sample_rate_hz"], 16000);
    assert_eq!(data["channels"], 1);

    let finished = run.close(WAIT);
    assert_eq!(
        answers(&finished.all, "harness.complete").len(),
        4,
        "the ignored turns answered nothing"
    );
    assert!(answers(&finished.all, "harness.failed").is_empty());
}

#[test]
fn a_turn_with_nothing_to_show_or_both_text_and_audio_fails_and_the_detector_goes_on() {
    let harness = layout("60");
    let (mut run, _ready) = started(&harness);
    run.event(
        START,
        &json!({"conversation_ref": "call", "turn_ref": "t1", "content": text("caller", "   ")}),
    );
    run.event(END, &json!({"conversation_ref": "call", "turn_ref": "t1"}));
    assert_eq!(
        next_answer(&run),
        (
            "harness.failed".to_owned(),
            json!({
                "stage": "turn",
                "message": "Turn closed with neither text, audio, nor modality-neutral content.",
                "participant_ref": "bot", "conversation_ref": "call", "turn_ref": "t1",
                "source_participant_ref": "caller"
            })
        )
    );
    run.event(
        START,
        &json!({"turn_ref": "t2", "content": text("caller", "hi")}),
    );
    run.event(
        END,
        &json!({"turn_ref": "t2", "content": {"kind": "audio", "source_participant_ref": "caller", "content": "media/a.pcm"}}),
    );
    let (label, data) = next_answer(&run);
    assert_eq!(label, "harness.failed");
    assert_eq!(
        data["message"],
        "Turn product cannot contain both text and audio."
    );
    // Idle again: the next turn completes.
    run.event(
        START,
        &json!({"turn_ref": "t3", "content": text("caller", "ok")}),
    );
    run.event(END, &json!({"turn_ref": "t3"}));
    let (label, data) = next_answer(&run);
    assert_eq!(label, "harness.complete");
    assert_eq!(data["text"], "ok");
    // A start whose text stimulus is empty is refused by its schema
    // before it reaches the detector.
    run.event(
        START,
        &json!({"turn_ref": "t4", "content": text("caller", "")}),
    );
    let (label, data) = next_answer(&run);
    assert_eq!(label, "harness.refused");
    assert_eq!(data, json!("error.execution"));
    let _finished = run.close(WAIT);
}

/// Heard speech: `content` and its `content_type`, with voice spans or
/// none.
fn speech(content: &str, content_type: &str, voiced: bool) -> Json {
    let segments = if voiced {
        json!([{"start_seconds": 0.0, "end_seconds": 0.2, "confidence": 0.9}])
    } else {
        json!([])
    };
    json!({
        "content": content, "content_type": content_type, "voice_detection": {"segments": segments},
        "sample_rate_hz": 16000, "media_type": "audio/pcm", "channels": 1, "source_ids": []
    })
}

#[test]
fn heard_speech_opens_a_turn_and_the_end_of_turn_silence_closes_it() {
    let harness = layout("1");
    let (mut run, _ready) = started(&harness);
    // Voiced frames open and feed a turn, an unvoiced one pauses it, and
    // the silence closes it.
    run.event(SPEECH, &speech("media/a.pcm", "audio/pcm", true));
    run.event(SPEECH, &speech("media/b.pcm", "audio/pcm", true));
    run.event(SPEECH, &speech("media/c.pcm", "audio/pcm", false));
    let (label, data) = next_answer(&run);
    assert_eq!(label, "harness.complete");
    assert_eq!(
        data,
        json!({
            "conversation_ref": "conversation", "turn_ref": "turn-1", "participant_ref": "bot",
            "self_participant_ref": "bot", "source_participant_ref": "caller", "text": "",
            "audio": ["media/a.pcm", "media/b.pcm"], "content": ["media/a.pcm", "media/b.pcm"],
            "content_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1
        })
    );
    // Unvoiced speech opens nothing; decoded words carry no frames, so a
    // turn they open closes with nothing to show.
    run.event(SPEECH, &speech("media/d.pcm", "audio/pcm", false));
    run.event(SPEECH, &speech("hello there", "text/plain", true));
    let (label, data) = next_answer(&run);
    assert_eq!(label, "harness.failed");
    assert_eq!(data["turn_ref"], "turn-2");
    // A started turn also closes on silence.
    run.event(
        START,
        &json!({"turn_ref": "t1", "content": text("caller", "hi")}),
    );
    let (label, data) = next_answer(&run);
    assert_eq!(label, "harness.complete");
    assert_eq!(data["turn_ref"], "t1");
    assert_eq!(data["text"], "hi");
    let _finished = run.close(WAIT);
}
