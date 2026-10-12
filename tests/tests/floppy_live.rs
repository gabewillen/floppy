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
    for sub in ["bots/floppy", "bots/floppy/schemas"] {
        copy_dir(&workspace().join(sub), &dir.join(sub));
    }
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
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

/// The turn completed, and every selection of its output but an ignore
/// (never dispatched) was routed.
fn all_routed(records: &[Json], output: Option<&Json>) -> bool {
    let selected = output
        .and_then(|output| output["output"].as_array())
        .map_or(usize::MAX, |items| {
            items
                .iter()
                .filter(|item| item["event"] != "bot.ability.cognition.ignore")
                .count()
        });
    texts(records, "floppy.route").len() == selected
        && texts(records, "bot.processing.failed").is_empty()
}

fn model() -> String {
    support::live_model()
}

/// One question turn through the full cascade: autonomy's seam leaves it
/// unhandled, intuition (the decision service on `TypeSafe`) hands it to
/// reasoning or abstains, and reasoning (the model) selects from the
/// body's assembled choices, which the body routes (`live_chat` checks
/// what is said). The env files must also hold `TYPESAFE_API_KEY`.
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
        .find(|(label, _)| label == "floppy.output")
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
/// the cognition's and the communication's offers; whatever the cascade selects is routed (a focus
/// on the lamp moves the body's attention there), and the turn completes.
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
    let choices = "bot.focus_device bot.ability.cognition.ignore@cognition bot.ability.communication.respond@communication bot.ability.identity.adopt@identity";
    let output = texts(&all, "floppy.output")
        .pop()
        .and_then(|text| serde_json::from_str::<Json>(&text).ok());
    let reasoned = texts(&all, "reasoning");
    let focused_on_lamp = output.as_ref().is_some_and(|output| {
        output["output"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["event"] == "bot.focus_device"))
    }) == (texts(&all, "floppy.focus") == ["lamp"]);
    let pass = texts(&all, "floppy.choices") == [choices]
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

/// One text turn through the original's communication path: a
/// `bot.ability.communication.input` addressed to the bot is admitted into
/// the conversation, whose product is the turn's stimulus; the cascade
/// selects `bot.ability.communication.respond`, which the body routes to
/// the communication and speaking says (`floppy.say`), and the spoken text is
/// recorded in the conversation.
#[test]
#[ignore = "makes real decision and model calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_chat() {
    let mut run = start();
    run.event("bot.activate", &json!({"model": model()}));
    run.event(
        "bot.ability.communication.input",
        &json!({
            "source_ids": ["gabe"],
            "target_ids": ["floppy"],
            "content": "Hi floppy! Please say hello back to me.",
            "content_type": "text/plain"
        }),
    );
    let records = run.until(Duration::from_mins(6), |record| {
        let text = record["text"].as_str().unwrap_or("");
        record["type"] == "log"
            && match record["label"].as_str() {
                Some("communication.history" | "bot.processing.failed") => true,
                Some("bot.processing.completed") => {
                    !text.contains("bot.ability.communication.respond")
                }
                _ => false,
            }
    });
    let finished = run.close(Duration::from_mins(1));
    let mut all = records;
    all.extend(finished.records);
    keep("live_chat", &all);
    let output = texts(&all, "floppy.output")
        .pop()
        .and_then(|text| serde_json::from_str::<Json>(&text).ok());
    let said = texts(&all, "floppy.say");
    let pass = texts(&all, "communication.product") == ["0"]
        && output.as_ref().is_some_and(|output| output["turn"] == 1)
        && all_routed(&all, output.as_ref())
        && said.len() == 1
        && said.iter().all(|text| !text.trim().is_empty())
        && texts(&all, "bot.ability.speaking.output").len() == 1
        && texts(&all, "conversation")
            .iter()
            .any(|line| line == "outbound 1");
    println!("live chat: {}", if pass { "pass" } else { "fail" });
    assert!(
        pass,
        "the input was not answered through communication.respond (target/floppy-live/live_chat.json)"
    );
}

/// A scratch copy of the bot whose identity starts with the name `name`
/// (standing in for the invoker's `<param>`); returns the body's path.
fn with_name(name: &str) -> PathBuf {
    let dir = unique_scratch("floppy-live");
    for sub in ["bots/floppy", "bots/floppy/schemas"] {
        copy_dir(&workspace().join(sub), &dir.join(sub));
    }
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    let identity = dir.join("abilities/identity/identity.scx");
    let text = std::fs::read_to_string(&identity).expect("the identity is readable");
    let declared = r#"<data id="name" expr="null"/>"#;
    assert!(text.contains(declared), "the identity declares its name");
    std::fs::write(
        &identity,
        text.replace(declared, &format!(r#"<data id="name" expr="'{name}'"/>"#)),
    )
    .expect("the identity copy is writable");
    dir.join("bots/floppy/floppy.scx")
}

/// A bot named Floppy is addressed by "Floppy, what's your name?": the
/// identity hears its name (`identity.heard`), the addressed is a turn's
/// stimulus, and the bot answers through `communication.respond`.
#[test]
#[ignore = "makes real decision and model calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_identity() {
    let mut run = start_body(&with_name("Floppy"));
    run.event("bot.activate", &json!({"model": model()}));
    run.event(
        "bot.ability.communication.input",
        &json!({
            "source_ids": ["gabe"],
            "target_ids": ["floppy"],
            "content": "Floppy, what's your name?",
            "content_type": "text/plain"
        }),
    );
    let addressed = std::cell::Cell::new(false);
    let records = run.until(Duration::from_mins(6), |record| {
        if record["type"] == "log" && record["label"] == "identity.heard" {
            addressed.set(true);
        }
        addressed.get()
            && record["type"] == "log"
            && matches!(
                record["label"].as_str(),
                Some("bot.ability.speaking.output" | "bot.processing.failed")
            )
    });
    let finished = run.close(Duration::from_mins(1));
    let mut all = records;
    all.extend(finished.records);
    keep("live_identity", &all);
    let said = texts(&all, "floppy.say");
    let pass = texts(&all, "identity.heard") == ["addressed as Floppy"]
        && !said.is_empty()
        && said.iter().all(|text| !text.trim().is_empty())
        && texts(&all, "bot.processing.failed").is_empty();
    println!("live identity: {}", if pass { "pass" } else { "fail" });
    assert!(
        pass,
        "the addressed bot did not answer (target/floppy-live/live_identity.json)"
    );
}

/// One labelled line per record of interest, for reading a long run.
fn transcript(records: &[Json]) -> String {
    const SHOWN: &[&str] = &[
        "bot.activated",
        "bot.activating.failed",
        "cognition.stage",
        "cognition.failed",
        "cognition.reflection.failed",
        "cognition.routines.failed",
        "cognition.hop.timed_out",
        "autonomy",
        "autonomy.candidate",
        "autonomy.candidate.failed",
        "autonomy.behaviors.load.failed",
        "reasoning",
        "memory",
        "reflection",
        "reflection.selected",
        "reflection.revision",
        "reflection.inventory",
        "reflection.stored",
        "reflection.failed",
        "routines",
        "routines.failed",
        "bot.ability.routines.reconciled",
        "bot.ability.routines.reconcile.failed",
        "floppy.output",
        "floppy.route",
        "floppy.say",
        "floppy.deferred",
        "floppy.degraded",
        "bot.processing.completed",
        "bot.processing.failed",
        "bot.processing.timed_out",
        "bot.ability.failed",
        "bot.ability.listening.failed",
        "bot.ability.communication.failed",
        "bot.ability.speaking.output",
        "bot.reboot",
        "identity.heard",
        "communication.product",
        "conversation",
    ];
    let mut out = String::new();
    for (label, text) in logs(records) {
        if SHOWN.contains(&label.as_str()) {
            let text: String = text.chars().take(400).collect();
            out.push_str(&label);
            out.push_str(": ");
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}

/// The whole bot over several turns of one session, every ability wired:
/// a fact to remember, a standing request reflection may learn as a
/// behavior (the builder authors it), the request's trigger (autonomy may
/// answer it from the learned behavior), and a question the reasoning
/// answers from memory. Each turn waits for its reflection to settle
/// (or the turn to fail) before the next. Records go to
/// `target/floppy-live/live_conversation.json` and a readable transcript
/// to `live_conversation.txt`. It asserts that every turn completed, that
/// reflection learned a behavior by the end of the second turn, and that the
/// fourth turn's answer names green (the first turn's fact, which the
/// conversation's history carries to reasoning, decision D99). The last two
/// depend on the model's choices: a model that never learns the request,
/// or that does not answer from the history, fails the run.
#[test]
#[ignore = "makes real decision and model calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_conversation() {
    let inputs = [
        "Hi floppy! My favorite color is green. Please remember that.",
        "From now on, whenever I say exactly 'ping', reply with exactly 'pong'.",
        "ping",
        "What is my favorite color?",
    ];
    let mut run = start();
    run.event("bot.activate", &json!({"model": model()}));
    let settled = std::cell::Cell::new(0_usize);
    for (index, content) in inputs.iter().enumerate() {
        run.event(
            "bot.ability.communication.input",
            &json!({
                "source_ids": ["gabe"],
                "target_ids": ["floppy"],
                "content": content,
                "content_type": "text/plain"
            }),
        );
        let goal = index + 1;
        let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run.until(Duration::from_mins(10), |record| {
                if record["type"] == "log"
                    && matches!(
                        record["label"].as_str(),
                        Some(
                            "bot.ability.reflection.output"
                                | "cognition.reflection.failed"
                                | "bot.processing.failed"
                                | "floppy.degraded"
                                | "bot.activating.failed"
                        )
                    )
                {
                    settled.set(settled.get() + 1);
                }
                settled.get() >= goal
            })
        }));
        if waited.is_err() {
            break;
        }
    }
    let finished = run.close(Duration::from_mins(1));
    let all = finished.all;
    keep("live_conversation", &all);
    std::fs::write(
        workspace().join("target/floppy-live/live_conversation.txt"),
        transcript(&all),
    )
    .expect("writable");
    let completed = texts(&all, "bot.processing.completed").len();
    let failed = texts(&all, "bot.processing.failed");
    println!(
        "live conversation: {completed} completed, {} failed",
        failed.len()
    );
    assert!(
        failed.is_empty() && completed >= inputs.len(),
        "a turn failed (target/floppy-live/live_conversation.txt)"
    );
    let logged = logs(&all);
    let third = logged
        .iter()
        .enumerate()
        .filter(|(_, (label, _))| label == "bot.processing.completed")
        .nth(2)
        .map_or(logged.len(), |(index, _)| index);
    assert!(
        logged[..third]
            .iter()
            .any(|(label, text)| label == "reflection.inventory"
                && text.contains(" ACTIVE ")
                && text.ends_with("(accept)")),
        "no behavior was learned by the second turn (target/floppy-live/live_conversation.txt)"
    );
    let answer = texts(&all, "bot.processing.completed")
        .get(3)
        .cloned()
        .unwrap_or_default();
    assert!(
        answer.to_lowercase().contains("green"),
        "the fourth turn did not recall green (target/floppy-live/live_conversation.txt)"
    );
}
