//! Two floppy bots in one environment (`environments/room/room.scx`)
//! against real backends, on the wall clock. Ignored by default:
//!
//! `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p floppy-tests --test
//! environment_live -- --ignored`
//!
//! The env files must hold `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (the
//! generate service), `TYPESAFE_API_KEY` (the intuition's decision
//! service), `FLOPPY_SPEECH_BASE_URL` and `FLOPPY_SPEECH_API_KEY` (speaking
//! and speech decoding) and `FLOPPY_VOICE_BASE_URL` and
//! `FLOPPY_VOICE_API_KEY` (voice detection: without it the listening fails
//! `voice_detection` and hands over no product, decision D98); the model
//! is `FLOPPY_LIVE_MODEL`, else `Qwen/Qwen3.8-Flash-Next`, given to both
//! bots as their `model` param. Both bots' speech services share one media
//! grant, so `b` reads the audio `a` wrote, and the files service the
//! layout's behaviors directory, where the builder reflection invokes
//! writes a learned behavior's documents. The run's records are written
//! under `target/floppy-live/`; only pass or fail is printed, never a
//! value.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use support::{Run, unique_scratch, workspace};

/// The variables the run needs, by name (their values are never read).
const NEEDED: [&str; 7] = [
    "FLOPPY_BASE_URL",
    "FLOPPY_API_KEY",
    "TYPESAFE_API_KEY",
    "FLOPPY_SPEECH_BASE_URL",
    "FLOPPY_SPEECH_API_KEY",
    "FLOPPY_VOICE_BASE_URL",
    "FLOPPY_VOICE_API_KEY",
];

/// The names `NEEDED` lists that no env file sets (reading names only).
fn missing_variables() -> Vec<&'static str> {
    let mut set = Vec::new();
    for file in support::live_env_files() {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|_| panic!("the env file {} is readable", file.display()));
        for line in text.lines() {
            if let Some((name, value)) = line.split_once('=')
                && !value.trim().is_empty()
            {
                set.push(name.trim().to_owned());
            }
        }
    }
    NEEDED
        .into_iter()
        .filter(|name| !set.iter().any(|set| set == name))
        .collect()
}

fn keep(name: &str, records: &[Json]) {
    let out = workspace().join("target/floppy-live");
    std::fs::create_dir_all(&out).expect("the output directory is creatable");
    std::fs::write(
        out.join(format!("{name}.json")),
        serde_json::to_string_pretty(records).expect("JSON"),
    )
    .expect("writable");
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

/// A scratch copy of the repository's bot, abilities, and the builder
/// reflection invokes, with the room invoking bots `a` and `b` given the
/// live model.
fn room() -> std::path::PathBuf {
    let dir = unique_scratch("environment-live");
    support::copy_tree(&workspace().join("bots/floppy"), &dir.join("bots/floppy"));
    support::copy_tree(&workspace().join("bots/builder"), &dir.join("bots/builder"));
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    let model = format!("'{}'", support::live_model());
    support::environment(&dir, &["a", "b"], &[("model", &model)])
}

/// Two bots in one room: `a` is asked to say something, speaks it, and `b`
/// hears it, decodes it and takes a turn on it; `a` does not take its own
/// speech as a turn (its efference copy withholds it, D98).
#[test]
#[ignore = "makes real model, decision, speech and voice calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_two_participants() {
    let missing = missing_variables();
    assert!(
        missing.is_empty(),
        "the env files set no value for {missing:?}"
    );
    let room = room();
    let media = unique_scratch("environment-live-media");
    std::fs::write(media.join("silence.wav"), support::hearing::silent_wav())
        .expect("the media directory is writable");
    let mut args = Vec::new();
    for file in support::live_env_files() {
        args.push("--env-file".to_owned());
        args.push(file.display().to_string());
    }
    args.extend(support::speech_grant(&media));
    // The builder (reflection's) writes and reads its documents in the
    // layout's behaviors directory (decision D121).
    args.extend(support::behaviors_grant(&room));
    args.push(room.display().to_string());
    let started = Instant::now();
    let mut run = Run::start(&args, &[]);
    let mut all = Vec::new();
    for _ in 0..2 {
        all.extend(run.until(Duration::from_mins(5), |record| {
            record["type"] == "log" && record["label"] == "bot.activated"
        }));
    }
    run.event(
        "bot.ability.communication.input",
        &json!({
            "source_ids": ["gabe"],
            "target_ids": ["a"],
            "content": "Please say out loud, in one short sentence, that the window is open.",
            "content_type": "text/plain"
        }),
    );
    // a's turn on the request and its speech; then the silence that ends
    // the utterance (the open-speech window closes on a sound without
    // voice, the original's VAD End), as
    // a_bots_speech_reaches_the_other_participant sends it; then b's turn
    // on what it heard (b's floppy.output), at most 15 minutes in all.
    let heard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut records = run.until(Duration::from_mins(10), |record| {
            record["type"] == "log" && record["label"] == "bot.ability.speaking.output"
        });
        run.event(
            "environment.sound",
            &json!({"audio": "media/silence.wav", "media_type": "audio/wav"}),
        );
        records.extend(run.until(Duration::from_mins(5), |record| {
            record["type"] == "log"
                && record["label"] == "floppy.output"
                && record["session"] == "session.b"
        }));
        records
    }));
    if let Ok(records) = heard {
        all.extend(records);
    }
    let finished = run.close(Duration::from_mins(1));
    all.extend(finished.records);
    keep("live_two_participants", &all);
    let a_said = texts_of(&all, "session.a.communication.speaking", "floppy.say");
    let a_turns = texts_of(&all, "session.a", "floppy.output").len();
    let b_turns = texts_of(&all, "session.b", "floppy.output").len();
    let pass = a_said.len() == 1 && a_turns == 1 && b_turns >= 1;
    println!(
        "live two participants: {} in {} s",
        if pass { "pass" } else { "fail" },
        started.elapsed().as_secs()
    );
    assert!(
        pass,
        "a said {} time(s) and took {a_turns} turn(s); b took {b_turns} turn(s) (target/floppy-live/live_two_participants.json)",
        a_said.len()
    );
}
