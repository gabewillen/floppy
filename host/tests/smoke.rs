//! nemobot-host smoke test: opens live sessions of trivial SCXML documents
//! through the real scxml.rs session components under Wasmtime.
//!
//! `scripts/build-components.sh --session-only` builds and verifies the
//! release session components into `target/smoke/components/scxml`; the test
//! writes a nemobot config that trusts that directory's `manifest.tsv` digest,
//! then drives one session on the logical clock and one on the wall clock
//! through the ingress and egress channels.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use nemobot_host::compile::compile_source;
use nemobot_host::config::{CONFIG_FORMAT, Config};
use nemobot_host::env_file::Values;
use nemobot_host::packages::Packages;
use nemobot_host::runtime::{ClockMode, Egress, SessionHandle, SessionSpec, SessionStatus};
use scxml_host_wasmtime::{ScriptedEvent, SessionOutput};
use sha2::{Digest as _, Sha256};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the host crate sits in the nemobot workspace")
}

/// The verified session components and the config that trusts them.
fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let artifacts = workspace().join("target/smoke/components");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/build-components.sh"))
            .arg("--session-only")
            .arg("--artifacts-dir")
            .arg(&artifacts)
            .current_dir(workspace())
            .status()
            .expect("the build script runs");
        assert!(status.success(), "the session component proof must pass");
        let components = artifacts.join("scxml");
        let manifest =
            std::fs::read(components.join("manifest.tsv")).expect("the manifest is readable");
        let digest = format!("{:x}", Sha256::digest(&manifest));
        let text = format!(
            r#"{{"format":"{CONFIG_FORMAT}","components":{{"dir":"{}","manifest-sha256":"{digest}"}},"outbound-http":false}}"#,
            components.display()
        );
        Config::parse(text.as_bytes(), &PathBuf::from("/")).expect("the smoke config is valid")
    })
}

fn open(document: &str, clock: ClockMode) -> SessionHandle {
    let packages = Packages::load(config()).expect("no packages bind");
    let compiled = compile_source(
        "smoke.scxml",
        document.to_owned(),
        scxml_event_schema::Resources::new(),
        &packages,
    )
    .expect("the smoke document compiles");
    SessionHandle::open(
        config(),
        packages,
        Values::none(),
        SessionSpec {
            compiled,
            session_id: "smoke".into(),
            clock,
            documents: Vec::new(),
        },
    )
    .expect("the session opens")
}

/// Egress records until `done` holds for the last one, at most 30 s.
fn until(handle: &SessionHandle, done: impl Fn(&Egress) -> bool) -> Vec<Egress> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut records = Vec::new();
    while Instant::now() < deadline {
        if let Some(record) = handle.recv_timeout(Duration::from_millis(100)) {
            let finished = done(&record);
            records.push(record);
            if finished {
                return records;
            }
        }
    }
    panic!("no expected egress record within 30 s; got {records:?}");
}

fn log_labels(records: &[Egress]) -> Vec<String> {
    records
        .iter()
        .filter_map(|record| match record {
            Egress::Outputs(outputs) => Some(outputs),
            _ => None,
        })
        .flatten()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => entry.label.clone(),
            SessionOutput::Done { .. } => None,
        })
        .collect()
}

const TRIVIAL: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="idle"><state id="idle"><onentry><log label="idle"/></onentry><transition event="go" target="done"><log label="go" expr="_event.data"/></transition></state><final id="done"/></scxml>"#;

#[test]
fn a_trivial_session_opens_takes_an_event_and_finishes() {
    let handle = open(TRIVIAL, ClockMode::Logical);
    let opened = until(&handle, |record| matches!(record, Egress::Opened { .. }));
    assert_eq!(
        opened.last(),
        Some(&Egress::Opened {
            configuration: vec!["idle".into()]
        })
    );
    handle
        .send(ScriptedEvent::named("go"))
        .expect("the session thread takes events");
    let records = until(&handle, |record| matches!(record, Egress::Closed { .. }));
    let mut labels = log_labels(&opened);
    labels.extend(log_labels(&records));
    assert_eq!(labels, ["idle", "go"]);
    assert!(matches!(
        records.last(),
        Some(Egress::Closed {
            status: SessionStatus::Finished,
            ..
        })
    ));
    assert!(
        handle.close().expect("the thread ends cleanly").is_empty(),
        "nothing follows the close"
    );
}

#[test]
fn an_invalid_event_is_rejected_and_the_session_keeps_running() {
    let handle = open(TRIVIAL, ClockMode::Logical);
    until(&handle, |record| matches!(record, Egress::Opened { .. }));
    handle
        .send(ScriptedEvent::named("not a name"))
        .expect("the session thread takes events");
    let records = until(&handle, |record| matches!(record, Egress::Rejected { .. }));
    assert!(matches!(
        records.last(),
        Some(Egress::Rejected { event, .. }) if event == "not a name"
    ));
    let closing = handle.close().expect("the thread ends cleanly");
    assert!(matches!(
        closing.last(),
        Some(Egress::Closed {
            status: SessionStatus::Running,
            ..
        })
    ));
}

#[test]
fn a_wall_clock_session_fires_its_delayed_send() {
    let handle = open(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="waiting"><state id="waiting"><onentry><send event="tick" delay="50ms"/></onentry><transition event="tick" target="done"><log label="ticked"/></transition></state><final id="done"/></scxml>"#,
        ClockMode::Wall,
    );
    let records = until(&handle, |record| matches!(record, Egress::Closed { .. }));
    assert_eq!(log_labels(&records), ["ticked"]);
    assert!(matches!(
        records.last(),
        Some(Egress::Closed {
            status: SessionStatus::Finished,
            ..
        })
    ));
}
