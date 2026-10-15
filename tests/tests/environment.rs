//! The environment (docs/plans/environment.mdscript.md): an SCXML document
//! run as the root session whose invoked children are its participants and
//! its devices, and the `device` namespace a bot reads its devices with
//! (decisions D102 to D110).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Run, copy_tree, logs, unique_scratch, workspace, write};

/// A root that invokes the lamp fixture as `lamp`, reads it with
/// `<device:snapshot>` by its address (`<parent _sessionid>.<invokeid>`,
/// D110) once it runs, turns it on, and reads it again.
const READER: &str = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:device="https://github.com/stateforward/floppy/namespaces/device"
       version="1.0" datamodel="ecmascript" initial="reading">
  <datamodel>
    <data id="read" expr="null"/>
  </datamodel>
  <state id="reading" initial="before">
    <invoke id="lamp" type="scxml" src="lamp/lamp.scx"/>
    <state id="before">
      <onentry><send event="read" delay="10ms"/></onentry>
      <transition event="read" target="after">
        <device:snapshot location="read" target="'#_scxml_' + _sessionid + '.lamp'"/>
        <log label="read" expr="JSON.stringify(read)"/>
        <send event="lamp.on" target="#_lamp"><param name="brightness" expr="40"/></send>
      </transition>
    </state>
    <state id="after">
      <onentry><send event="read" delay="10ms"/></onentry>
      <!-- Leaving reading would cancel the lamp before this content runs
           (SCXML 3.13), so the read stays inside it. -->
      <transition event="read" target="ending">
        <device:snapshot location="read" target="'#_lamp'"/>
        <log label="read" expr="JSON.stringify(read)"/>
      </transition>
    </state>
    <state id="ending">
      <transition target="done"/>
    </state>
  </state>
  <final id="done"/>
</scxml>"##;

/// Runs `document` as `name` beside a copy of the lamp fixture
/// (`lamp/lamp.scx`) on the logical clock until it ends.
fn run_beside_the_lamp(name: &str, document: &str) -> support::Finished {
    let dir = unique_scratch("environment");
    copy_tree(
        &workspace().join("tests/fixtures/environment/lamp"),
        &dir.join("lamp"),
    );
    let path = write(&dir, name, document);
    let mut run = Run::start(
        &[
            "--clock".to_owned(),
            "logical".to_owned(),
            path.display().to_string(),
        ],
        &[],
    );
    run.end_input();
    run.finish(Duration::from_secs(300))
}

/// The text of every `log` record labelled `label`, parsed as JSON.
fn logged(records: &[Json], label: &str) -> Vec<Json> {
    logs(records)
        .into_iter()
        .filter(|(logged, _)| logged == label)
        .map(|(_, text)| serde_json::from_str(&text).expect("the record is JSON"))
        .collect()
}

#[test]
fn a_device_snapshot_reads_another_session() {
    let finished = run_beside_the_lamp("reader.scx", READER);
    assert!(finished.status.success(), "{}", finished.stderr);
    assert_eq!(
        logged(&finished.records, "read"),
        [
            json!({"id": "session.lamp", "state": "off", "attributes": {"brightness": 0}, "events": ["lamp.on"]}),
            json!({"id": "session.lamp", "state": "on", "attributes": {"brightness": 40}, "events": ["lamp.off"]}),
        ],
        "{:?}",
        finished.records
    );
}
