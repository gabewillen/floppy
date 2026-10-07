//! Harness of the builder bot (`bots/builder/builder.scx`), run exactly as
//! its README says: `scx run --env-file <file> bots/builder/builder.scx`,
//! with the `builder.request` on standard input. Its packages resolve from
//! their repository URIs (the test cache `scripts/build-components.sh`
//! published), its `env` attributes scope the values, and it ends in the
//! final state `done` or `failed`, whose `<donedata>` `scx run` prints in its
//! `done` record. The session binds the real `scx` (embedded in `scx`) and `decision`
//! extension components and the real `generate` and `decision` service
//! components; the services talk to one deterministic fake backend on
//! 127.0.0.1 (Chat Completions and `TypeSafe` `system_one`; nothing reaches
//! the network) whose canned drafts include an invalid one, so the repair
//! loop, the machine-applicable fixes, the decision, and the simulation as
//! an invoked child all run. One route answers its first plan late, so a
//! `builder.timeout` fires while that generation is in flight and cancels
//! it.
//!
//! The ignored `live_builder_through_an_env_file` test builds the same spec
//! with the real backends: `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p
//! floppy-tests --test builder_bot -- --ignored`, the paths joined like
//! `PATH` when `CPA_API_KEY` and `TYPESAFE_API_KEY` live in separate files.
//! The values are the files'; without a `CPA_BASE_URL` or
//! `TYPESAFE_BASE_URL`, each service uses its package's declared origin
//! (decision D18). The model is `FLOPPY_LIVE_MODEL`, else
//! `sparks/qwen3.8-flash-next`. It writes the produced bot under `target/builder-live/`, and prints pass or
//! fail and counts only; no value is ever printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value as Json, json};
use support::{Run, output, scratch, workspace, write};

/// The fake keys (not secrets): the mock checks they arrive as bearers.
const FAKE_CPA_KEY: &str = "sk-floppy-fake";
const FAKE_TYPESAFE_KEY: &str = "ts-floppy-fake";
const SPEC: &str = "a traffic light with a pedestrian button";

fn builder_dir() -> PathBuf {
    workspace().join("bots/builder")
}

fn builder_source() -> String {
    std::fs::read_to_string(builder_dir().join("builder.scx")).expect("builder.scx is readable")
}

/// The builder document the README runs.
fn builder_document() -> PathBuf {
    PathBuf::from("bots/builder/builder.scx")
}

/// The flags of the builder's README command on `clock`, running
/// `document` (the builder, or a variant of it).
fn builder_args(document: &Path, clock: &str) -> Vec<String> {
    vec![
        "--clock".to_owned(),
        clock.to_owned(),
        document.display().to_string(),
    ]
}

/// Starts `document` with `env_file` and sends it `request`.
fn start(
    document: &Path,
    clock: &str,
    env_files: &[PathBuf],
    request: &Json,
    env: &[(&str, &str)],
) -> Run {
    let mut args = builder_args(document, clock);
    for env_file in env_files {
        args.extend(["--env-file".to_owned(), env_file.display().to_string()]);
    }
    // The base URLs fall back to the calling shell's values (`env`).
    for (name, _) in env {
        args.extend(["--env".to_owned(), (*name).to_owned()]);
    }
    let mut run = Run::start(&args, env);
    run.event("builder.request", request);
    run.end_input();
    run
}

fn finished(records: &[Json]) -> bool {
    support::finished(records)
}

/// The builder's answer: the final state it ended in (`done` or `failed`,
/// its last progress `log` label) and that state's done data (the root's
/// `done` record).
fn answer(records: &[Json]) -> (String, Json) {
    let root: Vec<&Json> = records
        .iter()
        .filter(|record| record.get("parent").is_none())
        .collect();
    let done = root
        .iter()
        .find(|record| record["type"] == "done")
        .unwrap_or_else(|| panic!("the builder finished with done data: {records:?}"));
    let label = root
        .iter()
        .filter(|record| record["type"] == "log")
        .filter_map(|record| record["label"].as_str())
        .next_back()
        .expect("the builder logs its final state")
        .to_owned();
    assert!(
        matches!(label.as_str(), "done" | "failed"),
        "the last progress log names the final state: {label}"
    );
    (label, done["data"].clone())
}

/// `scx validate` of `path` with `bindings`: the exit status and the
/// diagnostics.
fn validate(path: &Path, bindings: &[String]) -> support::Output {
    let mut args = vec!["validate".to_owned(), path.display().to_string()];
    args.extend_from_slice(bindings);
    output(&args)
}

/// Validates and compiles `scxml` with no package (as the builder's drafts
/// are): whether it compiled.
fn compiles(name: &str, scxml: &str) -> bool {
    let dir = support::unique_scratch("compile");
    let path = write(&dir, name, scxml);
    let compiled = output(&[
        "compile".to_owned(),
        path.display().to_string(),
        "--output".to_owned(),
        dir.join("out.scxml-ir.json").display().to_string(),
    ]);
    compiled.status.success()
}

// ---------------------------------------------------------------------------
// Canned backend answers.

const PLAN: &str = r#"{"name":"traffic_light","summary":"A vehicle light that cycles green, yellow, red, and serves a pedestrian button by ending green early.","states":[{"id":"green","kind":"atomic","parent":"","purpose":"Vehicles go."},{"id":"yellow","kind":"atomic","parent":"","purpose":"Vehicles prepare to stop."},{"id":"red","kind":"atomic","parent":"","purpose":"Vehicles stop; pedestrians walk."}],"events":[{"name":"pedestrian.button","direction":"input","description":"A pedestrian pressed the button."},{"name":"light.green","direction":"output","description":"The light turned green."},{"name":"light.yellow","direction":"output","description":"The light turned yellow."},{"name":"light.red","direction":"output","description":"The light turned red."},{"name":"walk.on","direction":"output","description":"Pedestrians may cross."},{"name":"light.timer","direction":"internal","description":"The phase timer expired."}],"transitions":[{"source":"green","event":"pedestrian.button","target":"yellow","description":"End green early."},{"source":"green","event":"light.timer","target":"yellow","description":"Green expired."},{"source":"yellow","event":"light.timer","target":"red","description":"Yellow expired."},{"source":"red","event":"light.timer","target":"green","description":"Red expired."}]}"#;

/// The plan the slow route answers too late: the builder has cancelled
/// that generation by then, so it never reaches the report.
const LATE_PLAN: &str = r#"{"name":"late_plan","summary":"Answered after the timeout.","states":[],"events":[],"transitions":[]}"#;

/// The longest the slow route holds its first plan answer while it waits
/// for the builder to ask for the redesign: a hang guard only. The answer is
/// released by the redesign request, so no assertion depends on time.
const SLOW_PLAN_HOLD_BOUND: Duration = Duration::from_mins(3);

const BUTTON_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;

/// The traffic light with `{green}` as its green state's transitions and
/// `{yellow_target}` as the target of yellow's timer.
fn traffic_light(green: &str, yellow_target: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" name="traffic_light" initial="green">
  <state id="green">
    <onentry><send target="#_parent" event="light.green"/><send id="green-timer" event="light.timer" delay="30s"/></onentry>
    <onexit><cancel sendid="green-timer"/></onexit>
    {green}
  </state>
  <state id="yellow">
    <onentry><send target="#_parent" event="light.yellow"/><send id="yellow-timer" event="light.timer" delay="3s"/></onentry>
    <onexit><cancel sendid="yellow-timer"/></onexit>
    <transition event="light.timer" target="{yellow_target}"/>
  </state>
  <state id="red">
    <onentry><send target="#_parent" event="light.red"/><send target="#_parent" event="walk.on"/><send id="red-timer" event="light.timer" delay="20s"/></onentry>
    <onexit><cancel sendid="red-timer"/><send target="#_parent" event="walk.off"/></onexit>
    <transition event="light.timer" target="green"/>
  </state>
</scxml>"##
    )
}

/// The traffic light's green transitions, separately.
fn green_separate() -> String {
    format!(
        r#"<transition event="pedestrian.button" event:schema='{BUTTON_SCHEMA}' target="yellow"/>
    <transition event="light.timer" target="yellow"/>"#
    )
}

/// The same transitions merged with a comma-separated descriptor list
/// (`SCXML013`, whose fix is machine-applicable).
fn green_comma() -> String {
    format!(
        r#"<transition event="pedestrian.button,light.timer" event:schema='{BUTTON_SCHEMA}' target="yellow"/>"#
    )
}

/// The test scripts every draft carries.
fn tests() -> Json {
    json!([
        {
            "name": "button ends green early",
            "events": [{"name": "pedestrian.button", "data": "{}", "delay_ms": 1000}],
            "settle_ms": 6000,
            "expect_events": ["light.green", "light.yellow", "light.red", "walk.on"],
            "expect_done": false,
        },
        {
            "name": "the light starts green",
            "events": [],
            "settle_ms": 2000,
            "expect_events": ["light.green"],
            "expect_done": false,
        },
    ])
}

/// A draft: the document, the button's schema, and two test scripts.
fn draft(scxml: &str) -> String {
    json!({
        "scxml": scxml,
        "schemas": [{"event": "pedestrian.button", "schema": BUTTON_SCHEMA}],
        "tests": tests(),
    })
    .to_string()
}

/// The invalid draft: yellow's timer targets a state that does not exist.
fn invalid_draft() -> String {
    draft(&traffic_light(&green_separate(), "amber"))
}

/// A draft whose only error has a machine-applicable fix.
fn fixable_draft() -> String {
    draft(&traffic_light(&green_comma(), "red"))
}

/// A draft that breaks the draft profile: red invokes a nested machine
/// from a string.
fn smuggling_draft() -> String {
    draft(&traffic_light(&green_separate(), "red").replace(
        "<transition event=\"light.timer\" target=\"green\"/>",
        "<transition event=\"light.timer\" target=\"green\"/><invoke type=\"scxml\"><content expr=\"'&lt;scxml xmlns=&quot;http://www.w3.org/2005/07/scxml&quot; version=&quot;1.0&quot;/&gt;'\"/></invoke>",
    ))
}

/// A draft that hides the `decision` namespace behind a non-ASCII prefix:
/// run as the simulation child, it would read the run's session snapshot
/// through a package the run binds.
fn unicode_prefix_draft() -> String {
    draft(&traffic_light(&green_separate(), "red").replace(
        "version=\"1.0\"",
        "xmlns:\u{e9}=\"https://github.com/stateforward/floppy/namespaces/decision\" version=\"1.0\"",
    ).replace(
        "<onexit><cancel sendid=\"red-timer\"/>",
        "<onexit><\u{e9}:menu location=\"x\"/><cancel sendid=\"red-timer\"/>",
    ))
}

/// A draft that hides an `invoke` behind an internal entity of a document
/// type declaration.
fn doctype_draft() -> String {
    draft(&format!(
        "<!DOCTYPE scxml [<!ENTITY i \"&#60;invoke type='scxml'/&#62;\">]>{}",
        traffic_light(&green_separate(), "red").replace(
            "<transition event=\"light.timer\" target=\"green\"/>",
            "<transition event=\"light.timer\" target=\"green\"/>&i;",
        )
    ))
}

/// A valid draft whose green state sends the builder's own control event
/// names to `#_parent`; its test expects them as outputs.
fn impostor_draft() -> String {
    json!({
        "scxml": traffic_light(&green_separate(), "red").replace(
            "<onentry><send target=\"#_parent\" event=\"light.green\"/>",
            "<onentry><send target=\"#_parent\" event=\"sim.settle\"/><send target=\"#_parent\" event=\"sim.drive\"/><send target=\"#_parent\" event=\"light.green\"/>",
        ),
        "schemas": [{"event": "pedestrian.button", "schema": BUTTON_SCHEMA}],
        "tests": [{
            "name": "control names are outputs",
            "events": [],
            "settle_ms": 2000,
            "expect_events": ["sim.settle", "sim.drive", "light.green"],
            "expect_done": false,
        }],
    })
    .to_string()
}

/// A valid draft whose green state first sends `error.stale` and then
/// `builder.timeout` to `#_parent`: the error ends each test, so the
/// `builder.timeout` is still queued when the candidate's invocation is
/// cancelled.
fn stale_timeout_draft() -> String {
    draft(&traffic_light(&green_separate(), "red").replace(
        "<onentry><send target=\"#_parent\" event=\"light.green\"/>",
        "<onentry><send target=\"#_parent\" event=\"error.stale\"/><send target=\"#_parent\" event=\"builder.timeout\"/><send target=\"#_parent\" event=\"light.green\"/>",
    ))
}

/// A draft that finishes on its second `ping`, with two tests that each
/// send one `ping` and expect it not to finish: a test whose `ping` is sent
/// twice fails.
fn two_ping_draft() -> String {
    json!({
        "scxml": format!(
            r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" name="two_pings" initial="first">
  <state id="first">
    <onentry><send target="#_parent" event="ready"/></onentry>
    <transition event="ping" event:schema='{BUTTON_SCHEMA}' target="second"/>
  </state>
  <state id="second">
    <transition event="ping" event:schema='{BUTTON_SCHEMA}' target="end"/>
  </state>
  <final id="end"/>
</scxml>"##
        ),
        "schemas": [{"event": "ping", "schema": BUTTON_SCHEMA}],
        "tests": [
            {"name": "first run", "events": [{"name": "ping", "data": "{}", "delay_ms": 0}],
             "settle_ms": 1000, "expect_events": ["ready"], "expect_done": false},
            {"name": "second run", "events": [{"name": "ping", "data": "{}", "delay_ms": 0}],
             "settle_ms": 1000, "expect_events": ["ready"], "expect_done": false},
        ],
    })
    .to_string()
}

/// A streamed Chat Completions answer (server-sent events, as the generate
/// service asks for) carrying `content`, split in two deltas.
fn completion(content: &str) -> String {
    let middle = content.chars().count() / 2;
    let head: String = content.chars().take(middle).collect();
    let tail: String = content.chars().skip(middle).collect();
    let chunk = |delta: Json, finish: Option<&str>| {
        json!({
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })
    };
    [
        chunk(json!({"role": "assistant", "content": ""}), None),
        chunk(json!({"content": head}), None),
        chunk(json!({"content": tail}), None),
        chunk(json!({}), Some("stop")),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .chain(std::iter::once("data: [DONE]\n\n".to_owned()))
    .collect()
}

/// A `system_one` answer choosing `chosen` in the selection (0.8, the rest
/// share 0.2) and every other question's first label.
fn system_one(chosen: &str, body: &[u8]) -> Option<String> {
    let request: Json = serde_json::from_slice(body).ok()?;
    let questions = request.get("questions")?.as_object()?;
    let mut answers = Map::new();
    for (name, question) in questions {
        let labels: Vec<&String> = question.get("criteria")?.as_object()?.keys().collect();
        let mut probabilities = Map::new();
        let choice = if name == "selection" {
            let rest = labels
                .iter()
                .filter(|label| label.as_str() != chosen)
                .count();
            for label in &labels {
                let share = if label.as_str() == chosen {
                    0.8
                } else {
                    0.2 / f64::from(u32::try_from(rest.max(1)).ok()?)
                };
                probabilities.insert((*label).clone(), share.into());
            }
            chosen.to_owned()
        } else {
            let first = (*labels.first()?).clone();
            probabilities.insert(first.clone(), 1.0.into());
            first
        };
        answers.insert(
            name.clone(),
            json!({"type": "choice", "choice": choice, "confidence": 0.8, "probabilities": probabilities}),
        );
    }
    Some(json!({"model": "jev-latest", "answers": answers}).to_string())
}

/// The mock's answer to one request on `path` with the structured-output
/// `schema`, after `authors` draft and `plans` plan requests on that path.
fn mock_reply(
    path: &str,
    schema: Option<&str>,
    authors: usize,
    plans: usize,
    body: &[u8],
) -> Option<String> {
    match (path, schema) {
        (
            "/repair/v1/chat/completions"
            | "/broken/v1/chat/completions"
            | "/smuggle/v1/chat/completions"
            | "/unicode/v1/chat/completions"
            | "/doctype/v1/chat/completions"
            | "/impostor/v1/chat/completions"
            | "/stale/v1/chat/completions"
            | "/pings/v1/chat/completions"
            | "/progress/v1/chat/completions",
            Some("topology_plan"),
        ) => Some(completion(PLAN)),
        ("/flaky/v1/chat/completions", Some("topology_plan")) => {
            (plans != 2).then(|| completion(PLAN))
        }
        ("/fastfail/v1/chat/completions", Some("topology_plan")) => {
            (plans != 1).then(|| completion(PLAN))
        }
        // The first plan is held (see `Mock::start`) and answered late.
        ("/slow/v1/chat/completions", Some("topology_plan")) => {
            Some(completion(if plans == 1 { LATE_PLAN } else { PLAN }))
        }
        ("/slow/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&draft(&traffic_light(&green_separate(), "red"))))
        }
        ("/unicode/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&unicode_prefix_draft()))
        }
        ("/doctype/v1/chat/completions", Some("scxml_draft")) => Some(completion(&doctype_draft())),
        ("/impostor/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&impostor_draft()))
        }
        ("/stale/v1/chat/completions", Some("scxml_draft")) => Some(completion(&match authors {
            1 => stale_timeout_draft(),
            _ => draft(&traffic_light(&green_separate(), "red")),
        })),
        ("/pings/v1/chat/completions", Some("scxml_draft")) => Some(completion(&two_ping_draft())),
        ("/smuggle/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&smuggling_draft()))
        }
        ("/repair/v1/chat/completions", Some("scxml_draft")) => Some(completion(&match authors {
            1 => invalid_draft(),
            2 => fixable_draft(),
            _ => draft(&traffic_light(&green_separate(), "red")),
        })),
        ("/progress/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&match authors {
                1 => invalid_draft(),
                2 => smuggling_draft(),
                _ => draft(&traffic_light(&green_separate(), "red")),
            }))
        }
        (
            "/broken/v1/chat/completions"
            | "/flaky/v1/chat/completions"
            | "/fastfail/v1/chat/completions",
            Some("scxml_draft"),
        ) => Some(completion(&invalid_draft())),
        ("/typesafe/v1/systemone", _) => system_one("triage.repair", body),
        ("/redesign/v1/systemone", _) => system_one("triage.redesign", body),
        _ => None,
    }
}

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    path: String,
    authorization: Option<String>,
    body: Json,
    /// When it arrived.
    at: Instant,
    /// When its answer was written.
    answered: Arc<Mutex<Option<Instant>>>,
}

impl Received {
    /// The structured-output schema name of a Chat Completions request.
    fn schema_name(&self) -> Option<&str> {
        self.body
            .pointer("/response_format/json_schema/name")
            .and_then(Json::as_str)
    }

    /// The user message of a Chat Completions request, parsed (the builder
    /// sends JSON text).
    fn user_input(&self) -> Json {
        let text = self
            .body
            .get("messages")
            .and_then(Json::as_array)
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message.get("role").and_then(Json::as_str) == Some("user"))
            })
            .and_then(|message| message.get("content"))
            .and_then(Json::as_str)
            .expect("a user message");
        serde_json::from_str(text).expect("the builder's user message is JSON")
    }
}

/// The deterministic fake backend on 127.0.0.1. `generate` routes:
/// `/repair/v1` answers the plan, then the invalid draft, then the fixable
/// one, then valid drafts; `/progress/v1` answers the plan, the invalid
/// draft, a draft that breaks the draft profile, then valid drafts;
/// `/broken/v1` answers the plan, then only invalid
/// drafts; `/smuggle/v1` answers the plan, then only drafts that break the
/// draft profile (`/unicode/v1` through a non-ASCII prefix, `/doctype/v1`
/// through a DTD entity); `/impostor/v1` answers the plan, then a valid draft
/// that sends the builder's control event names; `/flaky/v1` fails the
/// second plan (404) and otherwise answers like `/broken/v1`; `/fastfail/v1`
/// fails the first plan (404) at once and otherwise answers like
/// `/broken/v1`; `/noplan/v1`
/// fails every plan (404); `/slow/v1` holds its first plan until its
/// second plan request arrives (at most [`SLOW_PLAN_HOLD_BOUND`]) and
/// answers [`LATE_PLAN`], then answers the plan and
/// valid drafts. Each connection is served on its own thread, so a held
/// answer delays nothing else. `decision` routes:
/// `/typesafe` chooses `triage.repair`, `/redesign` `triage.redesign`.
struct Mock {
    origin: String,
    /// Every request so far, and the signal that one more arrived.
    received: Arc<(Mutex<Vec<Received>>, Condvar)>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new((Mutex::new(Vec::<Received>::new()), Condvar::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log.0, &log.1));
            }
        });
        Self { origin, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.0.lock().expect("the log lock").clone()
    }
}

/// Serves one connection of the mock: reads one request, records it (and
/// signals `arrived`), answers it (holding the slow route's first plan until
/// its second plan request arrives), and records when.
fn serve(mut stream: std::net::TcpStream, log: &Mutex<Vec<Received>>, arrived: &Condvar) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let path = request_line.split(' ').nth(1).unwrap_or("").to_owned();
    let parsed: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let request = Received {
        path: path.clone(),
        authorization: headers.get("authorization").cloned(),
        body: parsed,
        at: Instant::now(),
        answered: Arc::new(Mutex::new(None)),
    };
    let count = |log: &[Received], schema: &str| {
        log.iter()
            .filter(|seen| seen.path == path && seen.schema_name() == Some(schema))
            .count()
    };
    let (authors, plans) = {
        let mut log = log.lock().expect("the log lock");
        log.push(request.clone());
        arrived.notify_all();
        let counts = (count(&log, "scxml_draft"), count(&log, "topology_plan"));
        if path == "/slow/v1/chat/completions"
            && request.schema_name() == Some("topology_plan")
            && counts.1 == 1
        {
            // Hold the first plan until the builder asks for the redesign:
            // its timeout has preempted this generation by then.
            drop(
                arrived
                    .wait_timeout_while(log, SLOW_PLAN_HOLD_BOUND, |log| {
                        count(log, "topology_plan") < 2
                    })
                    .expect("the log lock"),
            );
        }
        counts
    };
    let reply = mock_reply(&path, request.schema_name(), authors, plans, &body);
    let (status, reply) = reply.map_or(("404 Not Found", "{}".to_owned()), |reply| {
        ("200 OK", reply)
    });
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
    *request.answered.lock().expect("the answered lock") = Some(Instant::now());
}

/// Writes an env file holding the fake keys and base URLs of `env` (the
/// builder's document names all four; a variable the file does not set
/// holds no value, so its slot is absent).
fn env_file(
    test: &str,
    mock: &Mock,
    generate_route: &str,
    decision_route: &str,
    env: &[&str],
) -> PathBuf {
    let origin = &mock.origin;
    let values = [
        ("CPA_API_KEY", FAKE_CPA_KEY.to_owned()),
        ("CPA_BASE_URL", format!("{origin}/{generate_route}/v1")),
        ("TYPESAFE_API_KEY", FAKE_TYPESAFE_KEY.to_owned()),
        ("TYPESAFE_BASE_URL", format!("{origin}/{decision_route}")),
    ];
    let mut contents = String::new();
    for (name, value) in values.iter().filter(|(name, _)| env.contains(name)) {
        contents.push_str(name);
        contents.push('=');
        contents.push_str(value);
        contents.push('\n');
    }
    let path = scratch("builder-env").join(format!("{test}.env"));
    std::fs::write(&path, contents).expect("the env file is writable");
    path
}

const ALL: [&str; 4] = support::GRANTED;

fn run(mock: &Mock, test: &str, route: &str, env: &[&str], request: &Json) -> Vec<Json> {
    run_deciding(mock, test, route, "typesafe", env, request)
}

/// [`run`] with `builder` as the text of the builder document, run from a
/// scratch file.
fn run_builder(mock: &Mock, test: &str, route: &str, builder: &str, request: &Json) -> Vec<Json> {
    let dir = scratch("builder-variant");
    let document = write(&dir, &format!("{test}.scx"), builder);
    let env = env_file(test, mock, route, "typesafe", &ALL);
    start(&document, "logical", &[env], request, &[])
        .finish(Duration::from_mins(5))
        .records
}

/// [`run`] with the decision backend on `decision_route`.
fn run_deciding(
    mock: &Mock,
    test: &str,
    route: &str,
    decision_route: &str,
    env: &[&str],
    request: &Json,
) -> Vec<Json> {
    let env = env_file(test, mock, route, decision_route, env);
    let finished =
        start(&builder_document(), "logical", &[env], request, &[]).finish(Duration::from_mins(5));
    assert!(
        !finished.records.is_empty(),
        "the session ran: {}",
        finished.stderr
    );
    finished.records
}

fn chat_requests<'a>(received: &'a [Received], schema: &str) -> Vec<&'a Received> {
    received
        .iter()
        .filter(|request| request.schema_name() == Some(schema))
        .collect()
}

// ---------------------------------------------------------------------------
// The document itself.

#[test]
fn builder_validates_clean_with_its_repository_packages() {
    let validated = validate(&builder_dir().join("builder.scx"), &[]);
    assert!(validated.status.success(), "{}", validated.stderr);
    assert!(
        validated
            .stderr
            .contains("diagnostic summary: 0 error(s), 0 warning(s), 0 note(s)"),
        "builder.scx validates without an error, a warning, or a note: {}",
        validated.stderr
    );
}

/// Every `event:schema` of builder.scx is an inline copy of
/// `schemas/<event>.json` (an event taken in two states carries the same
/// copy in both) or, on a final state's `<donedata>`, of
/// `schemas/<final state id>.json`, and every schema file is used.
#[test]
fn builder_inline_event_schemas_equal_the_schema_files() {
    let source = builder_source();
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "builder.scx",
        source.as_str(),
    ))
    .expect("builder.scx is XML");
    let mut inline = BTreeMap::new();
    document.root.visit(&mut |element| {
        let schema = element.attributes.iter().find(|attribute| {
            attribute.name.namespace_uri.as_deref() == Some(scxml_event_schema::EVENT_NAMESPACE_URI)
                && attribute.name.local_name == scxml_event_schema::SCHEMA_ATTRIBUTE
        });
        let event = element
            .attributes
            .iter()
            .find(|attribute| {
                attribute.name.namespace_uri.is_none() && attribute.name.local_name == "event"
            })
            .map(|attribute| attribute.value.clone());
        if let (Some(schema), Some(event)) = (schema, event) {
            let value: Json =
                serde_json::from_str(&schema.value).expect("every inline schema is JSON");
            if let Some(previous) = inline.insert(event.clone(), value.clone()) {
                assert_eq!(previous, value, "one schema per event: {event}");
            }
        }
        if element.name.local_name == "final"
            && let Some(id) = element.attribute("id")
        {
            for child in &element.children {
                let scxml_syntax::Node::Element(donedata) = child else {
                    continue;
                };
                if donedata.name.local_name != "donedata" {
                    continue;
                }
                let schema = donedata
                    .attributes
                    .iter()
                    .find(|attribute| {
                        attribute.name.namespace_uri.as_deref()
                            == Some(scxml_event_schema::EVENT_NAMESPACE_URI)
                            && attribute.name.local_name == scxml_event_schema::SCHEMA_ATTRIBUTE
                    })
                    .expect("each final state's donedata carries its schema");
                let value: Json =
                    serde_json::from_str(&schema.value).expect("every inline schema is JSON");
                inline.insert(id.value.clone(), value);
            }
        }
    });
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(builder_dir().join("schemas")).expect("schemas/ is listable") {
        let path = entry.expect("a directory entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
            .expect("every schema file is <event>.json")
            .to_owned();
        let value: Json = serde_json::from_slice(&std::fs::read(&path).expect("readable"))
            .expect("every schema file is JSON");
        files.insert(name, value);
    }
    assert_eq!(inline, files);
}

/// The bots live runs built (bots/builder/examples) still validate.
#[test]
fn the_saved_live_examples_validate() {
    for example in ["traffic_light", "traffic_light_luna"] {
        let validated = validate(
            &builder_dir().join(format!("examples/{example}/traffic_light.scx")),
            &[],
        );
        assert!(
            validated.status.success(),
            "{example}: {}",
            validated.stderr
        );
        assert!(
            validated.stderr.contains("0 error(s), 0 warning(s)"),
            "{example}: {}",
            validated.stderr
        );
    }
}

// ---------------------------------------------------------------------------
// The loop.

#[test]
fn the_builder_repairs_an_invalid_draft_fixes_it_simulates_it_and_reports_done() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "repair",
        "repair",
        &ALL,
        &json!({"spec": SPEC, "constraints": ["send every light change to the invoker"]}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");

    // The draft that came back validates on its own, and it is the
    // machine-fixed text of the second draft.
    let scxml = done["scxml"].as_str().expect("scxml is text");
    assert!(
        scxml.contains(r#"event="pedestrian.button light.timer""#),
        "{scxml}"
    );
    assert!(
        compiles("traffic_light.scx", scxml),
        "the built machine validates and compiles"
    );
    assert_eq!(
        done["schemas"],
        json!([{"event": "pedestrian.button", "schema": {"type": "object", "additionalProperties": false}}])
    );
    assert_eq!(done["tests"], tests());

    let report = &done["report"];
    assert_eq!(report["attempts"], 2, "{report}");
    assert_eq!(report["failures"], 1, "{report}");
    assert_eq!(report["fixes_applied"], 1, "{report}");
    assert_eq!(report["decisions"], json!(["triage.repair"]), "{report}");
    assert_eq!(report["plan"]["name"], "traffic_light");
    assert_eq!(report["history"][0]["diagnostics"][0]["code"], "SCXML005");
    let simulation = report["simulation"].as_array().expect("simulation results");
    assert_eq!(simulation.len(), 2, "{report}");
    assert!(
        simulation.iter().all(|result| result["passed"] == true),
        "{report}"
    );
    assert_eq!(
        simulation[0]["observed"],
        json!(["light.green", "light.yellow", "light.red", "walk.on"]),
        "the draft ran as an invoked child, driven through #_<invokeid>: {report}"
    );
    assert_eq!(
        simulation[1]["observed"],
        json!(["light.green"]),
        "{report}"
    );
    let first_id = simulation[0]["invokeid"].as_str().expect("an invoke id");
    let second_id = simulation[1]["invokeid"].as_str().expect("an invoke id");
    assert!(
        !first_id.is_empty() && first_id != second_id,
        "each test runs its own invocation, so nothing an earlier one queued counts: {report}"
    );

    assert_repair_requests(&mock.received());
}

/// The repair run's requests: one plan, two drafts (the second with the
/// first one's diagnostics, draft, and tests), one decision.
fn assert_repair_requests(received: &[Received]) {
    let designs = chat_requests(received, "topology_plan");
    let authors = chat_requests(received, "scxml_draft");
    assert_eq!(designs.len(), 1);
    assert_eq!(authors.len(), 2);
    assert_eq!(designs[0].user_input()["spec"], SPEC);
    assert_eq!(designs[0].user_input()["previous_plan"], Json::Null);
    let first = authors[0].user_input();
    assert_eq!(first["task"], "author");
    assert_eq!(first["plan"]["name"], "traffic_light");
    let second = authors[1].user_input();
    assert_eq!(second["task"], "repair");
    assert_eq!(second["diagnostics"][0]["code"], "SCXML005");
    assert!(
        second["previous_scxml"]
            .as_str()
            .is_some_and(|text| text.contains(r#"target="amber""#)),
        "the repair sees the failed draft"
    );
    assert_eq!(
        second["previous_tests"],
        tests(),
        "the repair sees the failed draft's tests"
    );
    assert_eq!(first["previous_tests"], Json::Null);
    for request in designs.iter().chain(&authors) {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {FAKE_CPA_KEY}").as_str())
        );
    }
    let decisions: Vec<&Received> = received
        .iter()
        .filter(|request| request.path == "/typesafe/v1/systemone")
        .collect();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].authorization.as_deref(),
        Some(format!("Bearer {FAKE_TYPESAFE_KEY}").as_str())
    );
    let offered: Vec<&String> = decisions[0]
        .body
        .pointer("/questions/selection/criteria")
        .and_then(Json::as_object)
        .expect("the selection question")
        .keys()
        .collect();
    for event in ["triage.repair", "triage.redesign", "triage.abandon"] {
        assert!(
            offered.iter().any(|label| *label == event),
            "{event} in {offered:?}"
        );
    }
}

/// A `builder.timeout` fires while the design generation is in flight (the
/// slow route holds its answer): it preempts the call, leaving design
/// cancels the invocation, and the late answer reaches no one. The builder
/// redesigns and finishes with the second plan. Runs on the wall clock.
#[test]
fn a_builder_timeout_preempts_a_generation_in_flight_and_cancels_it() {
    let mock = Mock::start();
    let env = env_file("timeout", &mock, "slow", "redesign", &ALL);
    let mut args = builder_args(&builder_document(), "wall");
    args.extend(["--env-file".to_owned(), env.display().to_string()]);
    let mut run = Run::start(&args, &[]);
    let mut records = run.until(Duration::from_mins(2), |record| {
        record["type"] == "call" && record["cause"] == "start"
    });
    run.event(
        "builder.request",
        &json!({"spec": SPEC, "generate_timeout_ms": 1000}),
    );
    run.end_input();
    records.extend(run.finish(Duration::from_mins(3)).records);
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(
        report["history"][0]["diagnostics"][0]["code"], "design-timeout",
        "{report}"
    );
    assert_eq!(report["history"][0]["diagnostics"][0]["kind"], "timeout");
    // No plan yet, so the decision service is offered no repair: it
    // redesigns.
    assert_eq!(report["decisions"], json!(["triage.redesign"]), "{report}");
    assert_eq!(
        report["plan"]["name"], "traffic_light",
        "the cancelled generation's late plan never arrives: {report}"
    );
    let received = mock.received();
    let designs = chat_requests(&received, "topology_plan");
    assert_eq!(designs.len(), 2, "the timed-out design, then the redesign");
    let late = designs[0]
        .answered
        .lock()
        .expect("the answered lock")
        .expect("the held plan was answered");
    // The first plan is held until the redesign request arrives, so the
    // redesign was asked for while that generation was still in flight:
    // the timeout preempted it.
    assert!(
        designs[1].at <= late,
        "the builder moved on while the first generation was still in flight"
    );
}

/// Without the decision key the builder falls back to its cond-based
/// triage; a repair that fails with the same diagnostic as the draft before
/// it is no progress, so the builder ends in `failed` (`no-progress`) with
/// the repeated diagnostics, without a cap from the request.
#[test]
fn the_same_diagnostic_twice_ends_the_build_with_no_progress() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "broken",
        "broken",
        &["CPA_API_KEY", "CPA_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "no-progress");
    assert_eq!(failed["diagnostics"][0]["kind"], "validate");
    assert_eq!(failed["diagnostics"][0]["code"], "SCXML005");
    let received = mock.received();
    assert_eq!(chat_requests(&received, "topology_plan").len(), 1);
    assert_eq!(chat_requests(&received, "scxml_draft").len(), 2);
    assert!(
        received
            .iter()
            .all(|request| request.path != "/typesafe/v1/systemone"),
        "no decision request without a granted key"
    );
}

/// A failure that differs from the one before it is progress: the builder
/// keeps repairing (an invalid draft, then one that breaks the draft
/// profile) until the fixed draft ends in `done`.
#[test]
fn a_different_diagnostic_each_time_keeps_repairing_until_done() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "progress",
        "progress",
        &["CPA_API_KEY", "CPA_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(report["attempts"], 3, "{report}");
    assert_eq!(report["failures"], 2, "{report}");
    assert_eq!(report["history"][0]["diagnostics"][0]["code"], "SCXML005");
    assert!(
        report["history"][1]["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == "SCXML098")),
        "{report}"
    );
}

/// A redesign sends the plan it replaces as `previous_plan`.
#[test]
fn a_redesign_sends_the_previous_plan() {
    let mock = Mock::start();
    let records = run_deciding(
        &mock,
        "redesign",
        "broken",
        "redesign",
        &ALL,
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    let received = mock.received();
    let designs = chat_requests(&received, "topology_plan");
    assert_eq!(designs.len(), 2, "one design, then the redesign");
    assert_eq!(designs[0].user_input()["previous_plan"], Json::Null);
    let expected: Json = serde_json::from_str(PLAN).expect("the plan is JSON");
    let second = designs[1].user_input();
    assert_eq!(
        second["previous_plan"], expected,
        "the redesign sees the plan it replaces"
    );
    assert_eq!(second["diagnostics"][0]["code"], "SCXML005");
}

/// A redesign whose generation failed keeps the plan it replaced: the next
/// redesign still sends it as `previous_plan`.
#[test]
fn a_failed_redesign_keeps_the_previous_plan() {
    let mock = Mock::start();
    let records = run_deciding(
        &mock,
        "flaky",
        "flaky",
        "redesign",
        &ALL,
        &json!({"spec": SPEC, "max_attempts": 3}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    let received = mock.received();
    let designs = chat_requests(&received, "topology_plan");
    assert_eq!(designs.len(), 3, "a design, a failed redesign, a redesign");
    let expected: Json = serde_json::from_str(PLAN).expect("the plan is JSON");
    assert_eq!(designs[1].user_input()["previous_plan"], expected);
    assert_eq!(
        designs[2].user_input()["previous_plan"],
        expected,
        "the failed redesign did not lose the plan it replaces"
    );
    assert_eq!(
        designs[2].user_input()["diagnostics"][0]["code"],
        "design-failed"
    );
}

/// A design generation that fails at once (the backend answers `404`), and
/// a later failure that ends the build: the design failure's cause (the
/// generate service's code, `status`) is in its `retry_gate` log and in the
/// `failed` done data's history, though the diagnostics that ended the
/// build are the later ones; the final `failed` log names the reason and
/// the cause that ended the build.
#[test]
fn a_fast_design_failure_keeps_its_cause_in_the_log_and_the_failed_done_data() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "fastfail",
        "fastfail",
        &["CPA_API_KEY", "CPA_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "no-progress");
    assert_eq!(failed["diagnostics"][0]["code"], "SCXML005");
    let logs: Vec<String> = records
        .iter()
        .filter(|record| record.get("parent").is_none() && record["type"] == "log")
        .map(|record| format!("{} {record}", record["label"]))
        .collect();
    assert!(
        logs.iter().any(|log| log.starts_with("\"retry_gate\"")
            && log.contains("design-failed")
            && log.contains("(status)")),
        "the retry_gate log carries the design failure's cause: {logs:?}"
    );
    assert!(
        logs.iter().any(|log| log.starts_with("\"failed\"")
            && log.contains("no-progress")
            && log.contains("SCXML005")),
        "the failed log carries the reason and the cause that ended the build: {logs:?}"
    );
    let history = failed["history"]
        .as_array()
        .expect("the failed done data has a history");
    assert_eq!(history.len(), 3, "{failed}");
    assert_eq!(history[0]["diagnostics"][0]["code"], "design-failed");
    assert!(
        history[0]["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("(status)")),
        "{failed}"
    );
}

/// Without a plan, `decision:menu` leaves out `triage.repair` (its guard is
/// false), so the decision service is never offered it and `builder.done`
/// never reports a null plan.
#[test]
fn repair_is_not_offered_without_a_plan() {
    let mock = Mock::start();
    let records = run_deciding(
        &mock,
        "noplan",
        "noplan",
        "redesign",
        &ALL,
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "no-progress");
    assert_eq!(failed["diagnostics"][0]["code"], "design-failed");
    let received = mock.received();
    assert_eq!(chat_requests(&received, "topology_plan").len(), 2);
    assert!(chat_requests(&received, "scxml_draft").is_empty());
    let decisions: Vec<&Received> = received
        .iter()
        .filter(|request| request.path == "/redesign/v1/systemone")
        .collect();
    assert_eq!(decisions.len(), 1);
    let offered: Vec<&String> = decisions[0]
        .body
        .pointer("/questions/selection/criteria")
        .and_then(Json::as_object)
        .expect("the selection question")
        .keys()
        .collect();
    assert!(
        !offered.iter().any(|label| *label == "triage.repair"),
        "{offered:?}"
    );
    assert!(
        offered.iter().any(|label| *label == "triage.redesign"),
        "{offered:?}"
    );
}

/// A draft that invokes, scripts, loads, or declares another namespace is
/// invalid under the validator's draft profile, so it never runs as a child
/// with the run's packages and env values.
#[test]
fn a_draft_that_breaks_the_draft_profile_never_runs() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "smuggle",
        "smuggle",
        &["CPA_API_KEY", "CPA_BASE_URL"],
        &json!({"spec": SPEC, "max_attempts": 1}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted");
    assert!(
        failed["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| diagnostics.iter().any(|diagnostic| {
                diagnostic["kind"] == "validate" && diagnostic["code"] == "SCXML098"
            })),
        "the invoke is the draft profile's SCXML098: {failed}"
    );
}

/// The draft profile reads namespaces by expanded name, whatever the prefix,
/// and refuses a document type declaration, so neither hides a namespace or
/// an element.
#[test]
fn a_non_ascii_prefix_or_an_entity_does_not_get_past_the_draft_profile() {
    for (route, code) in [("unicode", "SCXML097"), ("doctype", "SCXML101")] {
        let mock = Mock::start();
        let records = run(
            &mock,
            route,
            route,
            &ALL,
            &json!({"spec": SPEC, "max_attempts": 1}),
        );
        assert!(finished(&records), "{route}: {records:?}");
        let (label, failed) = answer(&records);
        assert_eq!(label, "failed", "{route}: {failed}");
        assert!(
            failed["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == code)),
            "{route}: {failed}"
        );
        let received = mock.received();
        assert_eq!(
            chat_requests(&received, "scxml_draft").len(),
            1,
            "{route}: the draft never ran, so it generated nothing"
        );
        assert!(
            received
                .iter()
                .all(|request| request.authorization.as_deref()
                    != Some(&format!("Bearer {FAKE_TYPESAFE_KEY}"))
                    || request.path.ends_with("/systemone")),
            "{route}: the decision key reached no other backend"
        );
    }
}

/// A candidate that sends `sim.settle` or `sim.drive` to its parent has them
/// recorded as outputs; only the builder's own (invoke-id-less) control
/// events end or drive a test.
#[test]
fn a_candidates_control_event_names_are_recorded_as_outputs() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "impostor",
        "impostor",
        &ALL,
        &json!({"spec": SPEC, "max_attempts": 1}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let simulation = done
        .pointer("/report/simulation/0/observed")
        .cloned()
        .unwrap_or(Json::Null);
    assert_eq!(
        simulation,
        json!(["sim.settle", "sim.drive", "light.green"]),
        "{done}"
    );
}

/// A `builder.timeout` a cancelled candidate sent to `#_parent` does not
/// end the simulation as a timeout: the first draft's tests fail on its
/// `error.stale` (each test, not the whole simulation), and the repaired
/// draft passes.
#[test]
fn a_cancelled_candidates_builder_timeout_does_not_end_the_simulation() {
    let mock = Mock::start();
    let records = run(&mock, "stale", "stale", &ALL, &json!({"spec": SPEC}));
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(report["failures"], 1, "{report}");
    let diagnostics = report["history"][0]["diagnostics"]
        .as_array()
        .expect("the failure's diagnostics");
    assert_eq!(diagnostics.len(), 2, "both tests ran: {report}");
    assert!(
        diagnostics.iter().all(|d| d["code"] == "test-failed"
            && d["message"]
                .as_str()
                .is_some_and(|m| m.contains("error error.stale"))),
        "{report}"
    );
}

/// The `sim.drive` of a test whose candidate fails to start is not taken by
/// the next test: that test's `ping` is sent once. The builder here is
/// `builder.scx` with its first test's invocation content replaced by a
/// text that is no SCXML document (a validated draft gives no way to make
/// the child's start fail).
#[test]
fn a_failed_invocations_sim_drive_does_not_drive_the_next_test() {
    let source = builder_source();
    let failing = source.replacen(
        r#"<content expr="scxml_text"/>"#,
        r#"<content expr="sim_index === 0 ? 'not a document' : scxml_text"/>"#,
        1,
    );
    assert_ne!(failing, source, "the builder invokes scxml_text");
    let mock = Mock::start();
    let records = run_builder(
        &mock,
        "pings",
        "pings",
        &failing,
        &json!({"spec": SPEC, "max_attempts": 1}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    let diagnostics = failed["diagnostics"]
        .as_array()
        .expect("the failure's diagnostics");
    assert_eq!(
        diagnostics.len(),
        1,
        "only the test whose invocation failed fails: {failed}"
    );
    let message = diagnostics[0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("test first run") && message.contains("error error.execution"),
        "{failed}"
    );
}

#[test]
fn an_invalid_request_is_answered_with_builder_failed() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "invalid",
        "repair",
        &ALL,
        &json!({"spec": "", "max_attempts": 99}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed");
    assert_eq!(failed["reason"], "invalid-request");
    assert_eq!(failed["diagnostics"][0]["kind"], "request");
    assert!(mock.received().is_empty(), "nothing is generated");
}

// ---------------------------------------------------------------------------
// Live.

/// Builds the spec with the real `generate` (and `decision`) backends on the
/// wall clock (`FLOPPY_LIVE_ENV_FILE`; `FLOPPY_LIVE_MODEL`, default
/// `sparks/qwen3.8-flash-next`). Writes the produced bot under `target/builder-live/` and
/// prints only pass or fail and counts.
#[test]
#[ignore = "makes real model calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_builder_through_an_env_file() {
    let env_files = support::live_env_files();
    let model =
        std::env::var("FLOPPY_LIVE_MODEL").unwrap_or_else(|_| "sparks/qwen3.8-flash-next".into());
    let finished = start(
        &builder_document(),
        "wall",
        &env_files,
        &json!({
            "spec": SPEC,
            "constraints": [
                "send every light change and every walk signal to the invoker",
                "a button press during green ends green early",
            ],
            "model": model,
        }),
        // No base URL from the shell: each service uses its package's
        // declared origin unless the env file sets one (decision D18).
        &[],
    )
    .finish(Duration::from_mins(30));
    let out = workspace().join("target/builder-live");
    std::fs::create_dir_all(&out).expect("the output directory is creatable");
    let (label, data) = answer(&finished.records);
    let pass = if label == "done" {
        {
            let scxml = data["scxml"].as_str().unwrap_or_default().to_owned();
            let valid = compiles("traffic_light.scx", &scxml);
            std::fs::write(out.join("traffic_light.scx"), &scxml).expect("writable");
            for member in ["schemas", "tests", "report"] {
                std::fs::write(
                    out.join(format!("{member}.json")),
                    serde_json::to_string_pretty(&data[member]).expect("JSON"),
                )
                .expect("writable");
            }
            println!(
                "live builder: attempts {}, failures {}, fixes {}, decisions {}",
                data["report"]["attempts"],
                data["report"]["failures"],
                data["report"]["fixes_applied"],
                data["report"]["decisions"]
            );
            valid
        }
    } else {
        std::fs::write(
            out.join("failed.json"),
            serde_json::to_string_pretty(&data).expect("JSON"),
        )
        .expect("writable");
        false
    };
    println!("live builder: {}", if pass { "pass" } else { "fail" });
    assert!(pass, "the live build did not pass");
}
