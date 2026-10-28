//! The floppy bot's listening (`abilities/listening/listening.scx` with its children
//! `abilities/sensitivity/sensitivity.scx` and `abilities/interpretation/interpretation.scx`, and the
//! interpretation's hearing leaves), run through `scx run` on the wall
//! clock.
//!
//! Each test lays out a scratch `abilities/` directory with every schema,
//! the listening's documents, and `tests/fixtures/phase6/listening_harness.scx`
//! as the listening's parent (the body's place): the harness forwards each
//! external `environment.sound` and `bot.ability.speaking.efference` to the
//! listening and logs what it answers (`harness.ready`, `harness.speech`,
//! `harness.sound`, `harness.failed`). Voice detection, diarization, voice
//! identification and speech decoding run the real speech service against
//! `support::hearing`'s fake backend on 127.0.0.1, which records each
//! request and reads the words a test spelled into its audio; each run
//! grants the service a fresh media directory holding the test's audio
//! files and passes the backend's URL and a fake key by `--env-file`.
//!
//! A voiced sound opens the interpretation's open-speech window and a
//! sound without voice closes it, so each utterance is its voiced sounds
//! followed by [`hearing::silent_wav`]. Every test ends by
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
use support::hearing::{self, Backend, silent_wav, spoken_wav};
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
    // These tests pin the words-only pipeline: voices (diarization and
    // identification) and their tone (the affect classifier), the
    // interpretation's defaults, are on only where a test turns them on,
    // and so is the transcription's streaming, the listening's default:
    // the interpretation hears every sound.
    rewrite(&dir.join("listening/listening.scx"), &[("streams", "false")]);
    rewrite(
        &dir.join("interpretation/interpretation.scx"),
        &[
            ("diarizer", "false"),
            ("voice_classifier", "false"),
            ("affect_classifier", "false"),
        ],
    );
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

/// Starts the harness with a fresh fake backend and a media directory
/// holding `files`.
fn started(harness: &Path, test: &str, files: &[(&str, Vec<u8>)]) -> (Run, Backend) {
    let backend = Backend::start();
    let mut args = hearing::granted(test, &backend, files).args;
    args.push(harness.display().to_string());
    let run = Run::start(&args, &[]);
    run.until(WAIT, label_is("harness.ready"));
    (run, backend)
}

/// Closes the run and asserts its pinned trace.
fn close(run: Run, test: &str) -> Finished {
    let finished = run.close(WAIT);
    trace::assert_trace(test, &finished.all, &[]);
    finished
}

/// A 16 kHz mono WAV sound referencing `media/<name>`, with `extra`
/// fields.
fn sound(name: &str, extra: &Json) -> Json {
    let mut sound = json!({
        "audio": format!("media/{name}"),
        "media_type": "audio/wav",
        "sample_rate_hz": 16000,
        "channels": 1
    });
    if let (Some(fields), Some(more)) = (sound.as_object_mut(), extra.as_object()) {
        fields.extend(more.clone());
    }
    sound
}

/// Sends `voiced` then the closing silence `silent`.
fn utter(run: &mut Run, voiced: &Json, silent: &Json) {
    run.event("environment.sound", voiced);
    run.event("environment.sound", silent);
}

/// The speech product listening's interpretation makes of the utterance
/// that `sound` opened, decoded into `text`.
fn speech(text: &str, sound: &Json) -> Json {
    json!({
        "content": text,
        "content_type": "text/plain",
        "voice_detection": {"segments": []},
        "sample_rate_hz": sound["sample_rate_hz"],
        "channels": sound.get("channels").cloned().unwrap_or(json!(1)),
        "media_type": "audio/pcm",
        "source_ids": [],
        "participants": [],
        "parent": {"event": "environment.sound", "data": sound},
    })
}

/// The seconds `text` lasts when spelled at 16 kHz.
#[expect(
    clippy::cast_precision_loss,
    reason = "test texts are a few characters"
)]
fn spelled_seconds(text: &str) -> f64 {
    (text.len() * hearing::SAMPLES_PER_CHAR) as f64 / 16_000.0
}

#[test]
fn a_heard_utterance_is_decoded_into_a_speech_product() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "a_heard_utterance_is_decoded_into_a_speech_product",
        &[
            ("weather.wav", spoken_wav("what is the weather")),
            ("silence.wav", silent_wav()),
        ],
    );
    let voiced = sound("weather.wav", &json!({}));
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "a_heard_utterance_is_decoded_into_a_speech_product");
    assert_eq!(
        texts(&finished.all, "harness.ready"),
        vec!["listening".to_owned()]
    );
    let detected = backend.received_at("/vad");
    assert_eq!(detected.len(), 2, "each sound is voice-detected");
    assert_eq!(detected[0]["model"], "silero-vad");
    assert_eq!(detected[0]["keyed"], true, "the env-file key is the bearer");
    // The clip, between half a second of silence on each side.
    assert_eq!(
        detected[0]["samples"],
        19 * hearing::SAMPLES_PER_CHAR + 2 * 8_000
    );
    let decoded = backend.received_at("/audio/transcriptions");
    assert_eq!(decoded.len(), 1, "the utterance is decoded once");
    assert_eq!(decoded[0]["keyed"], true);
    // speech decoding's defaults: the OpenAI-compatible transcriptions API
    // with the lab's Nemotron model.
    assert_eq!(decoded[0]["model"], "nemotron-speech-streaming-en-0.6b");
    assert_eq!(decoded[0]["form"]["content_type"], "audio/wav");
    assert_eq!(decoded[0]["heard"], "what is the weather");
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("what is the weather", &voiced)]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
}

/// With the affect classifier, decoded speech carries the tone the
/// backend heard in its voice (mosfet.py's `SpeechData.affect`), its
/// segment sent as mono PCM at its own rate.
/// The transcription's speech product of a turn that `sound` opened and
/// the streaming session heard as `text`, over `seconds` of voice and the
/// silence that ended it.
fn streamed_speech(text: &str, sound: &Json, seconds: f64) -> Json {
    let mut product = speech(text, sound);
    product["voice_detection"] = json!({"segments": [{"start_seconds": 0, "end_seconds": seconds}]});
    product["confidence"] = json!(hearing::stream::CONFIDENCE);
    product
}

/// Streaming (the listening's default): each sound is streamed to the
/// streaming hearing session as it is heard, and the turn the session
/// says has ended is handed on as its words, with no voice detection or
/// decoding afterwards.
#[test]
fn a_streamed_turn_is_handed_on_as_its_words() {
    let test = "a_streamed_turn_is_handed_on_as_its_words";
    let harness = layout(&[("listening/listening.scx", &[("streams", "true")])]);
    let (mut run, backend) = started(
        &harness,
        test,
        &[
            ("weather.wav", spoken_wav("what is the weather")),
            ("silence.wav", silent_wav()),
        ],
    );
    let voiced = sound("weather.wav", &json!({}));
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, test);
    let streams = backend.streams();
    assert_eq!(streams.len(), 1, "one session hears every sound");
    assert_eq!(streams[0]["keyed"], true, "the env-file key is the bearer");
    assert_eq!(
        streams[0]["request_line"],
        "GET /v1/audio/stream?sample_rate=16000&language=en&chunk=0.56&turns=1 HTTP/1.1"
    );
    assert_eq!(streams[0]["heard"], json!(["what is the weather"]));
    assert_eq!(streams[0]["agent"], json!([false]), "told once that the bot is not producing");
    assert!(backend.received_at("/vad").is_empty(), "nothing is voice-detected afterwards");
    assert!(backend.decoded().is_empty(), "nothing is decoded afterwards");
    let seconds = spelled_seconds("what is the weather") + 0.1;
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![streamed_speech("what is the weather", &voiced, seconds)]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
}

/// A backend that does not stream: the listening hears through the
/// interpretation, which decodes the utterance once it has ended.
#[test]
fn without_a_streaming_session_the_utterance_is_decoded() {
    let test = "without_a_streaming_session_the_utterance_is_decoded";
    let harness = layout(&[("listening/listening.scx", &[("streams", "true")])]);
    let backend = Backend::start_without_stream();
    let files = [
        ("weather.wav", spoken_wav("what is the weather")),
        ("silence.wav", silent_wav()),
    ];
    let mut args = hearing::granted(test, &backend, &files).args;
    args.push(harness.display().to_string());
    let mut run = Run::start(&args, &[]);
    run.until(WAIT, label_is("harness.ready"));
    let voiced = sound("weather.wav", &json!({}));
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, test);
    assert_eq!(backend.streams().len(), 1, "the session was asked for once");
    assert_eq!(
        texts(&finished.all, "transcription"),
        vec!["no streaming session (unavailable): the listening hears through the interpretation".to_owned()]
    );
    assert_eq!(backend.decoded(), vec!["what is the weather".to_owned()]);
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("what is the weather", &voiced)]
    );
}

#[test]
fn decoded_speech_carries_the_tone_of_its_voice() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("affect_classifier", "true")],
    )]);
    let (mut run, backend) = started(
        &harness,
        "decoded_speech_carries_the_tone_of_its_voice",
        &[
            ("weather.wav", spoken_wav("what is the weather")),
            ("silence.wav", silent_wav()),
        ],
    );
    let voiced = sound("weather.wav", &json!({}));
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "decoded_speech_carries_the_tone_of_its_voice");
    let classified = backend.received_at("/emotion?sample_rate=16000");
    assert_eq!(classified.len(), 1, "the utterance's tone is heard once");
    assert_eq!(classified[0]["keyed"], true);
    assert_eq!(classified[0]["heard"], "what is the weather");
    let mut expected = speech("what is the weather", &voiced);
    expected["affect"] =
        json!({"label": "neutral", "scores": {"neutral": 1}, "valence": 0, "confidence": 1});
    assert_eq!(json_texts(&finished.all, "harness.speech"), vec![expected]);
    assert_eq!(
        texts(&finished.all, "listening.affect"),
        vec!["heard neutral 1".to_owned()]
    );
}

#[test]
fn a_speech_product_carries_the_participants_stamped_on_its_sounds() {
    let harness = layout(&[]);
    let (mut run, _backend) = started(
        &harness,
        "a_speech_product_carries_the_participants_stamped_on_its_sounds",
        &[
            ("weather.wav", spoken_wav("what is the weather")),
            ("silence.wav", silent_wav()),
        ],
    );
    // Producers stamp, transformers copy (decision D119): the
    // product carries the ordered union of what was stamped on the sounds
    // the utterance took, each once.
    let caller = json!({"ref": "person-42", "role": "source"});
    let bot = json!({"ref": "floppy", "role": "target"});
    let voiced = sound("weather.wav", &json!({"participants": [caller]}));
    let silent = sound("silence.wav", &json!({"participants": [caller, bot]}));
    utter(&mut run, &voiced, &silent);
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(
        run,
        "a_speech_product_carries_the_participants_stamped_on_its_sounds",
    );
    let mut expected = speech("what is the weather", &voiced);
    expected["participants"] = json!([caller, bot]);
    assert_eq!(json_texts(&finished.all, "harness.speech"), vec![expected]);
}

#[test]
fn the_open_speech_window_joins_voiced_sounds_into_one_utterance() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "the_open_speech_window_joins_voiced_sounds_into_one_utterance",
        &[
            ("hello.wav", spoken_wav("hello")),
            ("world.wav", spoken_wav(" world")),
            ("silence.wav", silent_wav()),
        ],
    );
    // The loudest voiced sound is the utterance's.
    let first = sound("hello.wav", &json!({"received_level_db": 40}));
    let second = sound("world.wav", &json!({"received_level_db": 50}));
    run.event("environment.sound", &first);
    utter(&mut run, &second, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(
        run,
        "the_open_speech_window_joins_voiced_sounds_into_one_utterance",
    );
    assert_eq!(backend.received_at("/vad").len(), 3);
    assert_eq!(backend.decoded(), vec!["hello world"]);
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("hello world", &second)]
    );
}

#[test]
fn labelled_non_speech_passes_through_as_environment_sound() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "labelled_non_speech_passes_through_as_environment_sound",
        &[("ring-1.wav", silent_wav())],
    );
    let ring =
        json!({"audio": "media/ring-1.wav", "media_type": "audio/wav", "kind": " phone.ringing "});
    run.event("environment.sound", &ring);
    run.until(WAIT, label_is("harness.sound"));
    let finished = close(
        run,
        "labelled_non_speech_passes_through_as_environment_sound",
    );
    // Handed off as the detection normalized it, its label kept.
    assert_eq!(
        json_texts(&finished.all, "harness.sound"),
        vec![json!({
            "audio": "media/floppy-sound-1.pcm",
            "media_type": "audio/pcm",
            "sample_rate_hz": 16000,
            "channels": 1,
            "kind": " phone.ringing "
        })]
    );
    assert!(
        backend.decoded().is_empty(),
        "a labelled sound is not decoded"
    );
    assert!(texts(&finished.all, "harness.speech").is_empty());
}

#[test]
fn an_unlabelled_sound_without_voice_is_dropped() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "an_unlabelled_sound_without_voice_is_dropped",
        &[
            ("hum.wav", silent_wav()),
            ("hello.pcm", hearing::pcm(&hearing::voiced("hello"))),
            ("silence.wav", silent_wav()),
        ],
    );
    // A blank kind labels nothing: without voice, the sound is dropped.
    run.event(
        "environment.sound",
        &sound("hum.wav", &json!({"kind": "  "})),
    );
    // Raw PCM in its own format is heard as it is.
    let voiced = json!({"audio": "media/hello.pcm", "media_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1});
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "an_unlabelled_sound_without_voice_is_dropped");
    assert_eq!(backend.received_at("/vad").len(), 3);
    assert_eq!(backend.decoded(), vec!["hello"]);
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("hello", &voiced)]
    );
    assert!(texts(&finished.all, "harness.sound").is_empty());
}

#[test]
fn a_decoding_failure_is_a_listening_failure() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "a_decoding_failure_is_a_listening_failure",
        &[
            ("again.wav", spoken_wav("again")),
            ("fail.wav", spoken_wav("fail")),
            ("silence.wav", silent_wav()),
        ],
    );
    let silent = sound("silence.wav", &json!({}));
    utter(&mut run, &sound("fail.wav", &json!({})), &silent);
    run.until(WAIT, label_is("harness.failed"));
    // The interpretation is Idle again: the next utterance is decoded.
    let next = sound("again.wav", &json!({}));
    utter(&mut run, &next, &silent);
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "a_decoding_failure_is_a_listening_failure");
    assert_eq!(
        json_texts(&finished.all, "harness.failed"),
        vec![
            json!({"stage": "speech_decoding", "message": "Speech decoding failed: the backend answered an error status."})
        ]
    );
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("again", &next)]
    );
    assert_eq!(backend.decoded(), vec!["fail", "again"]);
}

#[test]
fn speech_with_no_sample_rate_cannot_be_placed_on_a_timeline() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "speech_with_no_sample_rate_cannot_be_placed_on_a_timeline",
        &[
            ("hello.pcm", hearing::pcm(&hearing::voiced("hello"))),
            ("silence.pcm", hearing::pcm(&hearing::silence(1600))),
        ],
    );
    // Raw PCM with no format: detected as 16 kHz mono, joined as raw PCM,
    // decoded, and then refused.
    utter(
        &mut run,
        &json!({"audio": "media/hello.pcm"}),
        &json!({"audio": "media/silence.pcm"}),
    );
    run.until(WAIT, label_is("harness.failed"));
    let finished = close(
        run,
        "speech_with_no_sample_rate_cannot_be_placed_on_a_timeline",
    );
    assert_eq!(
        backend.received_at("/audio/transcriptions")[0]["form"]["content_type"],
        "audio/pcm"
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
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("stage_timeout_ms", "3000")],
    )]);
    let (mut run, backend) = started(
        &harness,
        "a_hanging_stage_times_out",
        &[
            ("hang.wav", spoken_wav("hang")),
            ("silence.wav", silent_wav()),
        ],
    );
    utter(
        &mut run,
        &sound("hang.wav", &json!({})),
        &sound("silence.wav", &json!({})),
    );
    run.until(WAIT, label_is("harness.failed"));
    let finished = close(run, "a_hanging_stage_times_out");
    assert_eq!(
        json_texts(&finished.all, "harness.failed"),
        vec![
            json!({"stage": "speech_decoding", "message": "Listening speech_decoding timed out."})
        ]
    );
    assert_eq!(backend.decoded(), vec!["hang"]);
}

#[test]
fn a_quiet_utterance_is_dropped_and_sounds_are_interpreted_in_order() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "a_quiet_utterance_is_dropped_and_sounds_are_interpreted_in_order",
        &[
            ("loud.wav", spoken_wav("loud")),
            ("quiet.wav", spoken_wav("quiet")),
            ("unmeasured.wav", spoken_wav("unmeasured")),
            ("silence.wav", silent_wav()),
        ],
    );
    // Under the 3 dB product threshold: decoded, then dropped.
    let quiet = sound("quiet.wav", &json!({"received_level_db": 2.5}));
    let loud = sound("loud.wav", &json!({"received_level_db": 3}));
    let unmeasured = sound("unmeasured.wav", &json!({}));
    let silent = sound("silence.wav", &json!({}));
    utter(&mut run, &quiet, &silent);
    utter(&mut run, &loud, &silent);
    utter(&mut run, &unmeasured, &silent);
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("unmeasured"))
    });
    let finished = close(
        run,
        "a_quiet_utterance_is_dropped_and_sounds_are_interpreted_in_order",
    );
    assert_eq!(backend.decoded(), vec!["quiet", "loud", "unmeasured"]);
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("loud", &loud), speech("unmeasured", &unmeasured)]
    );
}

#[test]
fn the_efference_copy_opens_a_window_that_closes_itself() {
    let harness = layout(&[]);
    let (mut run, _backend) = started(
        &harness,
        "the_efference_copy_opens_a_window_that_closes_itself",
        &[
            ("after.wav", spoken_wav("after")),
            ("cut-in.wav", spoken_wav("cut in")),
            ("silence.wav", silent_wav()),
        ],
    );
    // While the mouth produces, an arrival that is not off it (the sound
    // names no source, so its stimulus names none) is scored as unproduced:
    // all of it survives.
    run.event(
        "bot.ability.speaking.efference",
        &json!({"mouth": "mouth-1", "duration": 0.2}),
    );
    let cut_in = sound(
        "cut-in.wav",
        &json!({"amplitude_db": 60, "received_level_db": 54}),
    );
    let silent = sound("silence.wav", &json!({"received_level_db": 54}));
    utter(&mut run, &cut_in, &silent);
    run.until(WAIT, label_is("harness.speech"));
    // After the window closed, the same holds.
    std::thread::sleep(Duration::from_millis(400));
    let after = sound("after.wav", &json!({"received_level_db": 54}));
    utter(&mut run, &after, &silent);
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"].as_str().is_some_and(|t| t.contains("after"))
    });
    let finished = close(run, "the_efference_copy_opens_a_window_that_closes_itself");
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("cut in", &cut_in), speech("after", &after)]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
}

/// The bot's own speech, as speaking emits it (its source the mouth its
/// efference copy names, no levels to compare), is scored against the
/// prediction while the copy's window is open: fully predicted, it is not
/// interpreted at all (nothing of it is decoded, #30). A sound from no
/// source arriving in the same window is still heard.
#[test]
fn the_bots_own_speech_is_scored_as_the_mouths_and_not_interpreted() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "the_bots_own_speech_is_scored_as_the_mouths_and_not_interpreted",
        &[
            ("own.wav", spoken_wav("own voice")),
            ("cut-in.wav", spoken_wav("cut in")),
            ("silence.wav", silent_wav()),
        ],
    );
    run.event(
        "bot.ability.speaking.efference",
        &json!({"mouth": "speaking", "duration": 60}),
    );
    let own = sound("own.wav", &json!({"source": "speaking"}));
    let own_silence = sound("silence.wav", &json!({"source": "speaking"}));
    utter(&mut run, &own, &own_silence);
    let cut_in = sound("cut-in.wav", &json!({}));
    utter(&mut run, &cut_in, &sound("silence.wav", &json!({})));
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("cut in"))
    });
    let finished = close(
        run,
        "the_bots_own_speech_is_scored_as_the_mouths_and_not_interpreted",
    );
    assert_eq!(backend.decoded(), vec!["cut in"]);
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("cut in", &cut_in)]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
}

#[test]
fn malformed_sound_is_refused_before_the_listening() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "malformed_sound_is_refused_before_the_listening",
        &[
            ("after.wav", spoken_wav("after")),
            ("silence.wav", silent_wav()),
        ],
    );
    // No audio reference: environment.sound's schema refuses it where the
    // harness sends it, so the listening never hears it.
    run.event("environment.sound", &json!({"kind": "knock"}));
    let good = sound("after.wav", &json!({}));
    utter(&mut run, &good, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "malformed_sound_is_refused_before_the_listening");
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![speech("after", &good)]
    );
    assert_eq!(backend.received_at("/vad").len(), 2);
}

#[test]
fn without_a_decoder_or_classifier_a_sound_without_voice_is_an_acoustic_product() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("sound_classifier", "false"), ("speech_decoder", "false")],
    )]);
    let (mut run, backend) = started(
        &harness,
        "without_a_decoder_or_classifier_a_sound_without_voice_is_an_acoustic_product",
        &[("hum.wav", silent_wav())],
    );
    let hum = sound("hum.wav", &json!({}));
    run.event("environment.sound", &hum);
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(
        run,
        "without_a_decoder_or_classifier_a_sound_without_voice_is_an_acoustic_product",
    );
    // The sound's PCM, written beside it, with no voice spans.
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![json!({
            "content": "media/floppy-sound-1.pcm",
            "content_type": "audio/pcm",
            "voice_detection": {"segments": []},
            "sample_rate_hz": 16000,
            "channels": 1,
            "media_type": "audio/pcm",
            "source_ids": [],
            "participants": [],
            "parent": {"event": "environment.sound", "data": hum},
        })]
    );
    assert!(backend.decoded().is_empty());
}

/// The voice a product names, by the ref the interpretation's registry
/// minted for it (`voice:` and 32 hex digits): a print never leaves it.
fn voice_ref(product: &Json) -> Json {
    let sources = product["source_ids"].as_array().expect("source ids");
    assert_eq!(sources.len(), 1, "one voice: {product}");
    let reference = sources[0].as_str().expect("a voice is named by its ref");
    let id = reference.strip_prefix("voice:").expect("a voice ref");
    assert!(
        id.len() == 32
            && id
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "{reference}"
    );
    sources[0].clone()
}

/// The speech product of diarized segment `index` of an utterance that
/// `opened` began and that spelled `text`, identified as `voice` (its ref).
fn segment(index: usize, text: &str, opened: &Json, voice: Option<&Json>) -> Json {
    let end = spelled_seconds(text);
    let source_ids = voice.map_or(json!([]), |voice| json!([voice]));
    json!({
        "content": format!("media/floppy-diarized-2-{index}"),
        "content_type": "audio/pcm",
        "media_type": "audio/pcm",
        "sample_rate_hz": 16000,
        "channels": 1,
        "start_seconds": 0,
        "end_seconds": end,
        "confidence": null,
        "voice_detection": {"segments": [{"start_seconds": 0, "end_seconds": end, "confidence": null}]},
        "source_ids": source_ids,
        "participants": [],
        "parent": {"event": "environment.sound", "data": opened},
    })
}

#[test]
fn a_diarized_utterance_is_one_speech_product_per_segment() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("speech_decoder", "false"), ("diarizer", "true")],
    )]);
    let (mut run, backend) = started(
        &harness,
        "a_diarized_utterance_is_one_speech_product_per_segment",
        &[("hi.wav", spoken_wav("hi")), ("silence.wav", silent_wav())],
    );
    let voiced = sound("hi.wav", &json!({}));
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(
        run,
        "a_diarized_utterance_is_one_speech_product_per_segment",
    );
    let diarized = backend.received_at("/audio/diarization");
    assert_eq!(diarized.len(), 1);
    assert_eq!(diarized[0]["model"], "pyannote-diarization");
    assert_eq!(diarized[0]["heard"], "hi");
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![segment(0, "hi", &voiced, None)]
    );
    assert!(backend.decoded().is_empty());
}

#[test]
fn diarized_segments_are_identified_by_their_voice() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[
            ("speech_decoder", "false"),
            ("diarizer", "true"),
            ("voice_classifier", "true"),
        ],
    )]);
    let (mut run, backend) = started(
        &harness,
        "diarized_segments_are_identified_by_their_voice",
        &[("hi.wav", spoken_wav("hi")), ("silence.wav", silent_wav())],
    );
    let voiced = sound("hi.wav", &json!({}));
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "diarized_segments_are_identified_by_their_voice");
    let embedded = backend.received_at("/voice/embed");
    assert_eq!(embedded.len(), 1, "one embedding per segment");
    assert_eq!(embedded[0]["model"], "voice-detect-wespeaker-resnet34");
    assert_eq!(embedded[0]["heard"], "hi");
    let speech = json_texts(&finished.all, "harness.speech");
    let voice = voice_ref(&speech[0]);
    assert_eq!(speech, vec![segment(0, "hi", &voiced, Some(&voice))]);
}

#[test]
fn without_diarization_the_opening_voice_is_identified_directly() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("speech_decoder", "false"), ("voice_classifier", "true")],
    )]);
    let (mut run, backend) = started(
        &harness,
        "without_diarization_the_opening_voice_is_identified_directly",
        &[("hi.wav", spoken_wav("hi")), ("silence.wav", silent_wav())],
    );
    let voiced = sound("hi.wav", &json!({}));
    let silent = sound("silence.wav", &json!({}));
    utter(&mut run, &voiced, &silent);
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("floppy-sound-2"))
    });
    let finished = close(
        run,
        "without_diarization_the_opening_voice_is_identified_directly",
    );
    assert_eq!(backend.received_at("/voice/embed").len(), 1);
    // The window streams each frame as the opening voice's speech.
    let speech = json_texts(&finished.all, "harness.speech");
    let voice = voice_ref(&speech[0]);
    let frame = |serial: usize, spans: Json, parent: &Json| {
        json!({
            "content": format!("media/floppy-sound-{serial}.pcm"),
            "content_type": "audio/pcm",
            "voice_detection": {"segments": spans},
            "sample_rate_hz": 16000,
            "channels": 1,
            "media_type": "audio/pcm",
            "source_ids": [voice],
            "participants": [],
            "parent": {"event": "environment.sound", "data": parent},
        })
    };
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![
            frame(
                1,
                json!([{"start_seconds": 0, "end_seconds": spelled_seconds("hi"), "confidence": null}]),
                &voiced
            ),
            frame(2, json!([]), &silent),
        ]
    );
}

/// A parent of the sensitivity alone (`abilities/harness.scx`): it
/// forwards each external `bot.ability.listening.sensitivity.score` and
/// `bot.ability.speaking.efference` to it and logs each output
/// (`harness.scored`, its `producing` and `perceived_level_db`).
const SENSITIVITY_HARNESS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="harness" initial="run">
  <state id="run">
    <invoke id="sensitivity" type="scxml" src="sensitivity/sensitivity.scx"/>
    <transition event="bot.ability.ready" cond="_event.invokeid === 'sensitivity'">
      <log label="harness.ready" expr="'sensitivity'"/>
    </transition>
    <transition event="bot.ability.listening.sensitivity.output" cond="_event.invokeid === 'sensitivity'">
      <log label="harness.scored" expr="_event.data.sound.audio + ' producing ' + _event.data.producing + ' at ' + _event.data.perceived_level_db"/>
    </transition>
    <transition event="bot.ability.listening.sensitivity.score bot.ability.speaking.efference" cond="_event.invokeid === undefined">
      <send eventexpr="_event.name" target="#_sensitivity">
        <content expr="_event.data"/>
      </send>
    </transition>
  </state>
</scxml>
"##;

/// Every scored arrival says whether the body was producing, and only a
/// copy with sound still to come opens the window: a silent copy (duration
/// 0) closes it, and opens nothing (decision D177).
#[test]
fn a_scored_sound_says_whether_the_body_was_producing_and_a_silent_copy_closes_the_window() {
    let dir = unique_scratch("sensitivity").join("abilities");
    support::copy_ability("sensitivity", &dir);
    let harness = dir.join("harness.scx");
    std::fs::write(&harness, SENSITIVITY_HARNESS).expect("the harness is writable");
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    run.until(WAIT, label_is("harness.ready"));
    let score = |run: &mut Run, name: &str, source: Option<&str>| {
        run.event(
            "bot.ability.listening.sensitivity.score",
            &json!({"stimulus": {"event": "environment.sound", "data": {"audio": format!("media/{name}"), "received_level_db": 54}, "source": source}}),
        );
        run.until(WAIT, label_is("harness.scored"));
    };
    let copy = |run: &mut Run, duration: f64| {
        run.event(
            "bot.ability.speaking.efference",
            &json!({"mouth": "speaking", "duration": duration}),
        );
    };
    score(&mut run, "quiet", None);
    // A silent copy opens nothing.
    copy(&mut run, 0.0);
    score(&mut run, "still-quiet", None);
    copy(&mut run, 60.0);
    score(&mut run, "cut-in", None);
    score(&mut run, "own", Some("speaking"));
    // The mouth has nothing left to play: the window closes now.
    copy(&mut run, 0.0);
    score(&mut run, "closed", None);
    let finished = run.close(WAIT);
    trace::assert_trace(
        "a_scored_sound_says_whether_the_body_was_producing_and_a_silent_copy_closes_the_window",
        &finished.all,
        &[],
    );
    assert_eq!(
        texts(&finished.all, "harness.scored"),
        vec![
            "media/quiet producing false at 54",
            "media/still-quiet producing false at 54",
            "media/cut-in producing true at 54",
            "media/own producing true at 0",
            "media/closed producing false at 54",
        ]
    );
}

/// Nobody identified in time: the opening voice's identification
/// completes with no voice once the identification deadline passes
/// (decision D177), and the window opens and streams its frames with no
/// source identity instead of failing the listening.
#[test]
fn an_opening_voice_nobody_identifies_in_time_is_heard_unidentified() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[
            ("speech_decoder", "false"),
            ("voice_classifier", "true"),
            ("stage_timeout_ms", "10000"),
            ("recognize_timeout_ms", "500"),
        ],
    )]);
    let (mut run, backend) = started(
        &harness,
        "an_opening_voice_nobody_identifies_in_time_is_heard_unidentified",
        &[
            ("hang.wav", spoken_wav("hang")),
            ("silence.wav", silent_wav()),
        ],
    );
    utter(
        &mut run,
        &sound("hang.wav", &json!({})),
        &sound("silence.wav", &json!({})),
    );
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("floppy-sound-2"))
    });
    let finished = close(
        run,
        "an_opening_voice_nobody_identifies_in_time_is_heard_unidentified",
    );
    assert_eq!(backend.received_at("/voice/embed").len(), 1);
    assert_eq!(
        texts(&finished.all, "interpretation.unidentified"),
        vec!["1 segment(s) unidentified after 10500 ms"]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
    let speech = json_texts(&finished.all, "harness.speech");
    assert_eq!(speech.len(), 2, "{speech:?}");
    for product in &speech {
        assert_eq!(product["source_ids"], json!([]), "{product}");
        assert!(product.get("voice_embedding").is_none(), "{product}");
    }
}

/// Diarized segments nobody identifies in time are each handed off with
/// no source and no embedding (decision D177).
#[test]
fn diarized_segments_nobody_identifies_in_time_are_heard_unidentified() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[
            ("speech_decoder", "false"),
            ("diarizer", "true"),
            ("voice_classifier", "true"),
            ("stage_timeout_ms", "10000"),
            ("recognize_timeout_ms", "500"),
        ],
    )]);
    let (mut run, backend) = started(
        &harness,
        "diarized_segments_nobody_identifies_in_time_are_heard_unidentified",
        &[
            ("hang.wav", spoken_wav("hang")),
            ("silence.wav", silent_wav()),
        ],
    );
    let voiced = sound("hang.wav", &json!({}));
    utter(&mut run, &voiced, &sound("silence.wav", &json!({})));
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(
        run,
        "diarized_segments_nobody_identifies_in_time_are_heard_unidentified",
    );
    assert_eq!(backend.received_at("/voice/embed").len(), 1);
    assert_eq!(
        texts(&finished.all, "interpretation.unidentified"),
        vec!["1 segment(s) unidentified after 10500 ms"]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
    assert_eq!(
        json_texts(&finished.all, "harness.speech"),
        vec![segment(0, "hang", &voiced, None)]
    );
}

/// A voice identification that stands in for the real one and answers no
/// embedding: the real one answers one per segment or fails, so only a
/// child that answers `embeddings: []` reaches this branch.
const NO_EMBEDDING_IDENTIFICATION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="voice_identification" initial="applied">
  <datamodel>
    <data id="segments" expr="null"/>
    <data id="audio" expr="null"/>
    <data id="sample_rate_hz" expr="null"/>
    <data id="channels" expr="null"/>
    <data id="start_seconds" expr="null"/>
    <data id="end_seconds" expr="null"/>
    <data id="output" expr="null"/>
  </datamodel>
  <final id="applied">
    <donedata>
      <content expr="({segments: [], embeddings: []})"/>
    </donedata>
  </final>
</scxml>
"#;

/// An opening voice identified with no embedding opens the window with
/// nobody identified, and no stage fails (decision D177).
#[test]
fn an_opening_voice_with_no_embedding_is_heard_unidentified() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("speech_decoder", "false"), ("voice_classifier", "true")],
    )]);
    std::fs::write(
        harness
            .parent()
            .expect("the harness has a directory")
            .join("voice_identification/voice_identification.scx"),
        NO_EMBEDDING_IDENTIFICATION,
    )
    .expect("the stand-in is writable");
    let (mut run, backend) = started(
        &harness,
        "an_opening_voice_with_no_embedding_is_heard_unidentified",
        &[("hi.wav", spoken_wav("hi")), ("silence.wav", silent_wav())],
    );
    utter(
        &mut run,
        &sound("hi.wav", &json!({})),
        &sound("silence.wav", &json!({})),
    );
    run.until(WAIT, |record| {
        label_is("harness.speech")(record)
            && record["text"]
                .as_str()
                .is_some_and(|t| t.contains("floppy-sound-2"))
    });
    let finished = close(
        run,
        "an_opening_voice_with_no_embedding_is_heard_unidentified",
    );
    assert!(backend.received_at("/voice/embed").is_empty());
    assert_eq!(
        texts(&finished.all, "interpretation.unidentified"),
        vec!["no embedding for the opening voice"]
    );
    assert!(texts(&finished.all, "harness.failed").is_empty());
    let speech = json_texts(&finished.all, "harness.speech");
    assert_eq!(speech.len(), 2, "{speech:?}");
    for product in &speech {
        assert_eq!(product["source_ids"], json!([]), "{product}");
        assert!(product.get("voice_embedding").is_none(), "{product}");
    }
}

#[test]
fn a_negative_product_threshold_degrades_the_listening() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("product_threshold_db", "-1")],
    )]);
    let backend = Backend::start();
    let mut args = hearing::granted(
        "a_negative_product_threshold_degrades_the_listening",
        &backend,
        &[],
    )
    .args;
    args.push(harness.display().to_string());
    let run = Run::start(&args, &[]);
    // The listening degrades on the interpretation's end, a step after
    // the interpretation's log: the run closes once it has, or the trace
    // holds it only when the close is slow enough.
    run.until(WAIT, label_is("listening.degraded"));
    let finished = close(run, "a_negative_product_threshold_degrades_the_listening");
    assert_eq!(
        texts(&finished.all, "interpretation.degraded"),
        vec!["product_threshold_db must not be negative"]
    );
    assert!(texts(&finished.all, "harness.ready").is_empty());
}

#[test]
fn with_voices_decoded_speech_says_who_spoke() {
    // The interpretation's defaults: diarization and voice identification
    // with speech decoding.
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("diarizer", "true"), ("voice_classifier", "true")],
    )]);
    let (mut run, backend) = started(
        &harness,
        "with_voices_decoded_speech_says_who_spoke",
        &[
            ("weather.wav", spoken_wav("what is the weather")),
            ("silence.wav", silent_wav()),
        ],
    );
    utter(
        &mut run,
        &sound("weather.wav", &json!({})),
        &sound("silence.wav", &json!({})),
    );
    run.until(WAIT, label_is("harness.speech"));
    let finished = close(run, "with_voices_decoded_speech_says_who_spoke");
    assert_eq!(backend.received_at("/audio/diarization").len(), 1);
    assert_eq!(backend.received_at("/voice/embed").len(), 1);
    let heard = json_texts(&finished.all, "harness.speech");
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0]["content"], "what is the weather");
    voice_ref(&heard[0]);
}

/// The interpretation remembers the voices it hears as the people long-term
/// memory holds (decision D258): a voice heard before a restart is heard
/// as the same voice after it.
#[test]
fn a_voice_heard_before_a_restart_is_recognized_after_it() {
    let harness = layout(&[(
        "interpretation/interpretation.scx",
        &[("diarizer", "true"), ("voice_classifier", "true")],
    )]);
    rewrite(&harness, &[("database", "'/memory/floppy.sqlite'")]);
    let memory = unique_scratch("memory");
    let grant = vec![
        "--dir".to_owned(),
        format!(
            "{}={}::/memory",
            support::service_uri("sql"),
            memory.display()
        ),
    ];
    let hear = |run: &mut Run| {
        utter(
            run,
            &sound("weather.wav", &json!({})),
            &sound("silence.wav", &json!({})),
        );
        let heard = run.until(WAIT, label_is("harness.speech"));
        let text = texts(&heard, "harness.speech")
            .pop()
            .expect("a speech product");
        voice_ref(&serde_json::from_str(&text).expect("the log is JSON"))
    };
    let files = [
        ("weather.wav", spoken_wav("what is the weather")),
        ("silence.wav", silent_wav()),
    ];
    let backend = Backend::start();
    let mut args = hearing::granted("a_voice_heard_before_a_restart", &backend, &files).args;
    args.extend(grant.clone());
    args.push(harness.display().to_string());
    let mut run = Run::start(&args, &[]);
    run.until(WAIT, label_is("harness.ready"));
    let first = hear(&mut run);
    // Heard again in the same run, the voice is the one minted, and the
    // new person was remembered before the second utterance was taken.
    assert_eq!(hear(&mut run), first);
    run.close(WAIT);

    let mut args = hearing::granted(
        "a_voice_heard_before_a_restart_is_recognized_after_it",
        &backend,
        &files,
    )
    .args;
    args.extend(grant);
    args.push(harness.display().to_string());
    let mut run = Run::start(&args, &[]);
    run.until(WAIT, label_is("harness.ready"));
    assert_eq!(
        hear(&mut run),
        first,
        "the voice is recognized after a restart"
    );
    close(run, "a_voice_heard_before_a_restart_is_recognized_after_it");
}

#[test]
fn a_voice_detection_failure_is_a_listening_failure() {
    let harness = layout(&[]);
    let (mut run, backend) = started(
        &harness,
        "a_voice_detection_failure_is_a_listening_failure",
        &[],
    );
    // The audio is not in the granted directory: the service cannot read
    // it, and the interpretation answers the failure and is Idle again.
    run.event("environment.sound", &sound("missing.wav", &json!({})));
    run.until(WAIT, label_is("harness.failed"));
    let finished = close(run, "a_voice_detection_failure_is_a_listening_failure");
    let failed = json_texts(&finished.all, "harness.failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["stage"], "voice_detection");
    assert!(
        failed[0]["message"]
            .as_str()
            .is_some_and(|message| message.ends_with("failed: the audio cannot be read.")),
        "{failed:?}"
    );
    assert!(backend.received().is_empty(), "nothing reaches the backend");
}
