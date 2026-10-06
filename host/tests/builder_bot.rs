//! nemobot-host harness of the builder bot (`bots/builder/builder.scxml`): a
//! parent machine invokes the builder as a child (`<invoke type="scxml"
//! src="builder.scxml">`), forwards it a `builder.request`, and finishes on
//! `builder.done` or `builder.failed`. The live session binds the real
//! `scxml`, `generate`, and `decision` extension components; `generate` and
//! `decision` talk to one deterministic fake backend on 127.0.0.1 (Chat
//! Completions and `TypeSafe` `system_one`; nothing reaches the network) whose
//! canned drafts include an invalid one, so the repair loop, the
//! machine-applicable fixes, the decision, and the simulation as an invoked
//! child all run.
//!
//! `scripts/build-components.sh` builds and verifies the release session
//! components and the namespace components into
//! `target/test-builder-bot/components`.
//!
//! The ignored `live_builder_through_an_env_file` test builds the same spec
//! with the real backends: `NEMOBOT_LIVE_ENV_FILE=<path> cargo test -p
//! nemobot-host --test builder_bot -- --ignored`. It grants `OPENAI_API_KEY`
//! (required) and, when the file sets it, `TYPESAFE_API_KEY`; the base URLs
//! are the file's values, else the config's non-secret `env-defaults` (no
//! backend URL is compiled into a component);
//! writes the produced bot under `target/builder-live/`, and prints pass or
//! fail and counts only; no value is ever printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nemobot_host::FailureKind;
use nemobot_host::compile::compile_source;
use nemobot_host::config::{CONFIG_FORMAT, Config};
use nemobot_host::env_file::Values;
use nemobot_host::packages::Packages;
use nemobot_host::runtime::{ClockMode, Egress, SessionHandle, SessionSpec, SessionStatus};
use scxml_host_wasmtime::{ScriptedEvent, SessionOutput};
use serde_json::{Map, Value as Json, json};
use sha2::{Digest as _, Sha256};

/// The fake keys (not secrets): the mock checks they arrive as bearers.
const FAKE_OPENAI_KEY: &str = "sk-nemobot-fake";
const FAKE_TYPESAFE_KEY: &str = "ts-nemobot-fake";
/// The extension call fuel `<sx:validate>` needs (namespaces/scxml).
const EXTENSION_CALL_FUEL: u64 = 1_000_000_000;
const SPEC: &str = "a traffic light with a pedestrian button";

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the host crate sits in the nemobot workspace")
}

fn builder_dir() -> PathBuf {
    workspace().join("bots/builder")
}

fn builder_source() -> String {
    std::fs::read_to_string(builder_dir().join("builder.scxml")).expect("builder.scxml is readable")
}

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("the artifact is readable");
    format!("{:x}", Sha256::digest(&bytes))
}

/// The verified session components and the three namespace components.
struct Artifacts {
    components: PathBuf,
    manifest_sha256: String,
    namespaces: PathBuf,
}

fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let dir = workspace().join("target/test-builder-bot/components");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/build-components.sh"))
            .arg("--artifacts-dir")
            .arg(&dir)
            .current_dir(workspace())
            .status()
            .expect("the build script runs");
        assert!(status.success(), "the component proof must pass");
        let components = dir.join("scxml");
        Artifacts {
            manifest_sha256: sha256_file(&components.join("manifest.tsv")),
            components,
            namespaces: dir.join("namespaces"),
        }
    })
}

/// A config binding the `scxml`, `generate`, and `decision` packages,
/// granting `env` from `env_file`.
fn config(env_file: &Path, env: &[&str]) -> Config {
    config_with_defaults(env_file, env, &json!({}))
}

/// [`config`] with non-secret `env-defaults`.
fn config_with_defaults(env_file: &Path, env: &[&str], defaults: &Json) -> Config {
    let artifacts = artifacts();
    let extension = |name: &str| {
        let component = artifacts
            .namespaces
            .join(format!("nemobot_{name}.virtualized.wasm"));
        json!({
            "manifest": workspace()
                .join(format!("namespaces/{name}/package/{name}.extension.json"))
                .display()
                .to_string(),
            "component": component.display().to_string(),
            "component-sha256": sha256_file(&component),
        })
    };
    let text = json!({
        "format": CONFIG_FORMAT,
        "components": {
            "dir": artifacts.components.display().to_string(),
            "manifest-sha256": artifacts.manifest_sha256,
        },
        "env-file": env_file.display().to_string(),
        "env": env,
        "env-defaults": defaults,
        "extensions": [extension("scxml"), extension("generate"), extension("decision")],
        "outbound-http": true,
        "extension-call-fuel": EXTENSION_CALL_FUEL,
    })
    .to_string();
    Config::parse(text.as_bytes(), &PathBuf::from("/")).expect("the test config is valid")
}

/// The parent: invokes the builder, forwards the `build` event's data as
/// `builder.request`, and logs the answer.
const PARENT: &str = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"
       datamodel="ecmascript" initial="running">
  <state id="running">
    <invoke id="builder" type="scxml" src="builder.scxml"/>
    <transition event="build">
      <send event="builder.request" target="#_builder"><content expr="_event.data"/></send>
    </transition>
    <transition event="builder.done" target="built">
      <log label="builder.done" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="builder.failed" target="failed">
      <log label="builder.failed" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="error.*" target="failed">
      <log label="error" expr="_event.name"/>
    </transition>
  </state>
  <final id="built"/>
  <final id="failed"/>
</scxml>"##;

fn open(config: &Config, clock: ClockMode) -> SessionHandle {
    let packages = Packages::load(config).expect("the three packages bind");
    let values = Values::granted(
        config
            .env_file
            .as_deref()
            .expect("the config names an env file"),
        &config.env,
        &config.env_defaults,
        &|_| None,
    )
    .expect("the env file grants the variables");
    let compiled = compile_source(
        "parent.scxml",
        PARENT.to_owned(),
        scxml_event_schema::Resources::new(),
        &packages,
    )
    .expect("the parent compiles");
    SessionHandle::open(
        config,
        packages,
        values,
        SessionSpec {
            compiled,
            session_id: "builder-harness".into(),
            clock,
            documents: vec![("builder.scxml".into(), builder_source())],
        },
    )
    .expect("the session opens")
}

fn build_event(request: &Json) -> ScriptedEvent {
    ScriptedEvent {
        name: "build".into(),
        data: scxml_types::DataValue::JsonObject(request.to_string()),
        trace_context: None,
    }
}

/// Egress records until the session closes, at most `limit`.
fn until_closed(handle: &SessionHandle, limit: Duration) -> Vec<Egress> {
    let deadline = Instant::now() + limit;
    let mut records = Vec::new();
    while Instant::now() < deadline {
        if let Some(record) = handle.recv_timeout(Duration::from_millis(100)) {
            let closed = matches!(record, Egress::Closed { .. });
            records.push(record);
            if closed {
                return records;
            }
        }
    }
    panic!(
        "the session did not close in time; got {} records",
        records.len()
    );
}

fn logs(records: &[Egress]) -> Vec<(String, String)> {
    records
        .iter()
        .filter_map(|record| match record {
            Egress::Outputs(outputs) => Some(outputs),
            _ => None,
        })
        .flatten()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => {
                Some((entry.label.clone().unwrap_or_default(), entry.text.clone()))
            }
            SessionOutput::Done { .. } => None,
        })
        .collect()
}

fn finished(records: &[Egress]) -> bool {
    matches!(
        records.last(),
        Some(Egress::Closed {
            status: SessionStatus::Finished,
            ..
        })
    )
}

/// The one answer the parent logged: its label and its JSON data.
fn answer(records: &[Egress]) -> (String, Json) {
    let logs = logs(records);
    let [(label, text)] = logs.as_slice() else {
        panic!("the parent logs exactly one answer: {logs:?}");
    };
    (
        label.clone(),
        serde_json::from_str(text).unwrap_or_else(|_| Json::String(text.clone())),
    )
}

// ---------------------------------------------------------------------------
// Canned backend answers.

const PLAN: &str = r#"{"name":"traffic_light","summary":"A vehicle light that cycles green, yellow, red, and serves a pedestrian button by ending green early.","states":[{"id":"green","kind":"atomic","parent":"","purpose":"Vehicles go."},{"id":"yellow","kind":"atomic","parent":"","purpose":"Vehicles prepare to stop."},{"id":"red","kind":"atomic","parent":"","purpose":"Vehicles stop; pedestrians walk."}],"events":[{"name":"pedestrian.button","direction":"input","description":"A pedestrian pressed the button."},{"name":"light.green","direction":"output","description":"The light turned green."},{"name":"light.yellow","direction":"output","description":"The light turned yellow."},{"name":"light.red","direction":"output","description":"The light turned red."},{"name":"walk.on","direction":"output","description":"Pedestrians may cross."},{"name":"light.timer","direction":"internal","description":"The phase timer expired."}],"transitions":[{"source":"green","event":"pedestrian.button","target":"yellow","description":"End green early."},{"source":"green","event":"light.timer","target":"yellow","description":"Green expired."},{"source":"yellow","event":"light.timer","target":"red","description":"Yellow expired."},{"source":"red","event":"light.timer","target":"green","description":"Red expired."}]}"#;

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

/// A draft: the document, the button's schema, and one test script.
fn draft(scxml: &str) -> String {
    json!({
        "scxml": scxml,
        "schemas": [{"event": "pedestrian.button", "schema": BUTTON_SCHEMA}],
        "tests": [{
            "name": "button ends green early",
            "events": [{"name": "pedestrian.button", "data": "{}", "delay_ms": 1000}],
            "settle_ms": 6000,
            "expect_events": ["light.green", "light.yellow", "light.red", "walk.on"],
            "expect_done": false,
        }],
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

fn completion(content: &str) -> String {
    json!({
        "choices": [{"index": 0, "finish_reason": "stop",
                     "message": {"role": "assistant", "content": content}}],
    })
    .to_string()
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

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    path: String,
    authorization: Option<String>,
    body: Json,
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
/// one, then valid drafts; `/broken/v1` answers the plan, then only invalid
/// drafts. `decision` route: `/typesafe` chooses `triage.repair`.
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
        let received = Arc::new(Mutex::new(Vec::<Received>::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
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
                    continue;
                }
                let path = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                let parsed: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
                let request = Received {
                    path: path.clone(),
                    authorization: headers.get("authorization").cloned(),
                    body: parsed,
                };
                let authors = {
                    let mut log = log.lock().expect("the log lock");
                    log.push(request.clone());
                    log.iter()
                        .filter(|seen| {
                            seen.path == path && seen.schema_name() == Some("scxml_draft")
                        })
                        .count()
                };
                let reply = match (path.as_str(), request.schema_name()) {
                    (
                        "/repair/v1/chat/completions" | "/broken/v1/chat/completions",
                        Some("topology_plan"),
                    ) => Some(completion(PLAN)),
                    ("/repair/v1/chat/completions", Some("scxml_draft")) => {
                        Some(completion(&match authors {
                            1 => invalid_draft(),
                            2 => fixable_draft(),
                            _ => draft(&traffic_light(&green_separate(), "red")),
                        }))
                    }
                    ("/broken/v1/chat/completions", Some("scxml_draft")) => {
                        Some(completion(&invalid_draft()))
                    }
                    ("/typesafe/v1/systemone", _) => system_one("triage.repair", &body),
                    _ => None,
                };
                let (status, reply) = reply.map_or(("404 Not Found", "{}".to_owned()), |reply| {
                    ("200 OK", reply)
                });
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self { origin, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().expect("the log lock").clone()
    }
}

/// Writes an env file holding both fake keys and both base URLs.
fn env_file(test: &str, mock: &Mock, generate_route: &str) -> PathBuf {
    let dir = workspace().join("target/test-builder-bot/env");
    std::fs::create_dir_all(&dir).expect("the env directory is creatable");
    let path = dir.join(format!("{test}.env"));
    std::fs::write(
        &path,
        format!(
            "OPENAI_API_KEY={FAKE_OPENAI_KEY}\nOPENAI_BASE_URL={origin}/{generate_route}/v1\nTYPESAFE_API_KEY={FAKE_TYPESAFE_KEY}\nTYPESAFE_BASE_URL={origin}/typesafe\n",
            origin = mock.origin
        ),
    )
    .expect("the env file is writable");
    path
}

const ALL: [&str; 4] = [
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "TYPESAFE_API_KEY",
    "TYPESAFE_BASE_URL",
];

fn run(mock: &Mock, test: &str, route: &str, env: &[&str], request: &Json) -> Vec<Egress> {
    let config = config(&env_file(test, mock, route), env);
    let handle = open(&config, ClockMode::Logical);
    handle
        .send(build_event(request))
        .expect("the session accepts the build event");
    until_closed(&handle, Duration::from_mins(5))
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
fn builder_scxml_validates_clean_with_its_packages() {
    let config = config(&workspace().join("target/test-builder-bot/none.env"), &[]);
    let packages = Packages::load(&config).expect("the three packages bind");
    let compiled = compile_source(
        "builder.scxml",
        builder_source(),
        scxml_event_schema::Resources::new(),
        &packages,
    )
    .expect("builder.scxml validates and compiles");
    // nemobot-host links no expression syntax checker, so the only finding
    // it may report is that note (SCXML045); `scxml-cli validate`, which
    // links one, reports nothing at all for builder.scxml.
    assert!(
        compiled
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "SCXML045"),
        "builder.scxml validates without an error, a warning, or another note: {:?}",
        compiled.diagnostics
    );
}

/// Every `event:schema` of builder.scxml is an inline copy of
/// `schemas/<event>.json`, and every schema file is used.
#[test]
fn builder_inline_event_schemas_equal_the_schema_files() {
    let source = builder_source();
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "builder.scxml",
        source.as_str(),
    ))
    .expect("builder.scxml is XML");
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
            assert!(
                inline.insert(event.clone(), value).is_none(),
                "one schema per event: {event}"
            );
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

/// The bot a live run built (bots/builder/examples) still validates.
#[test]
fn the_saved_live_example_validates() {
    let path = builder_dir().join("examples/traffic_light/traffic_light.scxml");
    let compiled = compile_source(
        "traffic_light.scxml",
        std::fs::read_to_string(&path).expect("the example is readable"),
        scxml_event_schema::Resources::new(),
        &Packages::default(),
    )
    .expect("the example validates and compiles");
    assert!(
        compiled
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "SCXML045"),
        "{:?}",
        compiled.diagnostics
    );
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
    assert_eq!(label, "builder.done", "{done}");

    // The draft that came back validates on its own, and it is the
    // machine-fixed text of the second draft.
    let scxml = done["scxml"].as_str().expect("scxml is text");
    assert!(
        scxml.contains(r#"event="pedestrian.button light.timer""#),
        "{scxml}"
    );
    compile_source(
        "traffic_light.scxml",
        scxml.to_owned(),
        scxml_event_schema::Resources::new(),
        &Packages::default(),
    )
    .expect("the built machine validates and compiles");
    assert_eq!(
        done["schemas"],
        json!([{"event": "pedestrian.button", "schema": {"type": "object", "additionalProperties": false}}])
    );
    assert_eq!(done["tests"][0]["name"], "button ends green early");

    let report = &done["report"];
    assert_eq!(report["attempts"], 2, "{report}");
    assert_eq!(report["failures"], 1, "{report}");
    assert_eq!(report["fixes_applied"], 1, "{report}");
    assert_eq!(report["decisions"], json!(["triage.repair"]), "{report}");
    assert_eq!(report["plan"]["name"], "traffic_light");
    assert_eq!(report["history"][0]["diagnostics"][0]["code"], "SCXML005");
    let simulation = report["simulation"].as_array().expect("simulation results");
    assert_eq!(simulation.len(), 1, "{report}");
    assert_eq!(simulation[0]["passed"], true, "{report}");
    assert_eq!(
        simulation[0]["observed"],
        json!(["light.green", "light.yellow", "light.red", "walk.on"]),
        "the draft ran as an invoked child, driven through #_candidate: {report}"
    );

    // One plan, two drafts (the second with the first one's diagnostics),
    // one decision.
    let received = mock.received();
    let designs = chat_requests(&received, "topology_plan");
    let authors = chat_requests(&received, "scxml_draft");
    assert_eq!(designs.len(), 1);
    assert_eq!(authors.len(), 2);
    assert_eq!(designs[0].user_input()["spec"], SPEC);
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
    for request in designs.iter().chain(&authors) {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {FAKE_OPENAI_KEY}").as_str())
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

/// Without the decision key the builder falls back to its cond-based
/// triage; drafts that never validate exhaust `max_attempts` and the builder
/// reports `builder.failed` with the last diagnostics.
#[test]
fn the_builder_gives_up_after_max_attempts_with_the_fallback_triage() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "broken",
        "broken",
        &["OPENAI_API_KEY", "OPENAI_BASE_URL"],
        &json!({"spec": SPEC, "max_attempts": 2}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "builder.failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted");
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
    assert_eq!(label, "builder.failed");
    assert_eq!(failed["reason"], "invalid-request");
    assert_eq!(failed["diagnostics"][0]["kind"], "request");
    assert!(mock.received().is_empty(), "nothing is generated");
}

// ---------------------------------------------------------------------------
// Live.

/// Adds `variable` to `granted` when the file sets it; the failure names the
/// kind only, never a value.
fn grant_if_set(path: &Path, granted: &mut Vec<&'static str>, variable: &'static str) {
    let mut probe: Vec<String> = granted.iter().map(|name| (*name).to_owned()).collect();
    probe.push(variable.to_owned());
    match Values::granted(path, &probe, &Values::none(), &|_| None) {
        Ok(_) => granted.push(variable),
        Err(failure)
            if failure.kind() == FailureKind::EnvFile
                && failure.message().contains(&format!("`{variable}`")) => {}
        Err(failure) => panic!("the env file is unusable ({:?})", failure.kind()),
    }
}

/// `OpenAI`'s and `TypeSafe`'s base URLs, the live run's config defaults (never
/// compiled into a component).
const OPENAI_BASE_URL_DEFAULT: &str = "https://api.openai.com/v1";
const TYPESAFE_BASE_URL_DEFAULT: &str = "https://api.typesafe.ai";

/// Builds the spec with the real `generate` (and `decision`) backends on the
/// wall clock (`NEMOBOT_LIVE_ENV_FILE`; `NEMOBOT_LIVE_MODEL`, default
/// `gpt-4o-mini`). Writes the produced bot under `target/builder-live/` and
/// prints only pass or fail and counts.
#[test]
#[ignore = "makes real model calls; needs NEMOBOT_LIVE_ENV_FILE"]
fn live_builder_through_an_env_file() {
    let Some(path) = std::env::var_os("NEMOBOT_LIVE_ENV_FILE").map(PathBuf::from) else {
        panic!("set NEMOBOT_LIVE_ENV_FILE to an env file that sets OPENAI_API_KEY");
    };
    let model = std::env::var("NEMOBOT_LIVE_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let mut granted = vec!["OPENAI_API_KEY"];
    grant_if_set(&path, &mut granted, "TYPESAFE_API_KEY");
    let mut defaults = json!({"OPENAI_BASE_URL": OPENAI_BASE_URL_DEFAULT});
    granted.push("OPENAI_BASE_URL");
    if granted.contains(&"TYPESAFE_API_KEY") {
        granted.push("TYPESAFE_BASE_URL");
        defaults["TYPESAFE_BASE_URL"] = json!(TYPESAFE_BASE_URL_DEFAULT);
    }
    let config = config_with_defaults(&path, &granted, &defaults);
    let handle = open(&config, ClockMode::Wall);
    handle
        .send(build_event(&json!({
            "spec": SPEC,
            "constraints": [
                "send every light change and every walk signal to the invoker",
                "a button press during green ends green early",
            ],
            "max_attempts": 4,
            "model": model,
        })))
        .expect("the session accepts the build event");
    let deadline = Instant::now() + Duration::from_mins(30);
    let mut records = Vec::new();
    while Instant::now() < deadline {
        if let Some(record) = handle.recv_timeout(Duration::from_millis(250)) {
            let done = matches!(record, Egress::Closed { .. });
            records.push(record);
            if done {
                break;
            }
        }
    }
    let logs = logs(&records);
    let out = workspace().join("target/builder-live");
    std::fs::create_dir_all(&out).expect("the output directory is creatable");
    let pass = match logs.as_slice() {
        [(label, text)] if label == "builder.done" => {
            let done: Json = serde_json::from_str(text).expect("builder.done data is JSON");
            let scxml = done["scxml"].as_str().unwrap_or_default().to_owned();
            let valid = compile_source(
                "traffic_light.scxml",
                scxml.clone(),
                scxml_event_schema::Resources::new(),
                &Packages::default(),
            )
            .is_ok();
            std::fs::write(out.join("traffic_light.scxml"), &scxml).expect("writable");
            for member in ["schemas", "tests", "report"] {
                std::fs::write(
                    out.join(format!("{member}.json")),
                    serde_json::to_string_pretty(&done[member]).expect("JSON"),
                )
                .expect("writable");
            }
            println!(
                "live builder: attempts {}, failures {}, fixes {}, decisions {}",
                done["report"]["attempts"],
                done["report"]["failures"],
                done["report"]["fixes_applied"],
                done["report"]["decisions"]
            );
            valid
        }
        [(label, text)] if label == "builder.failed" => {
            std::fs::write(out.join("failed.json"), text).expect("writable");
            false
        }
        _ => false,
    };
    println!("live builder: {}", if pass { "pass" } else { "fail" });
    assert!(pass, "the live build did not pass");
}
