#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! The `scx` namespace (`namespaces/scx`) end to end through `scx run`: the
//! binary embeds its package and component, so a document that binds
//! `https://github.com/stateforward/scxml.rs/namespaces/scx` runs
//! `<scx:validate>` with no flag and no fetch, under the default extension
//! call fuel. A session validates a broken draft, branches on the report,
//! applies the machine-applicable fixes, and validates the fixed text again.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn scratch(name: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/cli-scx-namespace")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

fn run(name: &str, document: &str) -> Output {
    let path = scratch(name).join(format!("{name}.scx"));
    fs::write(&path, document).expect("the document is writable");
    let mut child = Command::new(env!("CARGO_BIN_EXE_scx"))
        .args(["run", "--clock", "logical", "--on-eof", "close"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scx must start");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"")
        .expect("input is writable");
    child.wait_with_output().expect("scx must finish")
}

fn logs(output: &Output) -> Vec<(String, String)> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("one JSON record"))
        .filter(|record| record["type"] == "log")
        .map(|record| {
            (
                record["label"].as_str().unwrap_or_default().to_owned(),
                record["text"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// A builder's validate step in miniature: the draft's only error has a
/// machine-applicable fix (comma-separated event descriptors), so the
/// machine branches to `repair`, validates the fixed text, and finishes in
/// `valid`.
const VALIDATE_AND_REPAIR: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:scx="https://github.com/stateforward/scxml.rs/namespaces/scx"
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
      <scx:validate location="report" name="draft.scx"
                   applyfixes="machine-applicable" fixedlocation="fixed">
        <scx:content><![CDATA[
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a">
  <state id="a"><transition event="go,stop" target="b"/></state>
  <final id="b"/>
</scxml>
        ]]></scx:content>
      </scx:validate>
      <log label="errors" expr="report.validator.counts.errors"/>
      <log label="codes" expr="report.validator.diagnostics.map(function (d) { return d.code + ':' + d.fixes[0].applicability; }).join(',')"/>
    </onentry>
    <transition cond="report &amp;&amp; report.validator.counts.errors &gt; 0" target="repair"/>
    <transition cond="report &amp;&amp; report.validator.counts.errors === 0" target="valid"/>
  </state>
  <state id="repair">
    <onentry>
      <log label="fixed-has-space" expr="fixed.indexOf('event=&quot;go stop&quot;') &gt;= 0"/>
      <scx:validate location="recheck" expr="fixed"/>
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
    let output = run("validate", VALIDATE_AND_REPAIR);
    let stderr = String::from_utf8_lossy(&output.stderr);
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
    assert_eq!(logs(&output), expected, "{stderr}");
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("element `https://github.com/stateforward/scxml.rs/namespaces/scx`")
            && stderr.contains("built into scx"),
        "the consent listing names the embedded package: {stderr}"
    );
}

#[test]
fn a_malformed_element_raises_error_execution() {
    let output = run(
        "malformed",
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:scx="https://github.com/stateforward/scxml.rs/namespaces/scx"
       version="1.0" datamodel="ecmascript" initial="check">
  <datamodel><data id="report"/></datamodel>
  <state id="check">
    <onentry><scx:validate location="report" expr="42"/></onentry>
    <transition event="error.execution" target="errored"/>
  </state>
  <final id="errored"><onentry><log label="final" expr="report === undefined ? 'unassigned' : 'assigned'"/></onentry></final>
</scxml>"#,
    );
    assert_eq!(
        logs(&output),
        [("final".to_owned(), "unassigned".to_owned())],
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_resolves_the_embedded_package_with_no_flag() {
    let path = scratch("validate-only").join("draft.scx");
    fs::write(&path, VALIDATE_AND_REPAIR).expect("the document is writable");
    let output = Command::new(env!("CARGO_BIN_EXE_scx"))
        .arg("validate")
        .arg(&path)
        .output()
        .expect("scx must run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
