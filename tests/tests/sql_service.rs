//! Integration test of the `sql` service through `scx run`: a session
//! binds the real `floppy_service_sql` component (resolved from its
//! repository URI) and invokes it
//! (`<invoke type="https://github.com/stateforward/floppy/services/sql">`)
//! against a database file in a directory the run grants with
//! `--dir URI=HOST::GUEST`. Each invocation is one SQLite transaction of
//! mosfet's `Statement`s; the service answers `sql.result` (mosfet's
//! `OutputData`) or `sql.failed` (mosfet's `FailureData`), then completes.
//! The file outlives the invocation and the run.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use support::{Finished, logs, run_document, service_uri, unique_scratch};

fn sql() -> String {
    service_uri("sql")
}

/// A fresh host directory granted to the sql service at `/memory`.
fn granted() -> (PathBuf, Vec<String>) {
    let directory = unique_scratch("sql");
    let grant = vec![
        "--dir".to_owned(),
        format!("{}={}::/memory", sql(), directory.display()),
    ];
    (directory, grant)
}

fn open(args: &[String], document: &str) -> Finished {
    run_document("sql.scx", document, args, &[], Duration::from_secs(60))
}

/// Creates a table and inserts an adopted name, then recalls the names in
/// a second invocation, logging each event's name and data.
const REMEMBER_AND_RECALL: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="storing">
  <datamodel>
    <data id="create">CREATE TABLE IF NOT EXISTS bot_memory (memory_id TEXT PRIMARY KEY, subject_ref TEXT, content TEXT NOT NULL, query_tags TEXT, created_at TEXT DEFAULT (CURRENT_TIMESTAMP) NOT NULL)</data>
    <data id="insert_name">INSERT INTO bot_memory (memory_id, subject_ref, content, query_tags) VALUES (?, 'self', ?, 'self_name')</data>
    <data id="select_names">SELECT content FROM bot_memory WHERE query_tags = ? AND subject_ref = ? ORDER BY created_at LIMIT 50</data>
  </datamodel>
  <state id="storing">
    <invoke id="store" type="https://github.com/stateforward/floppy/services/sql">
      <param name="database" expr="'/memory/floppy.sqlite'"/>
      <param name="statements" expr="[{sql: create}, {sql: insert_name, parameters: ['n1', JSON.stringify({text: 'Floppy', kind: 'fact'})]}]"/>
    </invoke>
    <transition event="sql.result">
      <log label="stored" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="done.invoke.store" target="recalling"/>
    <transition event="*" target="failed">
      <log label="failed" expr="_event.name + ' ' + JSON.stringify(_event.data)"/>
    </transition>
  </state>
  <state id="recalling">
    <invoke id="recall" type="https://github.com/stateforward/floppy/services/sql">
      <param name="database" expr="'/memory/floppy.sqlite'"/>
      <param name="statements" expr="[{sql: select_names, parameters: ['self_name', 'self']}]"/>
    </invoke>
    <transition event="sql.result">
      <log label="recalled" expr="JSON.stringify(_event.data.results[0].rows.map(r =&gt; JSON.parse(r.values[0]).text))"/>
    </transition>
    <transition event="done.invoke.recall" target="done"/>
    <transition event="*" target="failed">
      <log label="failed" expr="_event.name + ' ' + JSON.stringify(_event.data)"/>
    </transition>
  </state>
  <final id="done"/>
  <final id="failed"/>
</scxml>"#;

#[test]
fn an_insert_commits_and_a_later_invocation_recalls_it_from_the_granted_file() {
    let (directory, grant) = granted();
    let finished = open(&grant, REMEMBER_AND_RECALL);
    assert_eq!(
        logs(&finished.records),
        [
            (
                "stored".to_owned(),
                r#"{"results":[{"rowcount":0,"rows":[]},{"rowcount":1,"rows":[]}]}"#.to_owned()
            ),
            ("recalled".to_owned(), r#"["Floppy"]"#.to_owned()),
        ],
        "{}",
        finished.stderr
    );
    assert!(support::finished(&finished.records));
    assert!(
        directory.join("floppy.sqlite").is_file(),
        "the database is a file in the granted directory"
    );
    let consent = &finished.stderr;
    let resources = consent
        .lines()
        .find(|line| line.starts_with("  = WASI resources: --dir "))
        .expect("the sql service's directory is listed");
    assert!(resources.ends_with("::/memory"), "{consent}");
}

#[test]
fn the_database_outlives_the_run() {
    let (_, grant) = granted();
    let first = open(&grant, REMEMBER_AND_RECALL);
    assert!(support::finished(&first.records), "{}", first.stderr);
    // The second run inserts the same memory id: the primary key refuses
    // it, so the whole transaction rolls back and the service answers
    // mosfet's failure.
    let second = open(&grant, REMEMBER_AND_RECALL);
    assert_eq!(
        logs(&second.records),
        [(
            "failed".to_owned(),
            r#"sql.failed {"code":"SQLITE_CONSTRAINT","message":"UNIQUE constraint failed: bot_memory.memory_id"}"#.to_owned()
        )],
        "{}",
        second.stderr
    );
}

/// (`support::output` runs `scx` as given: `Run` would grant the directory.)
#[test]
fn a_run_that_binds_the_service_without_a_directory_is_refused() {
    let path = support::write(
        &unique_scratch("sql-documents"),
        "sql.scx",
        REMEMBER_AND_RECALL,
    );
    let refused = support::output(&[
        "run".to_owned(),
        "--clock".to_owned(),
        "logical".to_owned(),
        "--on-eof".to_owned(),
        "close".to_owned(),
        path.display().to_string(),
    ]);
    assert_eq!(refused.status.code(), Some(2), "{}", refused.stderr);
    assert!(refused.stdout.is_empty(), "nothing ran: {}", refused.stdout);
    assert!(
        refused
            .stderr
            .contains("imports a WASI interface whose resource the run does not grant"),
        "{}",
        refused.stderr
    );
}

/// A malformed `<param>` rejects the start: the `<invoke>` raises
/// `error.execution` and the file is never opened.
#[test]
fn a_malformed_parameter_raises_error_execution() {
    let (directory, grant) = granted();
    let finished = open(
        &grant,
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <invoke id="store" type="https://github.com/stateforward/floppy/services/sql">
      <param name="database" expr="'/memory/floppy.sqlite'"/>
      <param name="statements" expr="[]"/>
    </invoke>
    <transition event="error.execution" target="done">
      <log label="error" expr="_event.name"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"#,
    );
    assert_eq!(
        logs(&finished.records),
        [("error".to_owned(), "error.execution".to_owned())],
        "{}",
        finished.stderr
    );
    assert!(!Path::new(&directory.join("floppy.sqlite")).exists());
}
