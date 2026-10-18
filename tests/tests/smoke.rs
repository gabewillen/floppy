//! Smoke test: `scx run` opens live sessions of trivial SCXML documents
//! through the session components it embeds (`scripts/build-components.sh`
//! builds them), on the logical clock and on the wall clock.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::time::Duration;

use serde_json::json;
use support::{Run, closed_status, logs, of_type, scratch, write};

const TRIVIAL: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="idle"><state id="idle"><onentry><log label="idle"/></onentry><transition event="go" target="done"><log label="go" expr="_event.data"/></transition></state><final id="done"/></scxml>"#;

fn start(name: &str, document: &str, clock: &str) -> Run {
    let path = write(&scratch("smoke"), name, document);
    let args = [
        "--clock".to_owned(),
        clock.to_owned(),
        path.display().to_string(),
    ];
    Run::start(&args, &[])
}

fn is(kind: &'static str) -> impl Fn(&serde_json::Value) -> bool {
    move |record| record["type"] == kind
}

#[test]
fn a_trivial_session_opens_takes_an_event_and_finishes() {
    let mut run = start("trivial.scx", TRIVIAL, "logical");
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
    let mut run = start("refuse.scx", TRIVIAL, "logical");
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
        "delayed.scx",
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

/// `scripts/build-components.sh` publishes the locally built packages into
/// its own dev cache under the artifacts directory, pinned under their
/// tagless repository URIs, and never into the release cache
/// (`${XDG_CACHE_HOME}/scxml/extensions`), so a dev build cannot overwrite a
/// release's entries.
#[test]
fn the_dev_build_publishes_under_target_and_leaves_the_release_cache_untouched() {
    use sha2::{Digest as _, Sha256};

    let components = support::components();
    assert!(
        !components.release_home.join("scxml").exists(),
        "the build wrote below XDG_CACHE_HOME"
    );
    assert!(components.cache.starts_with(&components.dir));
    for uri in [
        "https://github.com/stateforward/floppy/namespaces/decision",
        "https://github.com/stateforward/floppy/namespaces/bot",
        "https://github.com/stateforward/floppy/namespaces/conversation",
        "https://github.com/stateforward/floppy/namespaces/device",
        "https://github.com/stateforward/floppy/namespaces/processing",
        "https://github.com/stateforward/floppy/services/generate",
        "https://github.com/stateforward/floppy/services/decision",
    ] {
        let entry = components
            .cache
            .join("index")
            .join(format!("{:x}.json", Sha256::digest(uri.as_bytes())));
        let text = std::fs::read_to_string(&entry)
            .unwrap_or_else(|error| panic!("{uri} is pinned in the dev cache: {error}"));
        assert!(
            text.contains(&format!("\"{uri}\",\n  \"tag\": \"v0.1.0\",")),
            "{text}"
        );
    }
}
