//! Integration test of the `files` service through `scx run`: a session
//! binds the real `floppy_service_files` component (resolved from its
//! repository URI) and invokes it
//! (`<invoke type="https://github.com/stateforward/floppy/services/files">`)
//! against the directory the run grants with `--dir URI=HOST::/behaviors`.
//! It writes `name` with `text`, reads `seed`, and answers as scxml.rs's files
//! example does, with the done data `{seed, wrote}`; a name that would
//! leave the directory is refused (`error.execution`), and a run that binds
//! it with no grant is refused before it starts (decision D121).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::PathBuf;
use std::time::Duration;

use support::{Finished, logs, run_document, service_uri, unique_scratch};

/// A fresh host directory granted to the files service at `/behaviors`.
fn granted() -> (PathBuf, Vec<String>) {
    let directory = unique_scratch("files");
    let grant = vec![
        "--dir".to_owned(),
        format!(
            "{}={}::/behaviors",
            service_uri("files"),
            directory.display()
        ),
    ];
    (directory, grant)
}

fn open(args: &[String], document: &str) -> Finished {
    run_document("files.scx", document, args, &[], Duration::from_secs(60))
}

/// Writes `Greet.scx`, then reads it back, logging each done data; then
/// tries to write outside the directory.
const WRITE_READ_ESCAPE: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="writing">
  <state id="writing">
    <invoke id="write" type="https://github.com/stateforward/floppy/services/files">
      <param name="name" expr="'Greet.scx'"/>
      <param name="text" expr="'&lt;scxml/&gt;'"/>
    </invoke>
    <transition event="done.invoke.write" target="reading">
      <log label="wrote" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="error.*" target="failed"/>
  </state>
  <state id="reading">
    <invoke id="read" type="https://github.com/stateforward/floppy/services/files">
      <param name="seed" expr="'Greet.scx'"/>
    </invoke>
    <transition event="done.invoke.read" target="escaping">
      <log label="read" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="error.*" target="failed"/>
  </state>
  <state id="escaping">
    <invoke id="escape" type="https://github.com/stateforward/floppy/services/files">
      <param name="name" expr="'../Greet.scx'"/>
      <param name="text" expr="'x'"/>
    </invoke>
    <transition event="done.invoke.escape" target="failed"/>
    <transition event="error.execution" target="done">
      <log label="refused" expr="_event.name"/>
    </transition>
  </state>
  <final id="done"/>
  <final id="failed"/>
</scxml>"#;

#[test]
fn it_writes_and_reads_the_granted_directory_and_refuses_a_name_that_leaves_it() {
    let (directory, grant) = granted();
    let finished = open(&grant, WRITE_READ_ESCAPE);
    assert_eq!(
        logs(&finished.records),
        [
            ("wrote".to_owned(), r#"{"seed":null,"wrote":8}"#.to_owned()),
            (
                "read".to_owned(),
                r#"{"seed":"<scxml/>","wrote":null}"#.to_owned()
            ),
            ("refused".to_owned(), "error.execution".to_owned()),
        ],
        "{}",
        finished.stderr
    );
    assert!(support::finished(&finished.records));
    assert_eq!(
        std::fs::read_to_string(directory.join("Greet.scx")).expect("the file is written"),
        "<scxml/>"
    );
    assert!(
        !directory
            .parent()
            .expect("a scratch parent")
            .join("Greet.scx")
            .exists(),
        "nothing is written outside the directory"
    );
}

#[test]
fn a_run_that_binds_it_with_no_grant_is_refused() {
    let path = support::write(
        &unique_scratch("files-ungranted"),
        "files.scx",
        WRITE_READ_ESCAPE,
    );
    let output = support::output(&[
        "run".to_owned(),
        "--clock".to_owned(),
        "logical".to_owned(),
        path.display().to_string(),
    ]);
    assert!(!output.status.success(), "{}", output.stderr);
    assert!(
        output
            .stderr
            .contains("whose resource the run does not grant"),
        "{}",
        output.stderr
    );
}
