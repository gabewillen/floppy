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
//! by the schema's name, the one tool the request requires a call of: `dispatch` (the select step)
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

use serde_json::{Map, Value as Json, json};
use support::model_facing::{read, rendered};
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
      <content expr="({turn: turn, handled: true, selected: [{event: 'bot.ability.communication.respond', target: 'communication', data: {parts: [{words: 'Hello!'}]}, reason: 'a knock'}]})"/>
    </donedata>
  </final>
</scxml>"#;

/// A behavior that answers only a knock, read from its param `input`: run
/// with no param (its own test) it is unhandled, so only the builder's run
/// on the live turn (decision D131) can find it handled.
const GROUNDED: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="GreetOnKnock" initial="answering">
  <datamodel>
    <data id="turn" expr="0"/>
    <data id="input" expr="null"/>
  </datamodel>
  <script src="GreetOnKnock.js"/>
  <state id="answering">
    <transition cond="knocked(input)" target="answered"/>
    <transition target="unanswered"/>
  </state>
  <final id="answered">
    <donedata>
      <content expr="answer(turn, input)"/>
    </donedata>
  </final>
  <final id="unanswered">
    <donedata>
      <content expr="({turn: turn, handled: false, selected: []})"/>
    </donedata>
  </final>
</scxml>"#;

/// [`GROUNDED`]'s script.
const GROUNDED_SCRIPT: &str = "function knocked(input) {\n  return input !== null && input.label === 'knock';\n}\nfunction answer(turn, input) {\n  return {turn: turn, handled: true, selected: [{event: 'bot.ability.communication.respond', target: 'communication', data: {parts: [{words: 'Hello!'}], label: input.label}, reason: 'a knock'}]};\n}\n";

/// [`BEHAVIOR`] with the label of the knock it was learned from hardcoded
/// as another (a prior episode's) value.
fn hardcoded_behavior() -> String {
    BEHAVIOR.replace(
        "data: {parts: [{words: 'Hello!'}]}",
        "data: {parts: [{words: 'Hello!'}], label: 'doorbell'}",
    )
}

const PLAN: &str = r#"{"name":"GreetOnKnock","summary":"Answer a knock with a greeting.","states":[{"id":"answering","kind":"atomic","parent":"","purpose":"Decide the answer."},{"id":"answered","kind":"final","parent":"","purpose":"Carry the answer."}],"events":[],"transitions":[{"source":"answering","event":"","target":"answered","description":"Answer at once."}]}"#;

/// The builder's draft: `scxml`, no schema and no script, with one test
/// that drives nothing and expects the behavior to finish.
fn draft(scxml: &str) -> String {
    json!({
        "scxml": scxml,
        "schemas": {"$defs": {}},
        "script": "",
        "tests": [{"name": "answers its turn", "events": [], "settle_ms": 2000, "expect_events": [], "expect_done": true}],
    })
    .to_string()
}

/// [`draft`] with `script`.
fn scripted_draft(scxml: &str, script: &str) -> String {
    let mut draft: Json = serde_json::from_str(&draft(scxml)).expect("the draft is JSON");
    draft["script"] = script.into();
    draft.to_string()
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

/// The create a live model sends: only the fields it must, with no
/// `event` discriminator (the schema defaults it) and no `source`.
fn bare_create_selection() -> Json {
    json!([{
        "event": "bot.behavior.create",
        "data": {
            "name": "GreetOnKnock",
            "triggers": ["phone.incoming_call", "environment.sound"],
            "description": "Say hello when a knock sound arrives.",
            "reason": "Same knock and greeting pattern across recent episodes.",
        },
        "reason": "a repeated pattern",
    }])
}

fn change_selection() -> Json {
    json!([{
        "event": "bot.behavior.change",
        "data": {
            "event": "bot.behavior.change",
            "name": "GreetOnKnock",
            "reason": "It should greet by name.",
        },
        "reason": "a regression",
    }])
}

fn break_selection(name: &str) -> Json {
    json!([{
        "event": "bot.behavior.break",
        "data": {"event": "bot.behavior.break", "name": name, "reason": "  It greets strangers.  "},
        "reason": "harm outweighs practice",
    }])
}

/// A persona revision (decision D145).
fn revise_selection(text: &str) -> Json {
    json!([{
        "event": "bot.ability.persona.revise",
        "data": {"text": text, "reason": "I noticed I like greeting people."},
        "reason": "I see myself differently",
    }])
}

/// Arguments the generate service refuses as not-json.
const NOT_JSON: &str = r#"{"events": ["#;

/// The live runs' refused create (docs/live-proof.md, `live_conversation`): its
/// fields beside `event`, with no `data` and an unoffered `lifetime`, which
/// the generate service refuses as schema-mismatch.
fn flat_create_selection() -> Json {
    json!([{
        "event": "bot.behavior.create",
        "name": "GreetOnKnock",
        "triggers": ["environment.sound"],
        "lifetime": "turn",
        "reason": "a repeated pattern",
    }])
}

/// The live runs' create with its `data` added but its `lifetime` still
/// beside `event`: refused where `flat_create_selection` is, with another
/// message.
fn half_repaired_create_selection() -> Json {
    json!([{
        "event": "bot.behavior.create",
        "data": create_selection()[0]["data"].clone(),
        "lifetime": "turn",
        "reason": "a repeated pattern",
    }])
}

/// The select step's `dispatch` arguments on `route` for the run's
/// `attempt`th select call (from 0): the repair routes answer a refused
/// selection first (decision D130).
fn dispatch_reply(route: &str, attempt: usize, body: &Json) -> Option<String> {
    let events =
        |selected: &Json| json!({"events": support::with_targets(body, selected)}).to_string();
    match (route, attempt) {
        ("repaired" | "samekey", 0) | ("rejections", 1) => Some(events(&flat_create_selection())),
        ("samekey", 1) => Some(events(&half_repaired_create_selection())),
        ("rejections" | "repairedtwice", 0 | 2) => Some(NOT_JSON.to_owned()),
        ("repaired" | "rejections" | "samekey", _) => Some(events(&create_selection())),
        ("repairedtwice", _) => Some(events(&json!([]))),
        _ => Some(events(&selection(route)?)),
    }
}

/// The select step's answer on `route`.
fn selection(route: &str) -> Option<Json> {
    match route {
        "none" => Some(json!([])),
        "create" | "buildfail" | "genfail" | "slowbuild" | "stallbuild" | "slowplain"
        | "livebound" | "livehardcoded" => Some(create_selection()),
        "createbare" => Some(bare_create_selection()),
        "break" => Some(break_selection("GreetOnKnock")),
        "change" | "changefail" => Some(change_selection()),
        "breakunknown" => Some(break_selection("Nobody")),
        "two" => Some(json!([
            break_selection("GreetOnKnock")[0].clone(),
            create_selection()[0].clone()
        ])),
        "revise" => Some(revise_selection("I am a warm greeter.")),
        "revisecreate" => Some(json!([
            create_selection()[0].clone(),
            revise_selection("I am a warm greeter.")[0].clone()
        ])),
        "tworevise" => Some(json!([
            revise_selection("I am a warm greeter.")[0].clone(),
            revise_selection("I am a quiet greeter.")[0].clone()
        ])),
        _ => None,
    }
}

/// The scripted answer to one request, the run's `attempt`th select call
/// when it is one, or `None` to answer 500.
fn answer(target: &str, body: &Json, attempt: usize) -> Option<String> {
    let route = target.trim_start_matches('/').split('/').next()?;
    if !target.ends_with("/v1/chat/completions") {
        return None;
    }
    let schema = body.pointer("/tools/0/function/name")?.as_str()?;
    match schema {
        "dispatch" => dispatch_reply(route, attempt, body).map(|args| completion(&args)),
        "topology_plan" => Some(completion(PLAN)),
        "scxml_draft" if route == "buildfail" || route == "changefail" => {
            Some(completion(&draft(&rejected_behavior())))
        }
        "scxml_draft" if route == "genfail" => None,
        "scxml_draft" if route == "livebound" => {
            Some(completion(&scripted_draft(GROUNDED, GROUNDED_SCRIPT)))
        }
        "scxml_draft" if route == "livehardcoded" => {
            Some(completion(&draft(&hardcoded_behavior())))
        }
        "scxml_draft" => Some(completion(&draft(BEHAVIOR))),
        _ => None,
    }
}

/// The draft the `buildfail` route answers: its transition targets a state
/// that does not exist, so it never validates.
fn rejected_behavior() -> String {
    BEHAVIOR.replace("target=\"answered\"", "target=\"nowhere\"")
}

#[derive(Clone, Debug)]
struct Received {
    body: Json,
}

impl Received {
    fn schema_name(&self) -> Option<&str> {
        self.body
            .pointer("/tools/0/function/name")
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
    // The embedder (decision D255): every input is the vector (3, 4, 0, …)
    // of as many components as asked.
    if target == "/embed/v1/embeddings" {
        let dimensions = body["dimensions"].as_u64().unwrap_or(2);
        let vector: Vec<f64> = (0..dimensions)
            .map(|index| {
                [3.0, 4.0]
                    .get(usize::try_from(index).unwrap_or(2))
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect();
        let data: Vec<Json> = (0..body["input"].as_array().map_or(0, Vec::len))
            .map(|index| json!({"object": "embedding", "index": index, "embedding": vector}))
            .collect();
        let text = json!({"object": "list", "data": data}).to_string();
        log.lock().expect("the log lock").push(Received { body });
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
            text.len()
        );
        return;
    }
    // The builder's generations on the slow routes stream slowly but
    // steadily (seven pieces 1.5 s apart), or stall after their first
    // piece.
    let schema = body
        .pointer("/tools/0/function/name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    for (route, stall) in [
        ("slowbuild", false),
        ("stallbuild", true),
        ("slowplain", false),
    ] {
        let content = match schema.as_str() {
            "topology_plan" => PLAN.to_owned(),
            "scxml_draft" => draft(BEHAVIOR),
            _ => continue,
        };
        if target == format!("/{route}/v1/chat/completions") {
            log.lock()
                .expect("the log lock")
                .push(Received { body: body.clone() });
            support::answer_slowly(
                &mut stream,
                &body,
                &content,
                7,
                Duration::from_millis(1_500),
                stall && schema == "scxml_draft",
            );
            return;
        }
    }
    let attempt = log
        .lock()
        .expect("the log lock")
        .iter()
        .filter(|request| request.schema_name() == Some("dispatch"))
        .count();
    let reply = answer(&target, &body, attempt).map(|text| support::answer_as_asked(&body, text));
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

    /// The requests requiring a call of the schema `name`'s function, in order.
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

/// The harness's SQLite database, in the directory each run is granted.
const SQLITE: &str = "/memory/floppy.sqlite";

/// Lays out the scratch copy with the harness seeded by `seed` (a
/// statements expression) on `database` and an env file for `route`;
/// returns the harness's path and the env file's.
fn layout(mock: &Mock, route: &str, seed: &str, database: &str) -> (PathBuf, PathBuf) {
    layout_bounded(mock, route, seed, database, "({})")
}

/// [`layout`], the harness passing the reflection `bounds` (the
/// cognition's `<param>` of the bot's time bounds, decision D101).
fn layout_bounded(
    mock: &Mock,
    route: &str,
    seed: &str,
    database: &str,
    bounds: &str,
) -> (PathBuf, PathBuf) {
    let dir = unique_scratch("reflection");
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    support::copy_tree(&workspace().join("bots/builder"), &dir.join("bots/builder"));
    let harness = std::fs::read_to_string(
        workspace().join("tests/fixtures/phase5/reflection_harness.scx"),
    )
    .expect("the harness is readable")
    .replace(
        r#"<data id="seed" expr="[]"/>"#,
        &format!(r#"<data id="seed" expr="{seed}"/>"#),
    )
    .replace(
        r#"<param name="model" expr="'fake-model'"/>"#,
        &format!(
            r#"<param name="model" expr="'fake-model'"/><param name="bounds" expr="{bounds}"/>"#
        ),
    );
    let path = dir.join("abilities/reflection_harness.scx");
    std::fs::write(&path, harness).expect("the harness is writable");
    support::use_database(&path, database);
    let env = dir.join("fake.env");
    // The slow routes' backend streams (decision D101).
    let stream = if route == "slowbuild" || route == "stallbuild" {
        "FLOPPY_STREAM=true\n"
    } else {
        ""
    };
    std::fs::write(
        &env,
        format!(
            "{FAKE_KEYS}FLOPPY_BASE_URL={origin}/{route}/v1\nTYPESAFE_BASE_URL={origin}/{route}\n{stream}",
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
            {"event": "bot.ability.communication.respond", "data": {"parts": [{"words": "Hello!"}]}, "reason": "a knock", "target": "communication"}
        ],
    })
}

/// Sends one turn and returns its answer's records through the logged
/// inventory and episodes.
fn reflect(run: &mut Run, turn: u64) -> Vec<Json> {
    reflect_on(run, &input(turn))
}

/// [`reflect`] with the reflection input `input`.
fn reflect_on(run: &mut Run, input: &Json) -> Vec<Json> {
    run.event("bot.ability.reflection.input", input);
    run.until(WAIT, |record| {
        record["type"] == "log" && record["label"] == "episodes"
    })
}

/// [`input`] with `fields` added: a failed or cancelled turn's outcome
/// (decision D139) or what the bot knows of itself (decision D145).
fn input_with(turn: u64, fields: &Json) -> Json {
    let mut input = input(turn);
    input
        .as_object_mut()
        .expect("the input is an object")
        .extend(
            fields
                .as_object()
                .expect("the fields are an object")
                .clone(),
        );
    input
}

/// A bot with a persona: its owner's traits and the description it wrote.
fn persona_self() -> Json {
    json!({"self": {"traits": ["friendly"], "description": "I like company."}})
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

/// The turn's episode when it applied no behavior: an empty selection, or
/// a reflection that failed (decision D97).
fn unapplied_episode() -> Json {
    json!({
        "focus": "door",
        "focus_candidates": ["door"],
        "stimulus_name": "environment.sound",
        "heard": [],
        "output": input(1)["cognition_output"],
        "behavior": null,
    })
}

/// The logged episodes when `episode` is the only one stored.
fn stored(episode: &Json) -> Json {
    json!([{"context_ref": "door", "scope": "memory", "kind": "task", "content": episode}])
}

/// The path the builder writes `GreetOnKnock`'s documents under, the row's
/// source (decision D121).
const GREET_PATH: &str = "../../behaviors/GreetOnKnock.scx";

/// The behaviors directory of the layout whose harness is `harness`: where
/// the builder writes, the run's files grant (`support::behaviors_grant`).
fn behaviors(harness: &Path) -> PathBuf {
    harness
        .parent()
        .and_then(Path::parent)
        .expect("the harness is <layout>/abilities/reflection_harness.scx")
        .join("behaviors")
}

/// A seed of one ACTIVE turn behavior named `GreetOnKnock` whose program the
/// builder wrote ([`GREET_PATH`]).
const SEED_STORED: &str = "[{sql: &quot;INSERT INTO bot_behavior (name, source, description, status) VALUES ('GreetOnKnock', '../../behaviors/GreetOnKnock.scx', 'Say hello.', 'ACTIVE')&quot;}, {sql: &quot;INSERT INTO bot_behavior_trigger (behavior_name, \\&quot;trigger\\&quot;) VALUES ('GreetOnKnock', 'environment.sound')&quot;}]";

/// A seed of one ACTIVE turn behavior named `GreetOnKnock`, used once.
const SEED_BEHAVIOR: &str = "[{sql: &quot;INSERT INTO bot_behavior (name, source, description, status, used_count, last_used_at) VALUES ('GreetOnKnock', '&lt;scxml/&gt;', 'Say hello.', 'ACTIVE', '1', '2026-10-09T00:00:00Z')&quot;}, {sql: &quot;INSERT INTO bot_behavior_trigger (behavior_name, \\&quot;trigger\\&quot;) VALUES ('GreetOnKnock', 'environment.sound')&quot;}]";

/// The select step: with nothing selected the turn's episode is
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

/// An episode keeps the words its stimulus carried (mosfet.py's
/// `EpisodeData.heard`), each message of a text type's content once and in
/// order, whatever else the stimulus holds (#85), so a later turn recalls
/// what was said even when the bot said nothing back.
#[test]
fn a_turns_episode_keeps_the_words_its_stimulus_carried() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "none", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let message = |content: &str, content_type: &str| json!({"direction": "inbound", "content": content, "content_type": content_type, "participants": [{"ref": "gabe", "role": "source"}]});
    let mut turn = input(1);
    turn["cognition_input"]["stimulus"] = json!({
        "event": "bot.perceived",
        "data": {"percepts": [
            {"stimulus": {"event": "bot.ability.conversation.output", "data": {"messages": [
                message("My dog's name is Rufus.", "text/plain"),
                message("{\"a\":1}", "application/json"),
            ]}}},
            {"stimulus": {"event": "bot.ability.conversation.output", "data": {"messages": [
                message("  My dog's name is Rufus.  ", "text/plain"),
                message("And my favorite color is green.", "text/plain"),
                message("   ", "text/plain"),
            ]}}},
        ]},
    });
    turn["cognition_output"] =
        json!([{"event": "bot.ability.cognition.ignore", "data": {}, "target": "cognition"}]);
    let records = reflect_on(&mut run, &turn);
    drop(run.close(WAIT));
    let episodes = json_log(&records, "episodes");
    assert_eq!(
        episodes[0]["content"]["heard"],
        json!(["My dog's name is Rufus.", "And my favorite color is green."])
    );
}

/// A turn's episode is stored with its words embedded as a document
/// (decision D255): the Embedding ability asks the embedder for the heard
/// words with the document prefix, and the episode's vector is kept in
/// `bot_memory_embedding` as its compact text (`i8:` and base64 of signed
/// bytes); an episode without words asks nothing.
#[test]
fn a_turns_episode_is_stored_with_its_words_embedded() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "none", "[]", SQLITE);
    let inspected = std::fs::read_to_string(&harness)
        .expect("the harness is readable")
        .replace(
            "SELECT context_ref, scope, kind, content FROM bot_memory WHERE",
            "SELECT context_ref, scope, kind, content, (SELECT e.embedding FROM bot_memory_embedding e WHERE e.memory_id = bot_memory.memory_id AND e.kind = 'text') AS embedding FROM bot_memory WHERE",
        );
    std::fs::write(&harness, inspected).expect("the harness is writable");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&env)
        .expect("the env file opens");
    writeln!(file, "FLOPPY_EMBEDDING_BASE_URL={}/embed/v1", mock.origin)
        .expect("the env file is writable");
    drop(file);
    let mut run = start(&harness, &env);
    let mut turn = input(1);
    turn["cognition_input"]["stimulus"] = json!({
        "event": "bot.ability.conversation.output",
        "data": {"messages": [{"direction": "inbound", "content": "My dog's name is Rufus.", "content_type": "text/plain"}]},
    });
    let first = reflect_on(&mut run, &turn);
    let second = reflect(&mut run, 2);
    drop(run.close(WAIT));
    let episodes = json_log(&second, "episodes");
    let first_vector = json_log(&first, "episodes")[0]["embedding"].clone();
    // (3, 4, 0, …) as a unit vector is (0.6, 0.8, 0, …): bytes 76, 102, 0, ….
    let vector = first_vector.as_str().expect("the episode has a vector");
    assert!(vector.starts_with("i8:TGYA"), "{vector}");
    assert_eq!(vector.len(), 3 + 256 / 3 * 4 + 4, "{vector}");
    assert_eq!(episodes[0]["embedding"], first_vector);
    // The knock carried no words: nothing was embedded.
    assert_eq!(episodes[1]["embedding"], Json::Null);
    let asked: Vec<Json> = mock
        .received
        .lock()
        .expect("the log lock")
        .iter()
        .filter(|request| request.body.get("input").is_some())
        .map(|request| request.body.clone())
        .collect();
    assert_eq!(
        asked,
        [
            json!({"dimensions": 256, "input": ["title: none | text: My dog's name is Rufus."], "model": "embeddinggemma-2"})
        ]
    );
}

/// An episode keeps the images its stimulus carried (decision D252), each
/// once and in order, so a later turn recalls what floppy saw.
#[test]
fn a_turns_episode_keeps_the_images_its_stimulus_carried() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "none", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let view = |image: &str, media_type: &Json| json!({"image": image, "media_type": media_type, "kind": "camera"});
    let mut turn = input(1);
    turn["cognition_input"]["stimulus"] = json!({
        "event": "bot.perceived",
        "data": {"percepts": [
            {"stimulus": {"event": "environment.visual", "data": {"image": "media/a.png", "media_type": "image/png", "source": "camera"}}},
            {"stimulus": {"event": "bot.ability.seeing.sight", "data": {"cause": "look", "views": [
                view("media/a.png", &json!("image/png")),
                view("media/b.jpg", &Json::Null),
            ]}}},
        ]},
    });
    turn["cognition_output"] =
        json!([{"event": "bot.ability.cognition.ignore", "data": {}, "target": "cognition"}]);
    let records = reflect_on(&mut run, &turn);
    drop(run.close(WAIT));
    let episodes = json_log(&records, "episodes");
    // Each image once, as its reference and media type.
    assert_eq!(
        episodes[0]["content"]["images"],
        json!([{"image": "media/a.png", "media_type": "image/png"}, {"image": "media/b.jpg", "media_type": null}])
    );
}

fn an_empty_selection_stores_the_episode_and_the_next_turn_recalls_it_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "none", "[]", database);
    let mut run = start(&harness, &env);
    let first = reflect(&mut run, 1);
    assert_eq!(texts(&first, "harness.output"), ["answered"]);
    assert_eq!(texts(&first, "reflection.selected"), ["none"]);
    assert_eq!(json_log(&first, "inventory"), json!([]));
    let episode = unapplied_episode();
    assert_eq!(json_log(&first, "episodes"), stored(&episode));
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
    let prompt = |index: usize| -> Json { read(&selects[index].message("user")) };
    assert_eq!(prompt(0)["prior_episodes"], Json::Null);
    assert_eq!(prompt(0)["behaviors"], Json::Null);
    assert_eq!(
        prompt(0)["cognition_input"],
        rendered("cognition_input", &input(1)["cognition_input"])
    );
    assert_eq!(
        prompt(1)["prior_episodes"],
        rendered("prior_episodes", &json!([episode]))
    );
    // With an empty inventory only a create is offered: nothing could be
    // changed or broken (decision D100).
    assert_eq!(
        offered_names(&selects[0].body),
        json!({"bot.behavior.create": null})
    );
}

/// The inventory events the select call offers, each with the `enum` of
/// its `name`, if any (`null`: any name).
fn offered_names(body: &Json) -> Json {
    let schema = &body["tools"][0]["function"]["parameters"];
    schema["properties"]["events"]["items"]["anyOf"]
        .as_array()
        .expect("one branch per offered event")
        .iter()
        .map(|branch| {
            let event = branch["properties"]["event"]["enum"][0]
                .as_str()
                .expect("the branch's event")
                .to_owned();
            (
                event,
                branch["properties"]["data"]["properties"]["name"]["enum"].clone(),
            )
        })
        .collect::<Map<String, Json>>()
        .into()
}

/// A break: the row is kept, BROKEN with the trimmed reason,
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
    let prompt = read(&mock.requests("dispatch")[0].message("user"));
    assert_eq!(prompt["behaviors"][0]["name"], "GreetOnKnock");
    // A change and a break are offered while the inventory holds a
    // behavior (decisions D100, D243); a name is not an enum of it.
    assert_eq!(
        offered_names(&mock.requests("dispatch")[0].body),
        json!({
            "bot.behavior.create": null,
            "bot.behavior.change": null,
            "bot.behavior.break": null,
        })
    );
    assert_eq!(prompt["behaviors"][0]["used_count"], "1");
    assert_eq!(
        prompt["behaviors"][0]["triggers"],
        json!(["environment.sound"])
    );
}

/// A break of a behavior the inventory does not hold is not offered
/// (decision D100), so the generate service refuses a model that selects
/// one as schema-mismatch; the model is asked again once, refused the same
/// way, and the turn fails (the same rejection twice running,
/// decision D130), its episode still stored, with no behavior (decision
/// D97).
#[test]
fn a_break_outside_the_inventory_is_refused() {
    a_break_outside_the_inventory_is_refused_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_break_outside_the_inventory_is_refused() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    a_break_outside_the_inventory_is_refused_on(&server.database());
}

fn a_break_outside_the_inventory_is_refused_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "breakunknown", "[]", database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    let failed = texts(&records, "harness.failed");
    assert_eq!(failed.len(), 1, "{records:?}");
    assert!(failed[0].starts_with("Selection rejected: "), "{failed:?}");
    assert_eq!(mock.requests("dispatch").len(), 2);
    assert_eq!(json_log(&records, "episodes"), stored(&unapplied_episode()));
}

/// The select returns at most one inventory event: the dispatch
/// tool's `events` admits one (`<processing:send max="1">`), so the
/// generate service refuses a reply with two before anything is sent; the
/// model, asked again, repeats it, and the same rejection twice running
/// fails the turn (decision D130).
#[test]
fn more_than_one_selection_is_refused_twice_and_fails() {
    more_than_one_selection_is_refused_twice_and_fails_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_more_than_one_selection_is_refused_twice_and_fails() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    more_than_one_selection_is_refused_twice_and_fails_on(&server.database());
}

fn more_than_one_selection_is_refused_twice_and_fails_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "two", SEED_BEHAVIOR, database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(mock.requests("dispatch").len(), 2);
    let failed = texts(&records, "harness.failed");
    assert_eq!(failed.len(), 1, "{records:?}");
    assert!(failed[0].starts_with("Selection rejected: "), "{failed:?}");
    assert_eq!(texts(&records, "reflection.rejected").len(), 2);
    assert_eq!(json_log(&records, "inventory")[0]["status"], "ACTIVE");
}

/// The select repair (at most two repairs;
/// decision D130): the live run's create without `data`, which the
/// generate service refuses as schema-mismatch, is asked again with the
/// same system prompt and the repair feedback after the select input,
/// carrying the rejection's message, and the repaired create is built.
#[test]
fn a_refused_selection_is_asked_again_with_its_diagnostic_and_learns() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "repaired", "[]", SQLITE);
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
    let rejected = texts(&records, "reflection.rejected");
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert!(
        rejected[0].starts_with("attempt 0: schema-mismatch:"),
        "{rejected:?}"
    );
    let selects = mock.requests("dispatch");
    assert_eq!(selects.len(), 2);
    assert_eq!(selects[1].message("system"), selects[0].message("system"));
    let first = selects[0].message("user");
    assert!(first.ends_with("</reflection_input>"), "{first}");
    let repair = selects[1].message("user");
    assert!(
        repair.starts_with(&format!(
            "{first}\nThe previous event selection was rejected during dispatch. Repair the selection and try again."
        )),
        "{repair}"
    );
    assert!(
        repair.contains("\n----- BEGIN UNTRUSTED VALIDATOR DIAGNOSTIC -----\nSelection rejected: "),
        "{repair}"
    );
    assert!(
        repair.ends_with("\n----- END UNTRUSTED VALIDATOR DIAGNOSTIC -----"),
        "{repair}"
    );
    assert_eq!(
        json_log(&records, "episodes")[0]["content"]["behavior"]["name"],
        "GreetOnKnock"
    );
}

/// Three different rejections: reflection repairs two and fails on the
/// third with its message (decision D130), and the turn's episode is
/// stored with no behavior (decision D97).
#[test]
fn three_different_refusals_fail_after_two_repairs() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "rejections", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(mock.requests("dispatch").len(), 3);
    let failed = texts(&records, "harness.failed");
    assert_eq!(failed.len(), 1, "{records:?}");
    assert!(
        failed[0].starts_with("Selection rejected: the arguments are not JSON"),
        "{failed:?}"
    );
    let rejected = texts(&records, "reflection.rejected");
    assert_eq!(rejected.len(), 3, "{rejected:?}");
    for (entry, prefix) in rejected.iter().zip([
        "attempt 0: not-json",
        "attempt 1: schema-mismatch:",
        "attempt 2: not-json",
    ]) {
        assert!(entry.starts_with(prefix), "{rejected:?}");
    }
    assert!(mock.requests("scxml_draft").is_empty());
    assert_eq!(json_log(&records, "episodes"), stored(&unapplied_episode()));
}

/// Two rejections at the same place with different messages are not the
/// same rejection: the whole diagnostic is compared, so the
/// second is repaired too and the create is built (decision D130).
#[test]
fn a_refusal_at_the_same_place_with_another_message_is_repaired_again() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "samekey", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.output"),
        ["answered"],
        "{records:?}"
    );
    let rejected = texts(&records, "reflection.rejected");
    assert_eq!(rejected.len(), 2, "{rejected:?}");
    let key = |entry: &str| entry.split_once(": ").map(|(_, key)| key.to_owned());
    assert_eq!(key(&rejected[0]), key(&rejected[1]), "{rejected:?}");
    let selects = mock.requests("dispatch");
    assert_eq!(selects.len(), 3);
    let diagnostic = |n: usize| {
        let prompt = selects[n].message("user");
        prompt
            .split_once("Selection rejected: ")
            .map_or_else(|| panic!("{prompt}"), |(_, rest)| rest.to_owned())
    };
    assert_ne!(diagnostic(1), diagnostic(2));
    assert_eq!(
        json_log(&records, "episodes")[0]["content"]["behavior"]["name"],
        "GreetOnKnock"
    );
}

/// The repair count and the rejection being repaired start over with each
/// turn: the second turn's first select carries no feedback.
#[test]
fn each_turn_starts_its_select_unrepaired() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "repairedtwice", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let first = reflect(&mut run, 1);
    let second = reflect(&mut run, 2);
    drop(run.close(WAIT));
    assert_eq!(texts(&first, "harness.output"), ["answered"], "{first:?}");
    assert_eq!(texts(&second, "harness.output"), ["answered"], "{second:?}");
    let selects = mock.requests("dispatch");
    assert_eq!(selects.len(), 4);
    // The second turn's first select prompt is its input alone.
    let unrepaired = selects[2].message("user");
    assert!(unrepaired.ends_with("</reflection_input>"), "{unrepaired}");
    let rejected = texts(&second, "reflection.rejected");
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert!(
        rejected[0].starts_with("attempt 0: not-json"),
        "{rejected:?}"
    );
}

/// A revision through the builder: a create stores the built document
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

/// A create whose data has no `event` (a live model leaves the defaulted
/// discriminator out) is stored and reported like any other: the applied
/// behavior names its event from the inventory event it came as, so the
/// revision's answer passes its schema and the turn settles.
#[test]
fn a_create_without_its_event_field_is_built_stored_and_reported() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "createbare", "[]", SQLITE);
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
    let behavior = &json_log(&records, "episodes")[0]["content"]["behavior"];
    assert_eq!(behavior["event"], "bot.behavior.create", "{behavior}");
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
            "name": "GreetOnKnock", "source": GREET_PATH,
            "description": "Say hello when a knock sound arrives.",
            "lifetime": "turn", "status": "ACTIVE", "status_reason": "",
            "used_count": "0", "failed_count": "0", "triggers": ["environment.sound"],
        }])
    );
    let behavior = &json_log(&records, "episodes")[0]["content"]["behavior"];
    assert_eq!(behavior["name"], "GreetOnKnock");
    assert_eq!(behavior["triggers"], json!(["environment.sound"]));
    assert_eq!(behavior["source"], GREET_PATH);
    // The row's source is the path of the documents the builder wrote
    // under the intent's name (decision D121).
    let written = behaviors(&harness);
    assert_eq!(
        std::fs::read_to_string(written.join("GreetOnKnock.scx")).expect("the chart is written"),
        BEHAVIOR
    );
    assert_eq!(
        std::fs::read_to_string(written.join("GreetOnKnock.json"))
            .expect("the schemas are written"),
        r#"{"$defs":{}}"#
    );
    assert_eq!(
        std::fs::read_to_string(written.join("GreetOnKnock.js")).expect("the script is written"),
        ""
    );
    // The builder was asked once, with the intent, the DRAFT stub, and the
    // turn behavior contract.
    let drafts = mock.requests("scxml_draft");
    assert_eq!(drafts.len(), 1);
    let prompt = read(&drafts[0].message("user"));
    let spec = prompt["spec"].as_str().expect("the spec");
    assert!(
        spec.starts_with("Author or revise the SCXML program"),
        "{spec}"
    );
    let context = &prompt["context"];
    assert_eq!(context["existing_behavior"]["status"], "DRAFT", "{prompt}");
    assert_eq!(context["intent"]["name"], "GreetOnKnock", "{prompt}");
    // The persistent routine contract carries the at schedule's
    // next_after for the routine's data model.
    assert!(
        prompt["constraints"][1]
            .as_str()
            .is_some_and(|routine| routine
                .contains(r#"<data id="next_after" expr="(at, now) => { const hms"#)),
        "{prompt}"
    );
}

/// The turn behavior contract the builder gets names what autonomy will
/// run the behavior with: the grounded triggers it is stored under (the
/// stimulus events autonomy matches), so the behavior tests no event name,
/// and this turn's `input` exactly as autonomy builds it (the stimulus
/// data's fields with `focus` and `focus_candidates`; no stimulus envelope,
/// no event name). The live run of 2026-10-12 (docs/live-proof.md) built a
/// behavior that tested a stimulus event it never receives, because the
/// contract said `input` holds "the stimulus".
#[test]
fn the_builder_is_told_the_triggers_and_the_input_the_behavior_runs_with() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "create", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "reflection.inventory"),
        [r#"GreetOnKnock ACTIVE turn ["environment.sound"] (accept)"#]
    );
    let drafts = mock.requests("scxml_draft");
    assert_eq!(drafts.len(), 1);
    let prompt = read(&drafts[0].message("user"));
    let contract = prompt["constraints"][0]
        .as_str()
        .expect("the turn behavior contract")
        .to_owned();
    assert!(contract.starts_with("Write a turn behavior"), "{contract}");
    assert!(
        contract.contains("its triggers (environment.sound)"),
        "{contract}"
    );
    assert!(
        contract.contains(
            "for this turn input is <input focus=\"door\" label=\"knock\">\n  \
             <focus_candidates count=\"1\">\n    <item>door</item>\n  </focus_candidates>\n</input>"
        ),
        "{contract}"
    );
    assert!(!contract.contains("and the stimulus"), "{contract}");
    // A conversation's output carries the whole history: the behavior
    // decides from the message this turn answers, and a turn it does not
    // apply to is unhandled (decision D96; the 2026-10-12 live run's
    // behavior answered every later turn because an earlier message
    // matched).
    assert!(
        contract.contains(
            "parent.data is the message this turn answers and messages is the whole conversation so far"
        ),
        "{contract}"
    );
    assert!(
        contract.contains("never handled true with an empty list"),
        "{contract}"
    );
    // The builder runs it on this turn itself (decision D131), so its own
    // tests no longer stand for that run.
    assert!(
        contract.contains("The builder runs it once itself"),
        "{contract}"
    );
    assert!(
        !contract.contains("Its tests drive no input events and expect it to finish."),
        "{contract}"
    );
}

/// The live-turn check runs a turn behavior once on the live turn
/// (decision D131): a draft that answers only a knock read
/// from its param `input` is unhandled under its own test (no param) and
/// handled on this turn, so its ACTIVE row proves reflection sent the turn
/// and its input and the builder ran the draft with them.
#[test]
fn a_create_passes_only_when_its_draft_selects_on_the_live_input() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "livebound", "[]", SQLITE);
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
    let drafts = mock.requests("scxml_draft");
    assert_eq!(drafts.len(), 1);
    let prompt = read(&drafts[0].message("user"));
    let contract = prompt["constraints"][0]
        .as_str()
        .expect("the turn behavior contract");
    assert!(
        contract.contains("whose targets are among (communication)"),
        "{contract}"
    );
    assert!(
        contract.contains("The builder runs it once itself"),
        "{contract}"
    );
}

/// A create whose draft hardcodes a value of an earlier turn's input fails
/// the run on the live turn the same way after its repair, so it is
/// abandoned as a DRAFT, with the binding message that names `input`.
#[test]
fn a_create_whose_draft_hardcodes_an_input_value_is_abandoned() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "livehardcoded", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    let failed = texts(&records, "harness.failed");
    assert_eq!(failed.len(), 1, "{records:?}");
    assert!(
        failed[0].starts_with(
            "Reflection change abandoned: attempts exhausted:\nbehavior validation failed:\n  error[E0008] apply: selection data[\"label\"]=\"doorbell\" does not match live event value \"knock\"; read identifiers from input at runtime."
        ),
        "{failed:?}"
    );
    let inventory = json_log(&records, "inventory");
    assert_eq!(inventory[0]["status"], "DRAFT", "{inventory}");
    assert_eq!(inventory[0]["status_reason"], "validation", "{inventory}");
    assert_eq!(json_log(&records, "episodes"), stored(&unapplied_episode()));
}

/// A revision is bounded by inactivity (decision D101): the builder's
/// generations stream slowly but steadily (about 9 s each), and each stage
/// and each progress it reports starts the 8 s bound over, so a build of
/// some twenty seconds in all is accepted.
#[test]
fn a_build_that_keeps_making_progress_outlasts_the_inactivity_bound() {
    let mock = Mock::start();
    let (harness, env) = layout_bounded(
        &mock,
        "slowbuild",
        "[]",
        SQLITE,
        "({revising_timeout_ms: 8000})",
    );
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
    let generations = [mock.requests("topology_plan"), mock.requests("scxml_draft")].concat();
    assert!(!generations.is_empty());
    assert!(
        generations
            .iter()
            .all(|request| request.body["stream"] == true),
        "the builder streams its generations"
    );
}

/// A build whose generations are not streamed (the run's `FLOPPY_STREAM`
/// is unset) reports no generation progress, so the revision's inactivity
/// bound never arms: each generation of about 10 s outlasts the 8 s bound
/// and the build is accepted, bounded by the revision's backstop alone
/// (decision D101).
#[test]
fn a_build_that_is_not_streamed_is_bounded_by_the_backstop_alone() {
    let mock = Mock::start();
    let (harness, env) = layout_bounded(
        &mock,
        "slowplain",
        "[]",
        SQLITE,
        "({revising_timeout_ms: 8000})",
    );
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
    let drafts = mock.requests("scxml_draft");
    assert!(!drafts.is_empty());
    assert!(
        drafts
            .iter()
            .all(|request| request.body.get("stream").is_none())
    );
    assert!(
        drafts
            .iter()
            .all(|request| request.body["max_completion_tokens"] == 16384),
        "a whole document's tokens are bounded by document_max_completion_tokens"
    );
}

/// A build whose generation stops arriving fails the revision at the
/// inactivity bound (decision D101), with "Reflection child cancellation
/// timed out.".
#[test]
fn a_build_that_stalls_fails_at_the_inactivity_bound() {
    let mock = Mock::start();
    let (harness, env) = layout_bounded(
        &mock,
        "stallbuild",
        "[]",
        SQLITE,
        "({revising_timeout_ms: 8000})",
    );
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.failed"),
        ["Reflection child cancellation timed out."]
    );
}

/// A build that fails its checks drafts the rejected source as DRAFT with
/// `status_reason` validation, and the turn fails with the abandon message
/// and the rendered report;
/// the turn's episode is stored with no behavior (decision D97).
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
    let failed = texts(&records, "harness.failed");
    assert_eq!(failed.len(), 1, "{records:?}");
    assert!(
        failed[0].starts_with(
            "Reflection change abandoned: attempts exhausted:\nbehavior validation failed:\n  error[SCXML005] validate: "
        ),
        "{failed:?}"
    );
    assert_eq!(
        json_log(&records, "inventory"),
        json!([{
            "name": "GreetOnKnock", "source": GREET_PATH,
            "description": "Say hello when a knock sound arrives.",
            "lifetime": "turn", "status": "DRAFT", "status_reason": "validation",
            "used_count": "0", "failed_count": "0",
            "triggers": ["environment.sound", "phone.incoming_call"],
        }])
    );
    assert_eq!(
        std::fs::read_to_string(behaviors(&harness).join("GreetOnKnock.scx"))
            .expect("the rejected draft is written"),
        rejected_behavior(),
        "the DRAFT row's path names the rejected draft"
    );
    assert_eq!(json_log(&records, "episodes"), stored(&unapplied_episode()));
}

/// A change revises the stored program: reading reads its three documents
/// back through the files service (decision D121), the builder is given
/// them as the existing row's `scxml`, `schemas` and `script`, and the
/// revision is written under the same name, so the row keeps its path.
#[test]
fn a_change_reads_the_stored_documents_back_for_the_builder() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "change", SEED_STORED, SQLITE);
    let stored_dir = behaviors(&harness);
    std::fs::create_dir_all(&stored_dir).expect("the behaviors directory is creatable");
    let old = BEHAVIOR.replace("Hello!", "Hi!");
    std::fs::write(stored_dir.join("GreetOnKnock.scx"), &old).expect("writable");
    std::fs::write(
        stored_dir.join("GreetOnKnock.json"),
        r#"{"$defs": {"knock": {"type": "object"}}}"#,
    )
    .expect("writable");
    std::fs::write(stored_dir.join("GreetOnKnock.js"), "// stored\n").expect("writable");
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.output"),
        ["answered"],
        "{records:?}"
    );
    let drafts = mock.requests("scxml_draft");
    assert_eq!(drafts.len(), 1, "{records:?}");
    let prompt = read(&drafts[0].message("user"));
    let existing = &prompt["context"]["existing_behavior"];
    assert_eq!(existing["source"], GREET_PATH, "{existing}");
    assert_eq!(existing["scxml"], old.as_str(), "{existing}");
    assert_eq!(
        existing["schemas"],
        json!({"$defs": {"knock": {"type": "object"}}}),
        "{existing}"
    );
    assert_eq!(existing["script"], "// stored\n", "{existing}");
    assert_eq!(prompt["name"], "GreetOnKnock", "{prompt}");
    assert_eq!(
        json_log(&records, "inventory")[0]["source"],
        GREET_PATH,
        "the revision keeps the row's path"
    );
    assert_eq!(
        std::fs::read_to_string(stored_dir.join("GreetOnKnock.scx")).expect("readable"),
        BEHAVIOR,
        "the revision is written over the stored chart"
    );
}

/// A failed change of an ACTIVE behavior keeps it (decision D224): the
/// stored documents the builder's rejected drafts were written over are
/// written back, the row stays ACTIVE on its path, and the turn fails
/// with the abandon message.
#[test]
fn a_failed_change_of_an_active_behavior_keeps_it() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "changefail", SEED_STORED, SQLITE);
    let stored_dir = behaviors(&harness);
    std::fs::create_dir_all(&stored_dir).expect("the behaviors directory is creatable");
    let old = BEHAVIOR.replace("Hello!", "Hi!");
    let schemas = r#"{"$defs": {"knock": {"type": "object"}}}"#;
    std::fs::write(stored_dir.join("GreetOnKnock.scx"), &old).expect("writable");
    std::fs::write(stored_dir.join("GreetOnKnock.json"), schemas).expect("writable");
    std::fs::write(stored_dir.join("GreetOnKnock.js"), "// stored\n").expect("writable");
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    let failed = texts(&records, "harness.failed");
    assert_eq!(failed.len(), 1, "{records:?}");
    assert!(
        failed[0].starts_with("Reflection change abandoned: attempts exhausted:"),
        "{failed:?}"
    );
    assert!(
        texts(&records, "reflection.inventory")
            .contains(&"GreetOnKnock ACTIVE kept (change abandoned)".to_owned()),
        "{records:?}"
    );
    let row = &json_log(&records, "inventory")[0];
    assert_eq!(row["status"], "ACTIVE", "{row}");
    assert_eq!(row["source"], GREET_PATH, "{row}");
    for (file, text) in [
        ("GreetOnKnock.scx", old.as_str()),
        ("GreetOnKnock.json", schemas),
        ("GreetOnKnock.js", "// stored\n"),
    ] {
        assert_eq!(
            std::fs::read_to_string(stored_dir.join(file)).expect("readable"),
            text,
            "{file} is the working program's again"
        );
    }
}

/// A change call that fails without a draft to check (every generation
/// fails) fails the turn and drafts nothing: the create's stub stays as
/// preparing stored it. The
/// turn's episode is stored with no behavior (decision D97).
#[test]
fn a_failed_change_call_drafts_nothing() {
    a_failed_change_call_drafts_nothing_on(SQLITE);
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_failed_change_call_drafts_nothing() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    a_failed_change_call_drafts_nothing_on(&server.database());
}

fn a_failed_change_call_drafts_nothing_on(database: &str) {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "genfail", "[]", database);
    let mut run = start(&harness, &env);
    let records = reflect(&mut run, 1);
    drop(run.close(WAIT));
    let failed = texts(&records, "harness.failed");
    assert_eq!(failed.len(), 1, "{records:?}");
    assert!(
        failed[0].starts_with(
            "Reflection revision failed: attempts-exhausted: the author generation failed"
        ),
        "{failed:?}"
    );
    assert!(
        texts(&records, "reflection.inventory").is_empty(),
        "{records:?}"
    );
    assert_eq!(
        json_log(&records, "inventory"),
        json!([{
            "name": "GreetOnKnock", "source": "",
            "description": "Say hello when a knock sound arrives.",
            "lifetime": "turn", "status": "DRAFT", "status_reason": "",
            "used_count": "0", "failed_count": "0",
            "triggers": ["environment.sound", "phone.incoming_call"],
        }])
    );
    assert_eq!(json_log(&records, "episodes"), stored(&unapplied_episode()));
}

/// A failed turn and a cancelled one are remembered with their outcome and
/// nothing is selected for them; the next turn's select recalls both
/// (decision D139).
#[test]
fn failed_and_cancelled_turns_are_remembered_without_a_select() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "none", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let failure = "Cognition reasoning timed out after 30 seconds.";
    let failed = reflect_on(
        &mut run,
        &input_with(
            1,
            &json!({"cognition_output": [], "outcome": "failed", "failure": failure}),
        ),
    );
    assert_eq!(texts(&failed, "harness.output"), ["answered"], "{failed:?}");
    assert_eq!(texts(&failed, "reflection"), ["turn 1: failed"]);
    let mut failed_episode = unapplied_episode();
    failed_episode["output"] = json!([]);
    failed_episode["outcome"] = json!("failed");
    failed_episode["failure"] = json!(failure);
    assert_eq!(json_log(&failed, "episodes"), stored(&failed_episode));
    let cancelled = reflect_on(
        &mut run,
        &input_with(2, &json!({"cognition_output": [], "outcome": "cancelled"})),
    );
    assert_eq!(texts(&cancelled, "reflection"), ["turn 2: cancelled"]);
    let mut cancelled_episode = failed_episode.clone();
    cancelled_episode["outcome"] = json!("cancelled");
    cancelled_episode["failure"] = Json::Null;
    assert_eq!(
        json_log(&cancelled, "episodes")[1]["content"],
        cancelled_episode
    );
    assert!(mock.requests("dispatch").is_empty());
    let completed = reflect(&mut run, 3);
    drop(run.close(WAIT));
    assert_eq!(texts(&completed, "reflection.selected"), ["none"]);
    let selects = mock.requests("dispatch");
    assert_eq!(selects.len(), 1);
    let prompt = read(&selects[0].message("user"));
    assert_eq!(
        prompt["prior_episodes"],
        rendered(
            "prior_episodes",
            &json!([failed_episode, cancelled_episode])
        )
    );
    assert_eq!(
        json_log(&completed, "episodes")[2]["content"],
        unapplied_episode()
    );
}

/// With a persona, reflection offers its revision beside the inventory
/// events, shows the model the bot's self, and sends a selected revision
/// up for the persona (decision D145).
#[test]
fn a_selected_persona_revision_is_sent_up() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "revise", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let records = reflect_on(&mut run, &input_with(1, &persona_self()));
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.output"),
        ["answered"],
        "{records:?}"
    );
    assert_eq!(
        texts(&records, "harness.revise"),
        [revise_selection("I am a warm greeter.")[0]["data"].to_string()]
    );
    assert_eq!(json_log(&records, "episodes"), stored(&unapplied_episode()));
    let selects = mock.requests("dispatch");
    assert_eq!(selects.len(), 1);
    assert_eq!(
        offered_names(&selects[0].body),
        json!({"bot.behavior.create": null, "bot.ability.persona.revise": null})
    );
    assert_eq!(
        selects[0].body["tools"][0]["function"]["parameters"]["properties"]["events"]["maxItems"],
        2
    );
    let prompt = read(&selects[0].message("user"));
    assert_eq!(prompt["self"], rendered("self", &persona_self()["self"]));
}

/// A revision selected after a create is still sent up, and the create is
/// built (decision D145).
#[test]
fn a_revision_beside_a_create_is_sent_up_and_the_create_built() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "revisecreate", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let records = reflect_on(&mut run, &input_with(1, &persona_self()));
    drop(run.close(WAIT));
    assert_eq!(
        texts(&records, "harness.output"),
        ["answered"],
        "{records:?}"
    );
    assert_eq!(texts(&records, "harness.revise").len(), 1, "{records:?}");
    assert_eq!(
        texts(&records, "reflection.inventory"),
        [r#"GreetOnKnock ACTIVE turn ["environment.sound"] (accept)"#]
    );
}

/// Two revisions are an over-selection, refused as the model's to repair:
/// the same refusal again fails the turn (decisions D130, D145).
#[test]
fn two_persona_revisions_are_refused() {
    let mock = Mock::start();
    let (harness, env) = layout(&mock, "tworevise", "[]", SQLITE);
    let mut run = start(&harness, &env);
    let records = reflect_on(&mut run, &input_with(1, &persona_self()));
    drop(run.close(WAIT));
    assert!(texts(&records, "harness.revise").is_empty(), "{records:?}");
    assert_eq!(
        texts(&records, "reflection.rejected"),
        ["attempt 0: over-selection", "attempt 1: over-selection"]
    );
    assert_eq!(
        texts(&records, "harness.failed"),
        [
            "Selection rejected: select at most one of bot.behavior.create, bot.behavior.change or bot.behavior.break, and at most one bot.ability.persona.revise"
        ]
    );
    assert_eq!(mock.requests("dispatch").len(), 2);
}
