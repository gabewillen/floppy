//! The floppy bot's listening (`abilities/listening.scx` with its children
//! `abilities/sensitivity.scx` and `abilities/interpretation.scx`, and the
//! interpretation's hearing leaves), run through `scx run` on the wall
//! clock.
//!
//! Each test lays out a scratch `abilities/` directory with every schema,
//! the listening's documents, and `tests/fixtures/phase6/listening_harness.scx`
//! as the listening's parent (the body's place): the harness forwards each
//! external `environment.sound` and `bot.ability.speaking.efference` to the
//! listening and logs what it answers (`harness.ready`, `harness.speech`,
//! `harness.sound`, `harness.failed`). The speech service is replaced by
//! `tests/fixtures/phase6/speech_double.scx`, a double of its decoding
//! events (`support::copy_listening`), which logs each request as
//! `speech.decode` and decodes an audio reference's file name into words.
//!
//! The seam leaves log `voice_detection` `absent`. Every test ends by
//! comparing its whole run with its pinned trace under
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
const WAIT: Duration = Duration::from_secs(60);

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

/// Lays out the listening under the harness with `edits` (`(document,
/// [(id, expr)])`); returns the harness.
fn layout(edits: &[(&str, &[(&str, &str)])]) -> PathBuf {
    let dir = unique_scratch("listening").join("abilities");
    copy_dir(&workspace().join("abilities/schemas"), &dir.join("schemas"));
    support::copy_listening(&dir);
    for (document, data) in edits {
        rewrite(&dir.join(document), data);
    }
    let harness = dir.join("harness.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase6/listening_harness.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    harness
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

fn json_texts(records: &[Json], label: &str) -> Vec<Json> {
    texts(records, label)
        .iter()
        .map(|text| serde_json::from_str(text).expect("the log is JSON"))
        .collect()
}

fn started(harness: &Path) -> Run {
    let run = Run::start(&[harness.display().to_string()], &[]);
    run.until(WAIT, label_is("harness.ready"));
    run
}

/// Closes the run and asserts its pinned trace.
fn close(run: Run, test: &str) -> Finished {
    let finished = run.close(WAIT);
    trace::assert_trace(test, &finished.all, &[]);
    finished
}

/// The speech product mosfet's interpretation makes of `sound` decoded
/// into `text`.
fn speech(text: &str, sound: &Json) -> Json {
    json!({
        "content": text,
        "content_type": "text/plain",
        "voice_detection": {"segments": []},
        "sample_rate_hz": sound["sample_rate_hz"],
        "channels": sound.get("channels").cloned().unwrap_or(json!(1)),
        "media_type": "audio/pcm",
        "source_ids": [],
        "parent": {"event": "environment.sound", "data": sound},
    })
}

#[test]
fn a_heard_sound_is_decoded_into_a_speech_product() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    let sound = json!({
        "audio": "media/what-is-the-weather.wav",
        "media_type": "audio/wav",
        "sample_rate_hz": 16000,
        "channels": 1
    });
    run.event("environment.sound", &sound);
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "a_heard_sound_is_decoded_into_a_speech_product");
    assert_eq!(
        json_texts(&finished.all, "harness.ready"),
        vec![json!({"ability": "listening", "choices": []})]
    );
    assert_eq!(texts(&finished.all, "voice_detection"), vec!["absent"]);
    assert_eq!(
        json_texts(&finished.all, "speech.decode"),
        vec![json!({
            "operation": "decode",
            "model": "whisper-1",
            "audio": "media/what-is-the-weather.wav",
            "media_type": "audio/wav"
        })]
    );
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("what is the weather", &sound)]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
}

#[test]
fn labelled_non_speech_passes_through_as_environment_sound() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    let ring = json!({"audio": "media/ring-1.wav", "kind": " phone.ringing "});
    run.event("environment.sound", &ring);
    run.until(WAIT, label_is("harness.sound"));
    let finished = close(
        run,
        "labelled_non_speech_passes_through_as_environment_sound",
    );
    assert_eq!(json_texts(&finished.all, "harness.sound"), vec![ring]);
    assert!(
        texts(&finished.all, "speech.decode").is_empty(),
        "a labelled sound is not decoded"
    );
    assert!(texts(&finished.all, "harness.speech").is_empty());
}

#[test]
fn a_blank_kind_is_unlabelled_and_decoded() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    let sound = json!({"audio": "media/hello.pcm", "media_type": "audio/pcm", "sample_rate_hz": 48000, "kind": "  "});
    run.event("environment.sound", &sound);
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "a_blank_kind_is_unlabelled_and_decoded");
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("hello", &sound)]
    );
}

#[test]
fn a_decoding_failure_is_a_listening_failure() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    run.event(
        "environment.sound",
        &json!({"audio": "media/fail.wav", "sample_rate_hz": 16000}),
    );
    run.until(WAIT, label_is("harness.failed"));
    // The interpretation is Idle again: the next sound is decoded.
    let next = json!({"audio": "media/again.wav", "sample_rate_hz": 16000});
    run.event("environment.sound", &next);
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "a_decoding_failure_is_a_listening_failure");
    assert_eq!(
        json_texts(&finished.all, "harness.failed"),
        vec![json!({"stage": "speech_decoding", "message": "unconfigured"})]
    );
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("again", &next)]
    );
}

#[test]
fn speech_with_no_sample_rate_cannot_be_placed_on_a_timeline() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    run.event("environment.sound", &json!({"audio": "media/hello.wav"}));
    run.until(WAIT, label_is("harness.failed"));
    let finished = close(
        run,
        "speech_with_no_sample_rate_cannot_be_placed_on_a_timeline",
    );
    assert_eq!(
        json_texts(&finished.all, "harness.failed"),
        vec![json!({
            "stage": "speech_decoding",
            "message": "Speech decoding completed on sound with no sample rate to place it on a timeline."
        })]
    );
    assert!(texts(&finished.all, "harness.speech").is_empty());
}

#[test]
fn a_hanging_stage_times_out() {
    let harness = layout(&[("interpretation.scx", &[("stage_timeout_ms", "500")])]);
    let mut run = started(&harness);
    run.event(
        "environment.sound",
        &json!({"audio": "media/hang.wav", "sample_rate_hz": 16000}),
    );
    run.until(WAIT, label_is("harness.failed"));
    let finished = close(run, "a_hanging_stage_times_out");
    assert_eq!(
        json_texts(&finished.all, "harness.failed"),
        vec![
            json!({"stage": "speech_decoding", "message": "Listening speech_decoding timed out."})
        ]
    );
}

#[test]
fn a_quiet_arrival_is_dropped_and_sounds_are_interpreted_in_order() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    // Under the 3 dB product threshold: decoded, then dropped.
    let quiet =
        json!({"audio": "media/quiet.wav", "sample_rate_hz": 16000, "received_level_db": 2.5});
    let loud = json!({"audio": "media/loud.wav", "sample_rate_hz": 16000, "received_level_db": 3});
    let unmeasured = json!({"audio": "media/unmeasured.wav", "sample_rate_hz": 16000});
    run.event("environment.sound", &quiet);
    run.event("environment.sound", &loud);
    run.event("environment.sound", &unmeasured);
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("unmeasured"))
    });
    let finished = close(
        run,
        "a_quiet_arrival_is_dropped_and_sounds_are_interpreted_in_order",
    );
    assert_eq!(
        json_texts(&finished.all, "speech.decode")
            .iter()
            .map(|request| request["audio"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("media/quiet.wav"),
            json!("media/loud.wav"),
            json!("media/unmeasured.wav")
        ]
    );
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("loud", &loud), speech("unmeasured", &unmeasured)]
    );
}

#[test]
fn the_efference_copy_opens_a_window_that_closes_itself() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    // While the mouth produces, an arrival that is not off it (the
    // listening's stimulus names no source) is scored as unproduced: all of
    // it survives.
    run.event(
        "bot.ability.speaking.efference",
        &json!({"mouth": "mouth-1", "duration": 0.2}),
    );
    let sound = json!({"audio": "media/cut-in.wav", "sample_rate_hz": 16000, "amplitude_db": 60, "received_level_db": 54});
    run.event("environment.sound", &sound);
    run.until(WAIT, label_is("harness.speech"));
    // After the window closed, the same holds.
    std::thread::sleep(Duration::from_millis(400));
    let after =
        json!({"audio": "media/after.wav", "sample_rate_hz": 16000, "received_level_db": 54});
    run.event("environment.sound", &after);
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"].as_str().is_some_and(|t| t.contains("after"))
    });
    let finished = close(run, "the_efference_copy_opens_a_window_that_closes_itself");
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("cut in", &sound), speech("after", &after)]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
}

#[test]
fn malformed_sound_is_refused_before_the_listening() {
    let harness = layout(&[]);
    let mut run = started(&harness);
    // No audio reference: environment.sound's schema refuses it where the
    // harness sends it, so the listening never hears it.
    run.event("environment.sound", &json!({"kind": "knock"}));
    let good = json!({"audio": "media/after.wav", "sample_rate_hz": 16000});
    run.event("environment.sound", &good);
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "malformed_sound_is_refused_before_the_listening");
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("after", &good)]
    );
    assert_eq!(texts(&finished.all, "voice_detection"), vec!["absent"]);
}
