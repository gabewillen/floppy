//! Integration test of the `scxml` namespace through `scxml-cli run`: a
//! session binds the real `nemobot_scxml` extension component (with the
//! extension call fuel `<sx:validate>` needs), validates a broken draft
//! with `<sx:validate>`, branches on the report, applies the
//! machine-applicable fixes, and validates the fixed text again.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::time::Duration;

use support::{Finished, components, logs, run_document};

/// The extension call fuel the namespace documents for `<sx:validate>`.
const EXTENSION_CALL_FUEL: &str = "1000000000";

/// Runs `document` with the session components and the `scxml` package.
fn open(name: &str, document: &str) -> Finished {
    let components = components();
    let mut args = components.session_args();
    args.extend(components.namespace("scxml"));
    // <sx:validate> runs the whole validator in one call.
    args.extend([
        "--extension-call-fuel".to_owned(),
        EXTENSION_CALL_FUEL.to_owned(),
    ]);
    run_document(name, document, &args, &[], Duration::from_secs(120))
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
    let finished = open("validate.scxml", VALIDATE_AND_REPAIR);
    let records = &finished.records;
    let logs = logs(records);
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
    assert!(finished.status.success(), "{}", finished.stderr);
    assert!(support::finished(records), "{records:?}");
}

#[test]
fn a_malformed_element_raises_error_execution() {
    let finished = open(
        "malformed.scxml",
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
    let records = &finished.records;
    assert_eq!(
        logs(records),
        [("final".to_owned(), "unassigned".to_owned())],
        "{records:?}"
    );
}
