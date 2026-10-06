//! nemobot-host integration test of the `decision` namespace: a live session
//! binds the real `nemobot_decision` extension component, whose
//! `<decision:choose/>` reads the session snapshot, offers the machine's
//! three `event:schema` transitions to a deterministic fake `TypeSafe`
//! `system_one` backend on 127.0.0.1 (nothing reaches the network), fills
//! the chosen event's data from grounded options, validates it against the
//! schema, and raises it; the machine's transition fires with that data.
//! The key and base URL reach the run only through an env file the config
//! names; the component holds slot references and the host substitutes the
//! values into the admitted request.
//!
//! `scripts/build-components.sh` builds and verifies the release session
//! components and the namespace components (import allowlists included) into
//! `target/test-decision-namespace/components`.
//!
//! The ignored `live_choose_through_an_env_file` test makes one real call:
//! `NEMOBOT_LIVE_ENV_FILE=<path> cargo test -p nemobot-host --test
//! decision_namespace -- --ignored`. It grants `TYPESAFE_API_KEY` and, when
//! the file sets it, `TYPESAFE_BASE_URL`, and reports pass or fail and the
//! final state reached; no value is ever printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

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
use scxml_host_wasmtime::SessionOutput;
use serde_json::{Map, Value as Json, json};
use sha2::{Digest as _, Sha256};

const NAMESPACE_URI: &str = "https://github.com/stateforward/nemobot/namespaces/decision@v0.1.0";
/// The fake key (not a secret): the mock checks it arrives as the bearer.
const FAKE_KEY: &str = "ts-nemobot-fake";

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the host crate sits in the nemobot workspace")
}

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("the artifact is readable");
    format!("{:x}", Sha256::digest(&bytes))
}

/// The verified artifacts: the session components' directory and manifest
/// digest, and the decision component and its digest.
struct Artifacts {
    components: PathBuf,
    manifest_sha256: String,
    component: PathBuf,
    component_sha256: String,
}

fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let dir = workspace().join("target/test-decision-namespace/components");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/build-components.sh"))
            .arg("--artifacts-dir")
            .arg(&dir)
            .current_dir(workspace())
            .status()
            .expect("the build script runs");
        assert!(status.success(), "the component proof must pass");
        let components = dir.join("scxml");
        let component = dir.join("namespaces/nemobot_decision.virtualized.wasm");
        Artifacts {
            manifest_sha256: sha256_file(&components.join("manifest.tsv")),
            component_sha256: sha256_file(&component),
            components,
            component,
        }
    })
}

/// A config binding the decision package, granting `env` from `env_file`.
fn config(env_file: &Path, env: &[&str]) -> Config {
    let artifacts = artifacts();
    let package = workspace().join("namespaces/decision/package/decision.extension.json");
    let text = json!({
        "format": CONFIG_FORMAT,
        "components": {
            "dir": artifacts.components.display().to_string(),
            "manifest-sha256": artifacts.manifest_sha256,
        },
        "env-file": env_file.display().to_string(),
        "env": env,
        "extensions": [{
            "manifest": package.display().to_string(),
            "component": artifacts.component.display().to_string(),
            "component-sha256": artifacts.component_sha256,
        }],
        "outbound-http": true,
    })
    .to_string();
    Config::parse(text.as_bytes(), &PathBuf::from("/")).expect("the test config is valid")
}

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    request_line: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// The mock's answer to one `system_one` request: the selection gives the
/// event `target` names (`/ship`, `/cancel`, `/hold`, `/abstain` for the
/// reserved label) 0.8 and shares 0.2 among the rest; every other question
/// gives its first label (in the body's canonical order) 1.0. `/stray`
/// answers a label the selection does not offer.
fn answer(target: &str, body: &[u8]) -> Option<String> {
    let chosen = match target {
        "/ship/v1/systemone" => "order.ship",
        "/cancel/v1/systemone" => "order.cancel",
        "/hold/v1/systemone" => "order.hold",
        "/abstain/v1/systemone" => "none of these",
        "/stray/v1/systemone" => "order.refund",
        _ => return None,
    };
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
            if target == "/stray/v1/systemone" {
                probabilities.insert(chosen.to_owned(), 0.8.into());
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
    Some(
        json!({"model": "jev-latest", "answers": answers, "usage": {"input_tokens": 10, "output_tokens": 1}})
            .to_string(),
    )
}

/// A deterministic fake `system_one` backend on 127.0.0.1: the first path
/// segment picks the scripted answer ([`answer`]); every request is
/// recorded.
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
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.trim_end().split_once(':') {
                        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
                    }
                }
                let header = |wanted: &str| {
                    headers
                        .iter()
                        .find(|(name, _)| name == wanted)
                        .map(|(_, value)| value.clone())
                };
                let length = header("content-length")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                let (status, text) = match answer(&target, &body) {
                    Some(text) => ("200 OK", text),
                    None => ("404 Not Found", "{}".to_owned()),
                };
                log.lock().expect("the log lock").push(Received {
                    request_line: request_line.trim_end().to_owned(),
                    authorization: header("authorization"),
                    body,
                });
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        Self { origin, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().expect("the log lock").clone()
    }
}

/// Writes an env file holding the fake key and `base_url` for one test.
fn env_file(test: &str, base_url: &str) -> PathBuf {
    let dir = workspace().join("target/test-decision-namespace/env");
    std::fs::create_dir_all(&dir).expect("the env directory is creatable");
    let path = dir.join(format!("{test}.env"));
    std::fs::write(
        &path,
        format!("TYPESAFE_API_KEY={FAKE_KEY}\nTYPESAFE_BASE_URL={base_url}\n"),
    )
    .expect("the env file is writable");
    path
}

fn open(config: &Config, document: &str) -> SessionHandle {
    let packages = Packages::load(config).expect("the decision package binds");
    let consent = packages.consent(&Values::none());
    assert!(
        consent.contains(NAMESPACE_URI)
            && consent.contains("http-origins: https://api.typesafe.ai")
            && consent.contains("env-slots: apikey, baseurl"),
        "the consent listing names the package, its origin, and its slots: {consent}"
    );
    let values = Values::granted(
        config
            .env_file
            .as_deref()
            .expect("the config names an env file"),
        &config.env,
        &|_| None,
    )
    .expect("the env file grants the variables");
    let compiled = compile_source(
        "decision.scxml",
        document.to_owned(),
        scxml_event_schema::Resources::new(),
        &packages,
    )
    .expect("the document compiles with the decision package");
    SessionHandle::open(
        config,
        packages,
        values,
        SessionSpec {
            compiled,
            session_id: "decision-namespace".into(),
            clock: ClockMode::Logical,
            documents: Vec::new(),
        },
    )
    .expect("the session opens")
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

fn labelled(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(label, text)| ((*label).to_owned(), (*text).to_owned()))
        .collect()
}

const SHIP_SCHEMA: &str = r#"{"description":"Ship the order to the customer.","type":"object","properties":{"sku":{"type":"string"},"carrier":{"enum":["ups","fedex"]}},"required":["sku","carrier"],"additionalProperties":false}"#;
const CANCEL_SCHEMA: &str = r#"{"description":"Cancel the order.","type":"object","properties":{"reason":{"enum":["customer","stock"]}},"required":["reason"],"additionalProperties":false}"#;
const HOLD_SCHEMA: &str =
    r#"{"description":"Put the order on hold.","type":"object","additionalProperties":false}"#;

/// An order machine: `<decision:choose/>` on entry to `deciding`, whose
/// three `event:schema` transitions log `_event.data` and finish in their
/// own final state; `decision.abstain` and `error.execution` (no schema, so
/// never offered) finish in `abstained` and `failed`.
fn document(choose: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:decision="{NAMESPACE_URI}"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event"
       version="1.0" datamodel="ecmascript" initial="deciding">
  <datamodel><data id="order" expr="({{sku: 'A-1', qty: 2}})"/></datamodel>
  <state id="deciding">
    <onentry>
      {choose}
    </onentry>
    <transition event="order.ship" event:schema='{SHIP_SCHEMA}' target="shipped">
      <log label="data" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="order.cancel" event:schema='{CANCEL_SCHEMA}' target="cancelled">
      <log label="data" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="order.hold" event:schema='{HOLD_SCHEMA}' target="held">
      <log label="data" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="decision.abstain" target="abstained">
      <log label="data" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="shipped"><onentry><log label="final" expr="'shipped'"/></onentry></final>
  <final id="cancelled"><onentry><log label="final" expr="'cancelled'"/></onentry></final>
  <final id="held"><onentry><log label="final" expr="'held'"/></onentry></final>
  <final id="abstained"><onentry><log label="final" expr="'abstained'"/></onentry></final>
  <final id="failed"><onentry><log label="final" expr="'failed'"/></onentry></final>
</scxml>"#
    )
}

const CHOOSE: &str = r#"<decision:choose dataexpr="order"
                        env:apikey="TYPESAFE_API_KEY" env:baseurl="TYPESAFE_BASE_URL"/>"#;

fn run(route: &str, test: &str, env: &[&str]) -> (Vec<Egress>, Mock) {
    let mock = Mock::start();
    let env_path = env_file(test, &format!("{}/{route}", mock.origin));
    let config = config(&env_path, env);
    let handle = open(&config, &document(CHOOSE));
    let records = until_closed(&handle, Duration::from_secs(60));
    (records, mock)
}

const BOTH: [&str; 2] = ["TYPESAFE_API_KEY", "TYPESAFE_BASE_URL"];

#[test]
fn choose_fires_the_chosen_schema_transition_with_schema_valid_data() {
    let (records, mock) = run("ship", "ship", &BOTH);
    assert_eq!(
        logs(&records),
        labelled(&[
            ("data", r#"{"carrier":"fedex","sku":"A-1"}"#),
            ("final", "shipped"),
        ]),
        "{records:?}"
    );
    assert!(finished(&records));
    let received = mock.received();
    assert_eq!(received.len(), 1, "exactly one request");
    assert_eq!(received[0].request_line, "POST /ship/v1/systemone HTTP/1.1");
    assert_eq!(
        received[0].authorization.as_deref(),
        Some(format!("Bearer {FAKE_KEY}").as_str()),
        "the host substitutes the env-file key into the bearer header"
    );
    let body: Json = serde_json::from_slice(&received[0].body).expect("the body is JSON");
    assert_eq!(
        String::from_utf8(received[0].body.clone()).expect("UTF-8"),
        scxml_event_schema::canonical_json(&body),
        "the request body is canonical JSON"
    );
    assert_eq!(
        body["questions"]["selection"]["criteria"],
        json!({
            "order.ship": "Ship the order to the customer.",
            "order.cancel": "Cancel the order.",
            "order.hold": "Put the order on hold.",
            "none of these": "None of these: take no event.",
        }),
        "exactly the three event:schema transitions are offered, plus abstain"
    );
    assert_eq!(
        body["questions"]["fill.0.0"]["criteria"],
        json!({"A-1": null}),
        "the sku is grounded in the dataexpr evidence"
    );
    assert_eq!(
        body["questions"]["fill.0.1"]["criteria"],
        json!({"ups": null, "fedex": null}),
        "the carrier is grounded in the schema's enum"
    );
    assert_eq!(
        body["state"]["configuration"]["atomic"],
        json!(["deciding"]),
        "the default state is the session snapshot root"
    );
}

#[test]
fn choose_fires_another_transition_when_the_backend_prefers_it() {
    let (records, _mock) = run("cancel", "cancel", &BOTH);
    assert_eq!(
        logs(&records),
        labelled(&[("data", r#"{"reason":"customer"}"#), ("final", "cancelled")]),
        "{records:?}"
    );
    let (records, _mock) = run("hold", "hold", &BOTH);
    assert_eq!(
        logs(&records),
        labelled(&[("data", "{}"), ("final", "held")]),
        "{records:?}"
    );
}

#[test]
fn choose_raises_decision_abstain_when_none_fits() {
    let (records, _mock) = run("abstain", "abstain", &BOTH);
    assert_eq!(
        logs(&records),
        labelled(&[
            ("data", r#"{"confidence":0.8,"reason":"declined"}"#),
            ("final", "abstained"),
        ]),
        "{records:?}"
    );
}

#[test]
fn an_answer_outside_the_menu_raises_error_execution_and_no_event() {
    let (records, mock) = run("stray", "stray", &BOTH);
    assert_eq!(
        logs(&records),
        labelled(&[("final", "failed")]),
        "{records:?}"
    );
    assert_eq!(mock.received().len(), 1);
}

#[test]
fn without_a_granted_key_nothing_is_sent() {
    let (records, mock) = run("ship", "nokey", &["TYPESAFE_BASE_URL"]);
    assert_eq!(
        logs(&records),
        labelled(&[("final", "failed")]),
        "{records:?}"
    );
    assert!(mock.received().is_empty(), "no request without a key");
}

/// One real `system_one` decision through an env file
/// (`NEMOBOT_LIVE_ENV_FILE`). Passes when the machine reaches one of its
/// schema transitions with data its schema admits, or abstains. Prints
/// only pass or fail and the final state's id.
#[test]
#[ignore = "makes one real TypeSafe call; needs NEMOBOT_LIVE_ENV_FILE"]
fn live_choose_through_an_env_file() {
    let Some(path) = std::env::var_os("NEMOBOT_LIVE_ENV_FILE").map(PathBuf::from) else {
        panic!("set NEMOBOT_LIVE_ENV_FILE to an env file that sets TYPESAFE_API_KEY");
    };
    // Grant the base URL too when the file sets it; the failure names the
    // variable only, never a value.
    let both = config(&path, &BOTH);
    let config = match Values::granted(&path, &both.env, &|_| None) {
        Ok(_) => both,
        Err(failure)
            if failure.kind() == FailureKind::EnvFile
                && failure.message().contains("`TYPESAFE_BASE_URL`") =>
        {
            config(&path, &["TYPESAFE_API_KEY"])
        }
        Err(failure) => panic!("the env file is unusable ({:?})", failure.kind()),
    };
    let choose = r#"<decision:choose dataexpr="order"
                        instructions="A customer paid for order A-1 and asked for fast delivery. Choose what to do with the order."
                        env:apikey="TYPESAFE_API_KEY" env:baseurl="TYPESAFE_BASE_URL"/>"#;
    let handle = open(&config, &document(choose));
    let records = until_closed(&handle, Duration::from_secs(120));
    let logs = logs(&records);
    let admitted = |schema: &str, data: &str| {
        let resolved = scxml_event_schema::resolve(schema.as_bytes(), None).ok();
        let compiled = resolved
            .and_then(|resolved| scxml_event_schema::CompiledSchema::new(&resolved.schema).ok());
        compiled.is_some_and(|compiled| {
            compiled.admits(&scxml_types::DataValue::JsonObject(data.to_owned()))
        })
    };
    let pass = match logs.as_slice() {
        [(data_label, data), (final_label, last)]
            if data_label == "data" && final_label == "final" =>
        {
            match last.as_str() {
                "shipped" => admitted(SHIP_SCHEMA, data),
                "cancelled" => admitted(CANCEL_SCHEMA, data),
                "held" => admitted(HOLD_SCHEMA, data),
                "abstained" => true,
                _ => false,
            }
        }
        _ => false,
    };
    // The final state's id is the document's own, never a value.
    let outcome = logs
        .last()
        .filter(|(label, _)| label == "final")
        .map_or("none", |(_, last)| last.as_str());
    println!(
        "live decision: {} ({outcome})",
        if pass { "pass" } else { "fail" }
    );
    assert!(pass, "the live decision did not pass");
}
