//! The floppy bot (`bots/floppy/floppy.scx`) against real backends, on the
//! wall clock. Every test here is ignored by default:
//!
//! `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p floppy-tests --test
//! floppy_live -- --ignored`
//!
//! The env files must hold `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (the
//! generate service has no default backend) and `TYPESAFE_API_KEY` (the
//! intuition's decision service); the model is
//! `FLOPPY_LIVE_MODEL`, else `Qwen/Qwen3.8-Flash-Next`, given to the bot
//! by `bot.activate`. Each test writes the run's records under
//! `target/floppy-live/` and prints pass or fail only; no value is ever
//! printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::time::Duration;

use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};

use support::{Run, logs, unique_scratch, workspace};

/// A live run's records, written for inspection.
fn keep(name: &str, records: &[Json]) {
    let out = workspace().join("target/floppy-live");
    std::fs::create_dir_all(&out).expect("the output directory is creatable");
    std::fs::write(
        out.join(format!("{name}.json")),
        serde_json::to_string_pretty(records).expect("JSON"),
    )
    .expect("writable");
}

fn start() -> Run {
    start_body(&workspace().join("bots/floppy/floppy.scx"))
}

fn start_body(body: &Path) -> Run {
    let mut args = Vec::new();
    for file in support::live_env_files() {
        args.push("--env-file".to_owned());
        args.push(file.display().to_string());
    }
    args.push(body.display().to_string());
    Run::start(&args, &[])
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

/// A scratch copy of the bot whose body's `devices` expression is
/// `devices`; returns the body's path.
fn with_devices(devices: &str) -> PathBuf {
    let dir = unique_scratch("floppy-live");
    for sub in [
        "bots/floppy",
        "bots/floppy/schemas",
        "abilities",
        "abilities/schemas",
    ] {
        copy_dir(&workspace().join(sub), &dir.join(sub));
    }
    let body = dir.join("bots/floppy/floppy.scx");
    let text = std::fs::read_to_string(&body).expect("the body is readable");
    let declared = r#"<data id="devices" expr="[]"/>"#;
    assert!(text.contains(declared), "the body declares its devices");
    std::fs::write(
        &body,
        text.replace(
            declared,
            &format!(r#"<data id="devices" expr="{devices}"/>"#),
        ),
    )
    .expect("the body copy is writable");
    body
}

fn texts(records: &[Json], label: &str) -> Vec<String> {
    logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .collect()
}

/// Every selection of the turn's output was routed, and none dropped.
fn all_routed(records: &[Json], output: Option<&Json>) -> bool {
    let selected = output
        .and_then(|output| output["output"].as_array())
        .map_or(usize::MAX, Vec::len);
    texts(records, "bot.route").len() == selected && texts(records, "bot.route.dropped").is_empty()
}

fn model() -> String {
    std::env::var("FLOPPY_LIVE_MODEL").unwrap_or_else(|_| "Qwen/Qwen3.8-Flash-Next".into())
}

/// One question turn through the full cascade: autonomy's seam leaves it
/// unhandled, intuition (the decision service on `TypeSafe`) hands it to
/// reasoning or abstains, and reasoning (the model) selects from the
/// body's assembled choices, which the body routes. (No ability offers
/// `bot.ability.communication.respond` until communication exists, so the
/// reply is not checked here.) The env files must also hold
/// `TYPESAFE_API_KEY`.
#[test]
#[ignore = "makes real decision and model calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_cascade() {
    let mut run = start();
    run.event("bot.activate", &json!({"model": model()}));
    run.event(
        "bot.input",
        &json!({"observation": {"event": "text", "data": "What is the capital of France? Please tell me."}}),
    );
    let records = run.until(Duration::from_mins(6), |record| {
        record["type"] == "log"
            && matches!(
                record["label"].as_str(),
                Some("bot.processing.completed" | "bot.processing.failed")
            )
    });
    let finished = run.close(Duration::from_mins(1));
    let mut all = records;
    all.extend(finished.records);
    keep("live_cascade", &all);
    let stages: Vec<String> = logs(&all)
        .into_iter()
        .filter(|(label, _)| label == "cognition.stage")
        .map(|(_, text)| text)
        .collect();
    let output = logs(&all)
        .into_iter()
        .find(|(label, _)| label == "bot.output")
        .and_then(|(_, text)| serde_json::from_str::<Json>(&text).ok());
    let responded = output.as_ref().is_some_and(|output| output["turn"] == 1)
        && all_routed(&all, output.as_ref());
    let pass = stages.first().map(String::as_str) == Some("autonomy unhandled (no-behaviors)")
        && stages
            .get(1)
            .is_some_and(|stage| stage.starts_with("intuition unhandled"))
        && stages.get(2).map(String::as_str) == Some("reasoning handled")
        && responded;
    println!("live cascade: {}", if pass { "pass" } else { "fail" });
    assert!(
        pass,
        "the question did not escalate to a reasoned reply (target/floppy-live/live_cascade.json)"
    );
}

/// One turn on a bot with a lamp, for an input that names the lamp: the
/// body's choices are its own `bot.focus_device` (pinned to the lamp) and
/// the cognition's offer; whatever the cascade selects is routed (a focus
/// on the lamp moves the body's attention there), and nothing is dropped.
#[test]
#[ignore = "makes real decision and model calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_choices() {
    let mut run = start_body(&with_devices("['lamp']"));
    run.event("bot.activate", &json!({"model": model()}));
    run.event(
        "bot.input",
        &json!({"observation": {"event": "text", "data": "Please turn and look at the lamp."}, "target_device": "lamp"}),
    );
    let records = run.until(Duration::from_mins(6), |record| {
        record["type"] == "log"
            && matches!(
                record["label"].as_str(),
                Some("bot.processing.completed" | "bot.processing.failed")
            )
    });
    let finished = run.close(Duration::from_mins(1));
    let mut all = records;
    all.extend(finished.records);
    keep("live_choices", &all);
    let choices = "bot.focus_device bot.ability.cognition.ignore@cognition";
    let output = texts(&all, "bot.output")
        .pop()
        .and_then(|text| serde_json::from_str::<Json>(&text).ok());
    let reasoned = texts(&all, "reasoning");
    let focused_on_lamp = output.as_ref().is_some_and(|output| {
        output["output"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["event"] == "bot.focus_device"))
    }) == (texts(&all, "bot.focus") == ["lamp"]);
    let pass = texts(&all, "bot.choices") == [choices]
        && output.as_ref().is_some_and(|output| output["turn"] == 1)
        && reasoned
            .iter()
            .all(|line| *line == format!("turn 1: {choices}"))
        && all_routed(&all, output.as_ref())
        && focused_on_lamp;
    println!("live choices: {}", if pass { "pass" } else { "fail" });
    assert!(
        pass,
        "the turn's choices or routing were wrong (target/floppy-live/live_choices.json)"
    );
}
