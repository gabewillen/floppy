//! Smoke test: `scxml-cli run` opens live sessions of trivial SCXML
//! documents through the session components `scripts/build-components.sh`
//! builds, on the logical clock and on the wall clock.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::time::Duration;

use serde_json::json;
use support::{Run, closed_status, components, logs, of_type, scratch, write};

const TRIVIAL: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="idle"><state id="idle"><onentry><log label="idle"/></onentry><transition event="go" target="done"><log label="go" expr="_event.data"/></transition></state><final id="done"/></scxml>"#;

fn start(name: &str, document: &str, clock: &str) -> Run {
    let path = write(&scratch("smoke"), name, document);
    let mut args = components().session_args();
    args.extend([
        "--clock".to_owned(),
        clock.to_owned(),
        path.display().to_string(),
    ]);
    Run::start(&args, &[])
}

fn is(kind: &'static str) -> impl Fn(&serde_json::Value) -> bool {
    move |record| record["type"] == kind
}

#[test]
fn a_trivial_session_opens_takes_an_event_and_finishes() {
    let mut run = start("trivial.scxml", TRIVIAL, "logical");
    let mut records = run.until(Duration::from_secs(60), |record| {
        record["type"] == "call" && record["cause"] == "start"
    });
    assert_eq!(records[0]["type"], "opened");
    assert_eq!(
        records.last().map(|record| record["configuration"].clone()),
        Some(json!(["idle"]))
    );
    run.event("go", &json!("x"));
    records.extend(run.until(Duration::from_secs(30), is("closed")));
    let finished = run.finish(Duration::from_secs(30));
    assert!(finished.status.success(), "{}", finished.stderr);
    assert!(finished.records.is_empty(), "nothing follows the close");
    let labels: Vec<String> = logs(&records).into_iter().map(|(label, _)| label).collect();
    assert_eq!(labels, ["idle", "go"]);
    assert_eq!(closed_status(&records).as_deref(), Some("finished"));
}

#[test]
fn an_invalid_event_is_refused_and_the_session_keeps_running() {
    let mut run = start("refuse.scxml", TRIVIAL, "logical");
    run.until(Duration::from_secs(60), |record| {
        record["type"] == "call" && record["cause"] == "start"
    });
    run.event("not a name", &json!(null));
    let records = run.until(Duration::from_secs(30), is("error"));
    let error = of_type(&records, "error")[0];
    assert_eq!(error["fatal"], false, "{error}");
    let finished = run.close(Duration::from_secs(30));
    assert_eq!(finished.status.code(), Some(3), "{}", finished.stderr);
    assert_eq!(closed_status(&finished.records).as_deref(), Some("running"));
}

#[test]
fn a_wall_clock_session_fires_its_delayed_send() {
    let mut run = start(
        "delayed.scxml",
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="waiting"><state id="waiting"><onentry><send event="tick" delay="50ms"/></onentry><transition event="tick" target="done"><log label="ticked"/></transition></state><final id="done"/></scxml>"#,
        "wall",
    );
    run.end_input();
    let finished = run.finish(Duration::from_secs(60));
    assert!(finished.status.success(), "{}", finished.stderr);
    assert_eq!(
        logs(&finished.records),
        [("ticked".to_owned(), String::new())]
    );
    assert_eq!(
        closed_status(&finished.records).as_deref(),
        Some("finished")
    );
}
