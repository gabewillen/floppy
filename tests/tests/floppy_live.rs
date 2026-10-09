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
use support::{Run, logs, workspace};

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
    let mut args = Vec::new();
    for file in support::live_env_files() {
        args.push("--env-file".to_owned());
        args.push(file.display().to_string());
    }
    args.push(
        workspace()
            .join("bots/floppy/floppy.scx")
            .display()
            .to_string(),
    );
    Run::start(&args, &[])
}

fn model() -> String {
    std::env::var("FLOPPY_LIVE_MODEL").unwrap_or_else(|_| "Qwen/Qwen3.8-Flash-Next".into())
}

/// One question turn through the full cascade: autonomy's seam leaves it
/// unhandled, intuition (the decision service on `TypeSafe`) hands it to
/// reasoning or abstains, and reasoning (the model) answers it with a
/// `bot.ability.communication.respond` selection. The env files must also
/// hold `TYPESAFE_API_KEY`.
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
    let responded = output.as_ref().is_some_and(|output| {
        output["turn"] == 1
            && output["output"].as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    item["event"] == "bot.ability.communication.respond"
                        && item["data"]["text"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty())
                })
            })
    });
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
