//! nemobot-host integration test of the `scxml` namespace: a live session
//! binds the real `nemobot_scxml` extension component, validates a broken
//! draft with `<sx:validate>`, branches on the report, applies the
//! machine-applicable fixes, and validates the fixed text again.
//!
//! `scripts/build-components.sh` builds and verifies the release session
//! components and the namespace components (import allowlists included) into
//! `target/test-scxml-namespace/components`; the test writes a nemobot config
//! that trusts the session manifest's digest and binds the `scxml` package
//! with the verified component's digest.

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
use scxml_host_wasmtime::SessionOutput;
use sha2::{Digest as _, Sha256};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the host crate sits in the nemobot workspace")
}

/// The extension call fuel the namespace documents for `<sx:validate>`.
const EXTENSION_CALL_FUEL: u64 = 1_000_000_000;

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("the artifact is readable");
    format!("{:x}", Sha256::digest(&bytes))
}

/// The verified components and the config that binds the `scxml` package.
fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let artifacts = workspace().join("target/test-scxml-namespace/components");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/build-components.sh"))
            .arg("--artifacts-dir")
            .arg(&artifacts)
            .current_dir(workspace())
            .status()
            .expect("the build script runs");
        assert!(status.success(), "the component proof must pass");
        let components = artifacts.join("scxml");
        let manifest_digest = sha256_file(&components.join("manifest.tsv"));
        let component = artifacts.join("namespaces/nemobot_scxml.virtualized.wasm");
        let component_digest = sha256_file(&component);
        let package = workspace().join("namespaces/scxml/package/scxml.extension.json");
        let text = serde_json::json!({
            "format": CONFIG_FORMAT,
            "components": {
                "dir": components.display().to_string(),
                "manifest-sha256": manifest_digest,
            },
            "extensions": [{
                "manifest": package.display().to_string(),
                "component": component.display().to_string(),
                "component-sha256": component_digest,
            }],
            "outbound-http": false,
            // <sx:validate> runs the whole validator in one call.
            "extension-call-fuel": EXTENSION_CALL_FUEL,
        })
        .to_string();
        Config::parse(text.as_bytes(), &PathBuf::from("/")).expect("the test config is valid")
    })
}

fn open(document: &str) -> SessionHandle {
    let packages = Packages::load(config()).expect("the scxml package binds");
    let compiled = compile_source(
        "builder.scxml",
        document.to_owned(),
        scxml_event_schema::Resources::new(),
        &packages,
    )
    .expect("the document compiles with the scxml package");
    SessionHandle::open(
        config(),
        packages,
        Values::none(),
        SessionSpec {
            compiled,
            session_id: "scxml-namespace".into(),
            clock: ClockMode::Logical,
            documents: Vec::new(),
        },
    )
    .expect("the session opens")
}

/// Egress records until the session closes, at most 60 s.
fn until_closed(handle: &SessionHandle) -> Vec<Egress> {
    let deadline = Instant::now() + Duration::from_secs(60);
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
    panic!("the session did not close within 60 s; got {records:?}");
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

/// The builder's validate step in miniature: the draft's only error has a
/// machine-applicable fix (comma-separated event descriptors), so the
/// machine branches to `repair`, validates the fixed text, and finishes in
/// `valid`.
const VALIDATE_AND_REPAIR: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:sx="https://github.com/stateforward/nemobot/namespaces/scxml@v0.1.0"
       version="1.0" datamodel="ecmascript" initial="work">
  <datamodel>
    <data id="report"/>
    <data id="fixed"/>
    <data id="recheck"/>
  </datamodel>
  <state id="work" initial="check">
  <transition event="error.execution" target="errored"><log label="error" expr="JSON.stringify(_event.data)"/></transition>
  <state id="check">
    <onentry>
      <sx:validate location="report" name="draft.scxml"
                   applyfixes="machine-applicable" fixedlocation="fixed">
        <sx:content><![CDATA[
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a">
  <state id="a"><transition event="go,stop" target="b"/></state>
  <final id="b"/>
</scxml>
        ]]></sx:content>
      </sx:validate>
      <log label="errors" expr="report.validator.counts.errors"/>
      <log label="codes" expr="report.validator.diagnostics.map(function (d) { return d.code + ':' + d.fixes[0].applicability; }).join(',')"/>
    </onentry>
    <transition cond="report &amp;&amp; report.validator.counts.errors &gt; 0" target="repair"/>
    <transition cond="report &amp;&amp; report.validator.counts.errors === 0" target="valid"/>
  </state>
  <state id="repair">
    <onentry>
      <log label="fixed-has-space" expr="fixed.indexOf('event=&quot;go stop&quot;') &gt;= 0"/>
      <sx:validate location="recheck" expr="fixed"/>
      <log label="recheck-errors" expr="recheck.validator.counts.errors"/>
    </onentry>
    <transition cond="recheck &amp;&amp; recheck.validator.counts.errors === 0" target="valid"/>
    <transition cond="recheck &amp;&amp; recheck.validator.counts.errors &gt; 0" target="invalid"/>
  </state>
  </state>
  <final id="valid"><onentry><log label="final" expr="'valid'"/></onentry></final>
  <final id="invalid"><onentry><log label="final" expr="'invalid'"/></onentry></final>
  <final id="errored"><onentry><log label="final" expr="'errored'"/></onentry></final>
</scxml>"#;

#[test]
fn a_session_validates_a_broken_draft_branches_on_the_report_and_repairs_it() {
    let handle = open(VALIDATE_AND_REPAIR);
    let records = until_closed(&handle);
    let logs = logs(&records);
    let expected: Vec<(String, String)> = [
        ("errors", "1"),
        ("codes", "SCXML013:machine-applicable"),
        ("fixed-has-space", "true"),
        ("recheck-errors", "0"),
        ("final", "valid"),
    ]
    .into_iter()
    .map(|(label, text)| (label.to_owned(), text.to_owned()))
    .collect();
    assert_eq!(logs, expected, "{records:?}");
    assert!(matches!(
        records.last(),
        Some(Egress::Closed {
            status: SessionStatus::Finished,
            ..
        })
    ));
}

#[test]
fn a_malformed_element_raises_error_execution() {
    let handle = open(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:sx="https://github.com/stateforward/nemobot/namespaces/scxml@v0.1.0"
       version="1.0" datamodel="ecmascript" initial="check">
  <datamodel><data id="report"/></datamodel>
  <state id="check">
    <onentry><sx:validate location="report" expr="42"/></onentry>
    <transition event="error.execution" target="errored"/>
  </state>
  <final id="errored"><onentry><log label="final" expr="report === undefined ? 'unassigned' : 'assigned'"/></onentry></final>
</scxml>"#,
    );
    let records = until_closed(&handle);
    assert_eq!(
        logs(&records),
        [("final".to_owned(), "unassigned".to_owned())],
        "{records:?}"
    );
}
