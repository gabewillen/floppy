//! The floppy bot's identity (`abilities/identity/identity.scx`), run as the body's
//! child through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! under `bots/floppy/`, the communication, conversation, speaking,
//! identity, memory and every schema under `abilities/`) with
//! `tests/fixtures/phase4/cognition_double.scx` as `abilities/cognition/cognition.scx`
//! (no model; an addressed turn answers "I am <name>", a `bot.input`
//! observation `adopt` selects `bot.ability.identity.adopt` for its data).
//! A bot is born nameless; a test that needs a name rewrites the identity's
//! `<data id="name">`, which stands in for the invoker's `<param>`.
//!
//! The identity logs `identity.name`, `identity.heard`, `identity.offers`
//! and `identity.adopt.failed`; the body logs each route (`bot.route`).
//! The body gives the identity its database, in a directory each run is
//! granted (`support::memory_grant`), so an adopted name outlives the run.
//!
//! Every test ends by comparing its whole run with its pinned trace under
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
use support::{Run, logs, trace, unique_scratch, workspace};

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(300);

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

/// Lays out the bot with the cognition double; `name` (an ECMAScript
/// expression) replaces the identity's starting name. Returns the body.
fn layout(name: Option<&str>) -> PathBuf {
    let dir = unique_scratch("identity");
    copy_dir(&workspace().join("bots/floppy"), &dir.join("bots/floppy"));
    copy_dir(
        &workspace().join("bots/floppy/schemas"),
        &dir.join("bots/floppy/schemas"),
    );
    for name in [
        "communication",
        "conversation",
        "speaking",
        "identity",
        "memory",
    ] {
        support::copy_ability(name, &dir.join("abilities"));
    }
    std::fs::create_dir_all(dir.join("abilities/cognition")).expect("creatable");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/cognition_double.scx"),
        dir.join("abilities/cognition/cognition.scx"),
    )
    .expect("the double is copyable");
    if let Some(name) = name {
        let path = dir.join("abilities/identity/identity.scx");
        let text = std::fs::read_to_string(&path).expect("the identity is readable");
        let named = text.replacen(
            r#"<data id="name" expr="null"/>"#,
            &format!(r#"<data id="name" expr="{name}"/>"#),
            1,
        );
        assert_ne!(named, text, "the identity declares <data id=\"name\">");
        std::fs::write(&path, named).expect("the identity copy is writable");
    }
    dir.join("bots/floppy/floppy.scx")
}

fn label_is(label: &'static str) -> impl Fn(&Json) -> bool {
    move |record| record["type"] == "log" && record["label"] == label
}

/// The texts of the logs labelled `label`, in order.
fn texts(records: &[Json], label: &str) -> Vec<String> {
    logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .collect()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Starts the body and activates it; returns the run and every record up
/// to the activation.
fn started(body: &Path) -> (Run, Vec<Json>) {
    started_with(body, &[])
}

/// [`started`] with `grant` (the memory service's directory) before the
/// body; an empty `grant` gets a fresh directory.
fn started_with(body: &Path, grant: &[String]) -> (Run, Vec<Json>) {
    let mut args = grant.to_vec();
    args.push(body.display().to_string());
    let mut run = Run::start(&args, &[]);
    run.event("bot.activate", &json!({}));
    let records = run.until(WAIT, label_is("bot.activated"));
    (run, records)
}

fn said(text: &str) -> Json {
    json!({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": text, "content_type": "text/plain"})
}

fn input(action: &str, data: &str) -> Json {
    json!({"observation": {"event": action, "data": data}})
}

/// Closes the run and asserts its pinned trace; returns every record.
fn close(run: Run, test: &str) -> Vec<Json> {
    let finished = run.close(WAIT);
    trace::assert_trace(test, &finished.all, &[]);
    finished.all
}

#[test]
fn the_identity_is_ready_with_its_adopt_offer() {
    let (run, records) = started(&layout(None));
    // The identity's first offers are part of the activation; the body
    // gives it a database, so it recalls from memory.
    assert!(texts(&records, "memory").is_empty(), "{records:?}");
    assert_eq!(
        texts(&records, "identity.name"),
        strings(&["recalled null"])
    );
    assert_eq!(
        texts(&records, "identity.offers"),
        strings(&["bot.ability.identity.adopt"])
    );
    close(run, "the_identity_is_ready_with_its_adopt_offer");
}

#[test]
fn a_named_bot_is_addressed_by_its_name() {
    let (mut run, _) = started(&layout(Some("'Floppy'")));
    // The conversation's product is a turn too (the double ignores it:
    // the text's first word); the addressed turn answers.
    run.event(
        "bot.ability.communication.input",
        &said("ignore Floppy, what's your name?"),
    );
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(
        texts(&records, "identity.heard"),
        strings(&["addressed as Floppy"])
    );
    assert_eq!(texts(&records, "bot.say"), strings(&["I am Floppy"]));
    let all = close(run, "a_named_bot_is_addressed_by_its_name");
    let mut turns: Vec<String> = texts(&all, "double.input")
        .into_iter()
        .map(|turn| {
            turn.split_once(' ')
                .map(|(_, rest)| rest.to_owned())
                .unwrap_or(turn)
        })
        .collect();
    turns.sort();
    assert_eq!(turns, strings(&[r#"addressed "Floppy""#, "ignore 1"]));
}

#[test]
fn a_name_is_heard_only_as_a_whole_word() {
    let (mut run, _) = started(&layout(Some("'Al'")));
    run.event("bot.ability.communication.input", &said("ignore always"));
    let records = run.until(WAIT, label_is("identity.heard"));
    assert_eq!(
        texts(&records, "identity.heard"),
        strings(&["not addressed"])
    );
    let all = close(run, "a_name_is_heard_only_as_a_whole_word");
    assert!(
        texts(&all, "double.input")
            .iter()
            .all(|turn| !turn.contains("addressed")),
        "{all:?}"
    );
}

#[test]
fn a_nameless_bot_only_hears_words() {
    let (mut run, _) = started(&layout(None));
    run.event(
        "bot.ability.communication.input",
        &said("ignore Floppy, hello"),
    );
    let records = run.until(WAIT, label_is("communication.product"));
    let all = close(run, "a_nameless_bot_only_hears_words");
    let mut every = records;
    every.extend(all);
    assert!(texts(&every, "identity.heard").is_empty(), "{every:?}");
}

#[test]
fn an_adopted_name_is_answered_to() {
    let (mut run, _) = started(&layout(None));
    run.event("bot.input", &input("adopt", "Floppy"));
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "identity.name"
            && record["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("adopted"))
    });
    assert_eq!(
        texts(&records, "bot.route"),
        strings(&["bot.ability.identity.adopt -> identity"])
    );
    assert_eq!(
        texts(&records, "identity.name"),
        strings(&[r#"adopted "Floppy" (persisted: true)"#])
    );
    // Back in knowing, it offers again, and now answers to the name.
    run.event(
        "bot.ability.communication.input",
        &said("ignore hey floppy"),
    );
    let records = run.until(WAIT, label_is("bot.ability.speaking.output"));
    assert_eq!(
        texts(&records, "identity.heard"),
        strings(&["addressed as Floppy"])
    );
    assert_eq!(texts(&records, "bot.say"), strings(&["I am Floppy"]));
    let all = close(run, "an_adopted_name_is_answered_to");
    assert_eq!(
        texts(&all, "identity.offers"),
        strings(&["bot.ability.identity.adopt", "bot.ability.identity.adopt"])
    );
}

#[test]
fn an_adopted_name_is_recalled_by_the_next_run() {
    let body = layout(None);
    let grant = support::memory_grant();
    let (mut run, _) = started_with(&body, &grant);
    run.event("bot.input", &input("adopt", "Floppy"));
    run.until(WAIT, label_is("identity.offers"));
    run.event("bot.input", &input("adopt", "Pip"));
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "identity.name"
            && record["text"] == r#"adopted "Pip" (persisted: true)"#
    });
    assert!(texts(&records, "identity.adopt.failed").is_empty());
    drop(run.close(WAIT));
    // The next run wakes up with the name it took last.
    let (run, records) = started_with(&body, &grant);
    assert_eq!(
        texts(&records, "identity.name"),
        strings(&[r#"recalled "Pip""#])
    );
    close(run, "an_adopted_name_is_recalled_by_the_next_run");
}
