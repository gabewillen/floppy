//! The Reflection ability (`abilities/reflection/reflection.scx`) with its Revision
//! through the builder bot, run through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of `abilities/` (documents and
//! schemas) and `bots/builder/`, with the harness
//! `tests/fixtures/phase5/reflection_harness.scx` beside the reflection:
//! the harness seeds the database, invokes the reflection with a memory
//! database in a granted directory, forwards each
//! `bot.ability.reflection.input` from standard input, and after each
//! answer logs it and then the inventory and the stored episodes.
//!
//! One deterministic fake Chat Completions backend on 127.0.0.1 answers
//! by the request's structured-output schema: `dispatch` (the select step)
//! by the route (the first path segment of `FLOPPY_BASE_URL`),
//! `topology_plan` and `scxml_draft` (the builder) with a plan and a turn
//! behavior draft, or with drafts that never validate on the `buildfail`
//! route. The decision backend (the builder's triage) always fails, so the
//! builder falls back on its own. Nothing reaches the network; the keys are
//! fake and reach the run only through an env file.
//!
//! Each `postgres_` test runs its case on the Postgres engine, the
//! harness's database a fresh `PGlite` server's ([`support::Pglite`]); they
//! are ignored unless `FLOPPY_PGLITE_SERVER` is set.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Run, logs, unique_scratch, workspace};

/// Bounded wait for any one expected record (scx startup and a builder run
/// included).
const WAIT: Duration = Duration::from_secs(180);
const FAKE_KEYS: &str = "FLOPPY_API_KEY=floppy-fake\nTYPESAFE_API_KEY=ts-floppy-fake\n";

/// The behavior a create installs: it answers its turn with one response.
const BEHAVIOR: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="GreetOnKnock" initial="answering">
  <datamodel>
    <data id="turn" expr="0"/>
    <data id="input" expr="null"/>
  </datamodel>
  <state id="answering">
    <transition target="answered"/>
  </state>
  <final id="answered">
    <donedata>
      <content expr="({turn: turn, handled: true, selected: [{event: 'bot.ability.communication.respond', data: {text: 'Hello!'}, reason: 'a knock'}]})"/>
    </donedata>
  </final>
</scxml>"#;

const PLAN: &str = r#"{"name":"GreetOnKnock","summary":"Answer a knock with a greeting.","states":[{"id":"answering","kind":"atomic","parent":"","purpose":"Decide the answer."},{"id":"answered","kind":"final","parent":"","purpose":"Carry the answer."}],"events":[],"transitions":[{"source":"answering","event":"","target":"answered","description":"Answer at once."}]}"#;

/// The builder's draft: `scxml` with one test that drives nothing and
/// expects the behavior to finish.
fn draft(scxml: &str) -> String {
    json!({
        "scxml": scxml,
        "schemas": [],
        "tests": [{"name": "answers its turn", "events": [], "settle_ms": 2000, "expect_events": [], "expect_done": true}],
    })
    .to_string()
}

/// A non-streamed Chat Completions answer carrying `content`, as the
/// generate service asks for one by default.
fn completion(content: &str) -> String {
    json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": content},
        }],
    })
    .to_string()
}

fn create_selection() -> Json {
    json!([{
        "event": "bot.behavior.create",
        "data": {
            "event": "bot.behavior.create",
            "name": "GreetOnKnock",
            "triggers": ["phone.incoming_call", "environment.sound"],
            "description": "Say hello when a knock sound arrives.",
            "reason": "Same knock and greeting pattern across recent episodes.",
            "source": null,
        },
        "reason": "a repeated pattern",
    }])
}

fn break_selection(name: &str) -> Json {
    json!([{
        "event": "bot.behavior.break",
        "data": {"event": "bot.behavior.break", "name": name, "reason": "  It greets strangers.  "},
        "reason": "harm outweighs practice",
    }])
}

/// The select step's answer on `route`.
fn selection(route: &str) -> Option<Json> {
    match route {
        "none" => Some(json!([])),
        "create" | "buildfail" => Some(create_selection()),
        "break" => Some(break_selection("GreetOnKnock")),
        "breakunknown" => Some(break_selection("Nobody")),
        "two" => Some(json!([
            break_selection("GreetOnKnock")[0].clone(),
            break_selection("Other")[0].clone()
        ])),
        _ => None,
    }
}

/// The scripted answer to one request, or `None` to answer 500.
fn answer(target: &str, body: &Json) -> Option<String> {
    let route = target.trim_start_matches('/').split('/').next()?;
    if !target.ends_with("/v1/chat/completions") {
        return None;
    }
    let schema = body
        .pointer("/response_format/json_schema/name")?
        .as_str()?;
    match schema {
        "dispatch" => Some(completion(
            &json!({"events": selection(route)?}).to_string(),
        )),
        "topology_plan" => Some(completion(PLAN)),
        "scxml_draft" if route == "buildfail" => Some(completion(&draft(
            &BEHAVIOR.replace("target=\"answered\"", "target=\"nowhere\""),
        ))),
        "scxml_draft" => Some(completion(&draft(BEHAVIOR))),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct Received {
    body: Json,
}

impl Received {
    fn schema_name(&self) -> Option<&str> {
        self.body
            .pointer("/response_format/json_schema/name")
            .and_then(Json::as_str)
    }

    fn message(&self, role: &str) -> String {
        self.body["messages"]
            .as_array()
            .and_then(|messages| messages.iter().find(|message| message["role"] == role))
            .and_then(|message| message["content"].as_str())
            .unwrap_or_else(|| panic!("a {role} message: {}", self.body))
            .to_owned()
    }
}

fn serve(mut stream: TcpStream, log: &Arc<Mutex<Vec<Received>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
    let body: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let reply = answer(&target, &body);
    log.lock().expect("the log lock").push(Received { body });
    let (status, text) = reply.map_or_else(
        || {
            (
                "500 Internal Server Error",
                r#"{"error":{"message":"boom"}}"#.to_owned(),
            )
        },
        |text| ("200 OK", text),
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    );
}

/// The fake backend of the generate and decision services.
struct Mock {
    origin: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log));
            }
        });
        Self { origin, received }
    }

    /// The requests with the structured-output schema `name`, in order.
    fn requests(&self, name: &str) -> Vec<Received> {
        self.received
            .lock()
            .expect("the log lock")
            .iter()
            .filter(|request| request.schema_name() == Some(name))
            .cloned()
            .collect()
    }
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

/// The harness's SQLite database, in the directory each run is granted.
const SQLITE: &str = "/memory/floppy.sqlite";

/// Lays out the scratch copy with the harness seeded by `seed` (a
/// statements expression) on `database` and an env file for `route`;
/// returns the harness's path and the env file's.
fn layout(mock: &Mock, route: &str, seed: &str, database: &str) -> (PathBuf, PathBuf) {
    let dir = unique_scratch("reflection");
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    copy_dir(&workspace().join("bots/builder"), &dir.join("bots/builder"));
    copy_dir(
        &workspace().join("bots/builder/schemas"),
        &dir.join("bots/builder/schemas"),
    );
    let harness =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase5/reflection_harness.scx"))
            .expect("the harness is readable")
            .replace(
                r#"<data id="seed" expr="[]"/>"#,
                &format!(r#"<data id="seed" expr="{seed}"/>"#),
            );
    let path = dir.join("abilities/reflection_harness.scx");
    std::fs::write(&path, harness).expect("the harness is writable");
    support::use_database(&path, database);
    let env = dir.join("fake.env");
    std::fs::write(
        &env,
        format!(
            "{FAKE_KEYS}FLOPPY_BASE_URL={origin}/{route}/v1\nTYPESAFE_BASE_URL={origin}/{route}\n",
            origin = mock.origin,
        ),
    )
    .expect("the env file is writable");
    (path, env)
}

fn start(harness: &Path, env: &Path) -> Run {
    Run::start(
        &[
            "--env-file".to_owned(),
            env.display().to_string(),
            harness.display().to_string(),
        ],
        &[],
    )
}

/// One turn's reflection input: a knock heard while looking at the door.
fn input(turn: u64) -> Json {
    json!({
        "turn": turn,
        "cognition_input": {
            "stimulus": {"event": "environment.sound", "data": {"label": "knock"}},
            "focus": "door",
            "focus_candidates": ["door"],
        },
        "cognition_output": [
            {"event": "bot.ability.communication.respond", "data": {"text": "Hello!"}, "reason": "a knock", "target": "communication"}
        ],
    })
}

/// Sends one turn and returns its answer's records through the logged
/// inventory and episodes.
fn reflect(run: &mut Run, turn: u64) -> Vec<Json> {
    run.event("bot.ability.reflection.input", &input(turn));
    run.until(WAIT, |record| {
        record["type"] == "log" && record["label"] == "episodes"
    })
}

fn texts(records: &[Json], label: &str) -> Vec<String> {
    logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .collect()
}

fn json_log(records: &[Json], label: &str) -> Json {
    let text = texts(records, label)
        .pop()
        .unwrap_or_else(|| panic!("a {label} log: {records:?}"));
    serde_json::from_str(&text).expect("the log text is JSON")
}

/// A seed of one ACTIVE turn behavior named `GreetOnKnock`, used once.
const SEED_BEHAVIOR: &str = "[{sql: &quot;INSERT INTO bot_behavior (name, source, description, status, used_count, last_used_at) VALUES ('GreetOnKnock', '&lt;scxml/&gt;', 'Say hello.', 'ACTIVE', '1', '2026-10-09T00:00:00Z')&quot;}, {sql: &quot;INSERT INTO bot_behavior_trigger (behavior_name, \\&quot;trigger\\&quot;) VALUES ('GreetOnKnock', 'environment.sound')&quot;}]";

/// mosfet's select step: with nothing selected the turn's episode is
/// stored, and the next turn's select recalls it.
#[test]
fn an_empty_selection_stores_the_episode_and_the_next_turn_recalls_it() {
    an_empty_selection_stores_the_episode_and_the_next_turn_recalls_it_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_an_empty_selection_stores_the_episode_and_the_next_turn_recalls_it() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    an_empty_selection_stores_the_episode_and_the_next_turn_recalls_it_on(&server.database());
}

fn an_empty_selection_stores_the_episode_and_the_next_turn_recalls_it_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "none", "[]", database);
    let mut run = start(&harness, &env);
    let first = reflect(&mut run, 1);
    assert_eq!(texts(&first, "harness.output"), ["answered"]);
    assert_eq!(texts(&first, "reflection.selected"), ["none"]);
    assert_eq!(json_log(&first, "inventory"), json!([]));
    let episode = json!({
        "focus": "door",
        "focus_candidates": ["door"],
        "stimulus_name": "environment.sound",
        "output": input(1)["cognition_output"],
        "behavior": null,
    });
    assert_eq!(
        json_log(&first, "episodes"),
        json!([{"context_ref": "door", "scope": "memory", "kind": "task", "content": episode}])
    );
    let second = reflect(&mut run, 2);
    assert_eq!(texts(&second, "harness.output"), ["answered"], "{second:?}");
    assert_eq!(
        json_log(&second, "episodes").as_array().map(Vec::len),
        Some(2)
    );
    drop(run.close(WAIT));

    let selects = mock.requests("dispatch");
    assert_eq!(selects.len(), 2);
    assert!(
        selects[0]
            .message("system")
            .starts_with("Select at most one offered behavior inventory event"),
    );
    let prompt = |index: usize| -> Json {
        serde_json::from_str(&selects[index].message("user")).expect("the select prompt is JSON")
    };
    assert_eq!(prompt(0)["prior_episodes"], json!([]));
    assert_eq!(prompt(0)["behaviors"], json!([]));
    assert_eq!(prompt(0)["cognition_input"], input(1)["cognition_input"]);
    assert_eq!(prompt(1)["prior_episodes"], json!([episode]));
    // The offered events are exactly the three inventory events.
    let schema = &selects[0].body["response_format"]["json_schema"]["schema"];
    let mut offered: Vec<String> = schema["properties"]["events"]["items"]["anyOf"]
        .as_array()
        .expect("one branch per offered event")
        .iter()
        .map(|branch| {
            branch["properties"]["event"]["enum"][0]
                .as_str()
                .expect("the branch's event")
                .to_owned()
        })
        .collect();
    offered.sort();
    assert_eq!(
        offered,
        [
            "bot.behavior.break",
            "bot.behavior.change",
            "bot.behavior.create"
        ]
    );
}

/// mosfet `_apply_break`: the row is kept, BROKEN with the trimmed reason,
/// and the episode records the break.
#[test]
fn a_break_marks_the_behavior_broken_and_keeps_it() {
    a_break_marks_the_behavior_broken_and_keeps_it_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_break_marks_the_behavior_broken_and_keeps_it() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    a_break_marks_the_behavior_broken_and_keeps_it_on(&server.database());
}

fn a_break_marks_the_behavior_broken_and_keeps_it_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "break", SEED_BEHAVIOR, database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(texts(&records, "harness.output"), ["answered"]);
    assert_eq!(
        texts(&records, "reflection.inventory"),
        ["GreetOnKnock BROKEN (break)"]
    );
    assert_eq!(
        json_log(&records, "inventory"),
        json!([{
            "name": "GreetOnKnock", "source": "<scxml/>", "description": "Say hello.",
            "lifetime": "turn", "status": "BROKEN", "status_reason": "It greets strangers.",
            "used_count": "1", "failed_count": "0", "triggers": ["environment.sound"],
        }])
    );
    assert_eq!(
        json_log(&records, "episodes")[0]["content"]["behavior"],
        break_selection("GreetOnKnock")[0]["data"]
    );
    // The select step saw the inventory with its usage.
    let prompt: Json = serde_json::from_str(&mock.requests("dispatch")[0].message("user"))
        .expect("the select prompt is JSON");
    assert_eq!(prompt["behaviors"][0]["name"], "GreetOnKnock");
    assert_eq!(prompt["behaviors"][0]["used_count"], 1);
    assert_eq!(
        prompt["behaviors"][0]["triggers"],
        json!(["environment.sound"])
    );
}

/// A break of a behavior the inventory does not hold fails the turn and
/// stores nothing.
#[test]
fn a_break_of_an_unknown_behavior_fails() {
    a_break_of_an_unknown_behavior_fails_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_break_of_an_unknown_behavior_fails() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    a_break_of_an_unknown_behavior_fails_on(&server.database());
}

fn a_break_of_an_unknown_behavior_fails_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "breakunknown", "[]", database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.failed"),
        ["Reflection break failed: Reflection break selected unknown behavior: Nobody."]
    );
    assert_eq!(json_log(&records, "episodes"), json!([]));
}

/// mosfet's select returns at most one inventory event.
#[test]
fn more_than_one_selection_fails() {
    more_than_one_selection_fails_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_more_than_one_selection_fails() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    more_than_one_selection_fails_on(&server.database());
}

fn more_than_one_selection_fails_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "two", SEED_BEHAVIOR, database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.failed"),
        ["Reflection select must return at most one behavior event."]
    );
    assert_eq!(json_log(&records, "inventory")[0]["status"], "ACTIVE");
}

/// mosfet Revision through the builder: a create stores the built document
/// ACTIVE with its grounded triggers, and the episode records it.
#[test]
fn a_create_is_built_and_stored_active_with_grounded_triggers() {
    a_create_is_built_and_stored_active_with_grounded_triggers_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_create_is_built_and_stored_active_with_grounded_triggers() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    a_create_is_built_and_stored_active_with_grounded_triggers_on(&server.database());
}

fn a_create_is_built_and_stored_active_with_grounded_triggers_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "create", "[]", database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.output"),
        ["answered"],
        "{records:?}"
    );
    assert_eq!(
        texts(&records, "reflection.inventory"),
        [r#"GreetOnKnock ACTIVE turn ["environment.sound"] (accept)"#]
    );
    assert_eq!(
        json_log(&records, "inventory"),
        json!([{
            "name": "GreetOnKnock", "source": BEHAVIOR,
            "description": "Say hello when a knock sound arrives.",
            "lifetime": "turn", "status": "ACTIVE", "status_reason": "",
            "used_count": "0", "failed_count": "0", "triggers": ["environment.sound"],
        }])
    );
    let behavior = &json_log(&records, "episodes")[0]["content"]["behavior"];
    assert_eq!(behavior["name"], "GreetOnKnock");
    assert_eq!(behavior["triggers"], json!(["environment.sound"]));
    assert_eq!(behavior["source"], BEHAVIOR);
    // The builder was asked once, with the intent, the DRAFT stub, and the
    // turn behavior contract.
    let drafts = mock.requests("scxml_draft");
    assert_eq!(drafts.len(), 1);
    let spec: Json =
        serde_json::from_str(&drafts[0].message("user")).expect("the builder's prompt is JSON");
    let spec = spec["spec"].as_str().expect("the spec").to_owned();
    assert!(
        spec.starts_with("Author or revise the SCXML program"),
        "{spec}"
    );
    assert!(spec.contains(r#""status":"DRAFT""#), "{spec}");
    assert!(spec.contains(r#""name":"GreetOnKnock""#), "{spec}");
}

/// A build that fails keeps the create's stub as DRAFT with
/// `status_reason` validation, and the turn fails (mosfet `_abandon`).
#[test]
fn a_failed_build_leaves_a_validation_draft() {
    a_failed_build_leaves_a_validation_draft_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_failed_build_leaves_a_validation_draft() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    a_failed_build_leaves_a_validation_draft_on(&server.database());
}

fn a_failed_build_leaves_a_validation_draft_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "buildfail", "[]", database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.failed"),
        ["Reflection change abandoned: no-progress."],
        "{records:?}"
    );
    assert_eq!(
        json_log(&records, "inventory"),
        json!([{
            "name": "GreetOnKnock", "source": "",
            "description": "Say hello when a knock sound arrives.",
            "lifetime": "turn", "status": "DRAFT", "status_reason": "validation",
            "used_count": "0", "failed_count": "0",
            "triggers": ["environment.sound", "phone.incoming_call"],
        }])
    );
    assert_eq!(json_log(&records, "episodes"), json!([]));
}
