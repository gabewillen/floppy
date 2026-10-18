//! Integration test of a decision through `scx run`: the `decision` namespace's
//! `<decision:choice>` reads the session snapshot and writes the machine's
//! three enabled `event:schema` transitions as the canonical choices, and the
//! `decision` service, invoked with those choices, asks a deterministic fake
//! `TypeSafe` `system_one` backend on 127.0.0.1 (nothing reaches the
//! network) off the session thread, fills the chosen event's data from
//! grounded options, validates it against the choices' schema, and returns it;
//! the machine's transition fires with that data. Both packages resolve from
//! their repository URIs. The key and base URL reach the run only through an
//! `--env-file`, and only the `<invoke>` whose `env` attributes name them
//! receives them; the component holds slot references and the host
//! substitutes the values into the admitted request.
//!
//! The ignored `live_decision_through_an_env_file` test makes one real call
//! (`live_filtered_decision_through_an_env_file` another, with the choices
//! filtered by `events`):
//! `FLOPPY_LIVE_ENV_FILE=<path> cargo test -p floppy-tests --test
//! decision_service -- --ignored`. The file must set `TYPESAFE_API_KEY`;
//! `TYPESAFE_BASE_URL` is the file's value, else `TypeSafe`'s, given as the
//! calling shell's value through `--env TYPESAFE_BASE_URL` (no backend URL is
//! compiled into the component). It reports pass or fail and the final state
//! reached; no value is ever printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value as Json, json};
use support::{Finished, labelled, logs, run_document, scratch, service_uri};

const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/decision";
/// The fake key (not a secret): the mock checks it arrives as the bearer.
const FAKE_KEY: &str = "ts-floppy-fake";

/// The flags of a run whose values come from `env_file`.
fn args(env_file: &Path) -> Vec<String> {
    vec!["--env-file".to_owned(), env_file.display().to_string()]
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
        "/abstain/v1/systemone" => "__unhandled__",
        "/stray/v1/systemone" => "order.refund",
        "/labels/v1/systemone" => "complaint",
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

/// Writes an env file holding the fake key and `base_url` for one test,
/// each only when `env` names its variable.
fn env_file(test: &str, base_url: &str, env: &[&str]) -> PathBuf {
    let path = scratch("decision-env").join(format!("{test}.env"));
    let key = format!("TYPESAFE_API_KEY={FAKE_KEY}\n");
    let base = format!("TYPESAFE_BASE_URL={base_url}\n");
    let mut contents = String::new();
    if env.contains(&"TYPESAFE_API_KEY") {
        contents.push_str(&key);
    }
    if env.contains(&"TYPESAFE_BASE_URL") {
        contents.push_str(&base);
    }
    std::fs::write(&path, contents).expect("the env file is writable");
    path
}

/// Runs `document` with `args` to its end, checking the consent listing
/// names both packages, the service's origin, and its slots.
fn open(args: &[String], document: &str, env: &[(&str, &str)], limit: Duration) -> Finished {
    let finished = run_document("decision.scx", document, args, env, limit);
    let consent = &finished.stderr;
    assert!(
        (!document.contains(NAMESPACE_URI)
            || consent.contains(&format!("element `{NAMESPACE_URI}`")))
            && consent.contains(&format!("service `{}`", service_uri("decision")))
            // The declared origin is public: it is listed even when a run
            // value equals it (the live run's TypeSafe default).
            && consent.contains("outbound HTTP: https://api.typesafe.ai")
            && consent.contains("env slots: apikey, baseurl"),
        "the consent listing names the package, its origin, and its slots: {consent}"
    );
    assert!(!consent.contains(FAKE_KEY), "names only: {consent}");
    finished
}

const SHIP_SCHEMA: &str = r#"{"description":"Ship the order to the customer.","type":"object","properties":{"sku":{"type":"string"},"carrier":{"enum":["ups","fedex"]}},"required":["sku","carrier"],"additionalProperties":false}"#;
const CANCEL_SCHEMA: &str = r#"{"description":"Cancel the order.","type":"object","properties":{"reason":{"enum":["customer","stock"]}},"required":["reason"],"additionalProperties":false}"#;
const HOLD_SCHEMA: &str =
    r#"{"description":"Put the order on hold.","type":"object","additionalProperties":false}"#;

/// An order machine: `<decision:choice>` on entry to `deciding` and the
/// decision service invoked there with `params` (besides the choices), whose
/// three `event:schema` transitions log `_event.data` and finish in their
/// own final state; `decision.abstain`, `decision.failed`, and
/// `error.execution` (no schema, so never offered) finish in `abstained`
/// and `failed`.
fn document(params: &str) -> String {
    filtered_document("", params)
}

/// [`document`] with `choices_attributes` added to its `<decision:choice>`.
fn filtered_document(choices_attributes: &str, params: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:decision="{NAMESPACE_URI}"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event"
       version="1.0" datamodel="ecmascript" initial="deciding">
  <datamodel><data id="order" expr="({{sku: 'A-1', qty: 2}})"/><data id="choices" expr="[]"/></datamodel>
  <state id="deciding">
    <onentry>
      <decision:choice location="choices"{choices_attributes}/>
    </onentry>
    <invoke id="decide" type="{service}" env:apikey="TYPESAFE_API_KEY" env:baseurl="TYPESAFE_BASE_URL">
      <param name="choices" expr="choices"/>
      {params}
    </invoke>
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
    <transition event="decision.failed" target="failed">
      <log label="code" expr="_event.data.code"/>
    </transition>
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="shipped"><onentry><log label="final" expr="'shipped'"/></onentry></final>
  <final id="cancelled"><onentry><log label="final" expr="'cancelled'"/></onentry></final>
  <final id="held"><onentry><log label="final" expr="'held'"/></onentry></final>
  <final id="abstained"><onentry><log label="final" expr="'abstained'"/></onentry></final>
  <final id="failed"><onentry><log label="final" expr="'failed'"/></onentry></final>
</scxml>"#,
        service = service_uri("decision")
    )
}

const CHOOSE: &str = r#"<param name="state" expr="order"/>"#;

fn run(route: &str, test: &str, env: &[&str]) -> (Vec<Json>, Mock) {
    let mock = Mock::start();
    let env_path = env_file(test, &format!("{}/{route}", mock.origin), env);
    let finished = open(
        &args(&env_path),
        &document(CHOOSE),
        &[],
        Duration::from_secs(60),
    );
    (finished.records, mock)
}

const BOTH: [&str; 2] = ["TYPESAFE_API_KEY", "TYPESAFE_BASE_URL"];

/// The selection criteria (the choices' events plus abstain) the backend
/// received for one run of the order machine whose `<decision:choice>`
/// carries `choices_attributes`, and the run's log records.
fn received_choices(
    choices_attributes: &str,
    route: &str,
    test: &str,
) -> (Json, Vec<(String, String)>) {
    let mock = Mock::start();
    let env_path = env_file(test, &format!("{}/{route}", mock.origin), &BOTH);
    let finished = open(
        &args(&env_path),
        &filtered_document(choices_attributes, CHOOSE),
        &[],
        Duration::from_secs(60),
    );
    let received = mock.received();
    assert_eq!(received.len(), 1, "exactly one request");
    let body: Json = serde_json::from_slice(&received[0].body).expect("the body is JSON");
    (
        body["questions"]["selection"]["criteria"].clone(),
        logs(&finished.records),
    )
}

#[test]
fn the_events_filter_narrows_the_choices_the_backend_receives() {
    let (unfiltered, records) = received_choices("", "ship", "unfiltered");
    assert_eq!(
        unfiltered,
        json!({
            "order.ship": "Ship the order to the customer.",
            "order.cancel": "Cancel the order.",
            "order.hold": "Put the order on hold.",
            "__unhandled__": "None of these: leave the turn unhandled.",
        }),
        "without events every schema transition is offered"
    );
    assert_eq!(
        records,
        labelled(&[
            ("data", r#"{"carrier":"fedex","sku":"A-1"}"#),
            ("final", "shipped"),
        ])
    );
    let (filtered, records) = received_choices(
        r#" events="order.cancel order.hold.*""#,
        "cancel",
        "filtered",
    );
    assert_eq!(
        filtered,
        json!({
            "order.cancel": "Cancel the order.",
            "order.hold": "Put the order on hold.",
            "__unhandled__": "None of these: leave the turn unhandled.",
        }),
        "the filter keeps only the events its descriptors match"
    );
    assert_eq!(
        records,
        labelled(&[("data", r#"{"reason":"customer"}"#), ("final", "cancelled")])
    );
}

#[test]
fn a_choices_filtered_to_leave_out_the_chosen_event_never_fires_it() {
    // The backend prefers order.ship, but the filter left it off the choices:
    // the service's answer is outside the choices it was given.
    let (filtered, records) = received_choices(r#" events="order.cancel""#, "ship", "excluded");
    assert_eq!(
        filtered,
        json!({
            "order.cancel": "Cancel the order.",
            "__unhandled__": "None of these: leave the turn unhandled.",
        })
    );
    assert_eq!(
        records,
        labelled(&[("code", "malformed"), ("final", "failed")])
    );
}

#[test]
fn choice_fires_the_chosen_schema_transition_with_schema_valid_data() {
    let (records, mock) = run("ship", "ship", &BOTH);
    assert_eq!(
        logs(&records),
        labelled(&[
            ("data", r#"{"carrier":"fedex","sku":"A-1"}"#),
            ("final", "shipped"),
        ]),
        "{records:?}"
    );
    assert!(support::finished(&records), "{records:?}");
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
            "__unhandled__": "None of these: leave the turn unhandled.",
        }),
        "exactly the three event:schema transitions are offered, plus abstain"
    );
    assert_eq!(
        body["questions"]["order.ship::sku"]["criteria"],
        json!({"A-1": "Which value fills 'sku' for order.ship?"}),
        "the sku is grounded in the dataexpr evidence"
    );
    assert_eq!(
        body["questions"]["order.ship::carrier"]["criteria"],
        json!({
            "ups": "Which value fills 'carrier' for order.ship?",
            "fedex": "Which value fills 'carrier' for order.ship?"
        }),
        "the carrier is grounded in the schema's enum"
    );
    assert_eq!(
        body["state"],
        json!({"qty": 2, "sku": "A-1"}),
        "the state is the invocation's state parameter"
    );
}

#[test]
fn choice_fires_another_transition_when_the_backend_prefers_it() {
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

/// `predicate` (the original's `TypeSafe` processor): every groundable event comes
/// back in one `decision.selected`, each with its data and its own
/// probability in the selection, and nothing is fired.
#[test]
fn predicate_returns_every_groundable_event_with_its_own_probability() {
    let mock = Mock::start();
    let env_path = env_file("predicate", &format!("{}/ship", mock.origin), &BOTH);
    let document = document(
        r#"<param name="kind" expr="'predicate'"/>
      <param name="state" expr="order"/>"#,
    )
    .replace(
        r#"<transition event="decision.abstain" target="abstained">"#,
        r#"<transition event="decision.selected" target="abstained">
      <log label="data" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="decision.abstain" target="abstained">"#,
    );
    let finished = open(&args(&env_path), &document, &[], Duration::from_secs(60));
    // The mock gives order.ship 0.8 and shares 0.2 among the other three
    // labels; each member question answers its first label.
    let records = logs(&finished.records);
    let (label, text) = records.first().expect("a data log");
    assert_eq!(label, "data", "{records:?}");
    let data: Json = serde_json::from_str(text).expect("JSON");
    let selections = data["selections"].as_array().expect("selections");
    let events: Vec<&str> = selections
        .iter()
        .map(|selection| selection["event"].as_str().expect("an event"))
        .collect();
    assert_eq!(events, ["order.ship", "order.cancel", "order.hold"]);
    assert_eq!(
        selections[0]["data"],
        json!({"carrier": "fedex", "sku": "A-1"})
    );
    assert_eq!(selections[1]["data"], json!({"reason": "customer"}));
    assert_eq!(selections[2]["data"], json!({}));
    assert_eq!(selections[0]["confidence"], json!(0.8));
    let other = selections[1]["confidence"].as_f64().expect("a number");
    assert!((other - 0.2 / 3.0).abs() < 1e-12, "{data}");
    assert_eq!(mock.received().len(), 1);
}

#[test]
fn choice_returns_decision_abstain_when_none_fits() {
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
fn an_answer_outside_the_choices_returns_decision_failed_and_no_event() {
    let (records, mock) = run("stray", "stray", &BOTH);
    assert_eq!(
        logs(&records),
        labelled(&[("code", "malformed"), ("final", "failed")]),
        "{records:?}"
    );
    assert_eq!(mock.received().len(), 1);
}

#[test]
fn without_a_granted_key_nothing_is_sent() {
    let (records, mock) = run("ship", "nokey", &["TYPESAFE_BASE_URL"]);
    assert_eq!(
        logs(&records),
        labelled(&[("code", "unconfigured"), ("final", "failed")]),
        "{records:?}"
    );
    assert!(mock.received().is_empty(), "no request without a key");
}

#[test]
fn a_choice_over_labels_returns_every_labels_probability() {
    let mock = Mock::start();
    let env_path = env_file("labels", &format!("{}/labels", mock.origin), &BOTH);
    let args = args(&env_path);
    let document = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript" initial="classifying">
  <state id="classifying">
    <invoke id="classify" type="{service}" env:apikey="TYPESAFE_API_KEY" env:baseurl="TYPESAFE_BASE_URL">
      <param name="kind" expr="'choice'"/>
      <param name="state" expr="'The order arrived damaged.'"/>
      <param name="labels" expr="['complaint', 'praise']"/>
    </invoke>
    <transition event="decision.classified" target="done">
      <log label="labels" expr="JSON.stringify(_event.data.labels)"/>
    </transition>
    <transition event="decision.failed" target="done"><log label="code" expr="_event.data.code"/></transition>
  </state>
  <final id="done"/>
</scxml>"#,
        service = service_uri("decision")
    );
    let records = open(&args, &document, &[], Duration::from_secs(60)).records;
    assert_eq!(
        logs(&records),
        labelled(&[("labels", r#"{"complaint":1,"praise":0}"#)]),
        "{records:?}"
    );
    assert_eq!(mock.received().len(), 1);
}

/// One real `system_one` decision through an env file
/// (`FLOPPY_LIVE_ENV_FILE`). Passes when the machine reaches one of its
/// schema transitions with data its schema admits, or abstains. Prints
/// only pass or fail and the final state's id.
#[test]
#[ignore = "makes one real TypeSafe call; needs FLOPPY_LIVE_ENV_FILE"]
fn live_decision_through_an_env_file() {
    live_decision("", &["shipped", "cancelled", "held"]);
}

/// [`live_decision_through_an_env_file`] with the choices filtered to
/// `order.ship` and `order.cancel`: passes only when the machine reaches
/// one of those with schema-valid data, or abstains (never `held`).
#[test]
#[ignore = "makes one real TypeSafe call; needs FLOPPY_LIVE_ENV_FILE"]
fn live_filtered_decision_through_an_env_file() {
    live_decision(
        r#" events="order.ship order.cancel""#,
        &["shipped", "cancelled"],
    );
}

/// One real decision of the order machine whose `<decision:choice>` carries
/// `choices_attributes`; `offered` are the final states its choices can reach.
fn live_decision(choices_attributes: &str, offered: &[&str]) {
    // The env file sets no TYPESAFE_BASE_URL: the service uses its package's
    // declared origin (decision D18).
    let params = r#"<param name="state" expr="order"/>
      <param name="instructions" expr="'A customer paid for order A-1 and asked for fast delivery. Choose what to do with the order.'"/>"#;
    let args: Vec<String> = support::live_env_files()
        .iter()
        .flat_map(|path| args(path))
        .collect();
    let records = open(
        &args,
        &filtered_document(choices_attributes, params),
        &[],
        Duration::from_secs(120),
    )
    .records;
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
                "abstained" => true,
                last if !offered.contains(&last) => false,
                "shipped" => admitted(SHIP_SCHEMA, data),
                "cancelled" => admitted(CANCEL_SCHEMA, data),
                "held" => admitted(HOLD_SCHEMA, data),
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
