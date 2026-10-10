//! The memory ability (`abilities/memory/memory.scx`, mosfet's Memory) run as a
//! harness document's child through `scx run`: it migrates the database to
//! mosfet's head schema and applies one transaction of statements through
//! the sql service (the migration ability, `abilities/migration/migration.scx`,
//! brings it to mosfet's Alembic head first), answering its parent with `bot.ability.memory.output`
//! (the statements' results only) or `bot.ability.failed`.
//!
//! The `postgres_` tests run the same cases on the Postgres engine (the
//! pgsql service), each against a fresh `PGlite` server the test starts
//! ([`support::Pglite`]); they are ignored unless `FLOPPY_PGLITE_SERVER`
//! names `@electric-sql/pglite-socket`'s `dist/scripts/server.js`:
//! `FLOPPY_PGLITE_SERVER=<path> cargo test -p floppy-tests --test memory
//! -- --include-ignored`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::Path;
use std::time::Duration;

use support::{Finished, Pglite, Run, logs, service_uri, unique_scratch};

/// The SQLite database every SQLite case uses (in the granted directory).
const SQLITE: &str = "/memory/floppy.sqlite";

/// A harness that invokes the memory ability on the SQLite database with
/// `statements` (an ECMAScript expression), after `before` (statements for
/// the sql service directly, or `None`), logging what the memory answers.
fn harness(before: Option<&str>, statements: &str) -> String {
    harness_on(SQLITE, "sql", before, statements)
}

/// [`harness`] on `database` (a file path or a Postgres connection URI),
/// with `before` run by the floppy service `engine` directly.
fn harness_on(database: &str, engine: &str, before: Option<&str>, statements: &str) -> String {
    let engine = service_uri(engine);
    let (initial, aging) = before.map_or((String::from("remembering"), String::new()), |before| {
        (
            "aging".to_owned(),
            format!(
                r#"<state id="aging">
    <invoke id="age" type="{engine}">
      <param name="database" expr="'{database}'"/>
      <param name="statements" expr="{before}"/>
    </invoke>
    <transition event="done.invoke.age" target="remembering"/>
  </state>"#
            ),
        )
    });
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="{initial}">
  {aging}
  <state id="remembering">
    <invoke id="memory" type="scxml" src="memory/memory.scx">
      <param name="database" expr="'{database}'"/>
      <param name="statements" expr="{statements}"/>
    </invoke>
    <transition event="bot.ability.memory.output" target="done">
      <log label="output" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="bot.ability.failed" target="done">
      <log label="failed" expr="_event.data.message"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"#
    )
}

/// Runs `document` beside a copy of the memory ability, granting the sql
/// service `directory`.
fn run(directory: &Path, document: &str) -> Finished {
    let dir = unique_scratch("memory-ability");
    support::copy_ability("memory", &dir);
    let path = support::write(&dir, "harness.scx", document);
    let mut run = Run::start(
        &[
            "--dir".to_owned(),
            format!(
                "{}={}::/memory",
                support::service_uri("sql"),
                directory.display()
            ),
            "--clock".to_owned(),
            "logical".to_owned(),
            path.display().to_string(),
        ],
        &[],
    );
    run.end_input();
    run.finish(Duration::from_secs(300))
}

#[test]
fn a_fresh_database_is_migrated_to_mosfets_head_schema() {
    let directory = unique_scratch("memory");
    let finished = run(
        &directory,
        &harness(
            None,
            "[{sql: 'SELECT group_concat(name) FROM pragma_table_info(\\'bot_memory\\')'}, {sql: 'SELECT name FROM pragma_table_info(\\'bot_behavior\\') ORDER BY cid DESC LIMIT 1'}, {sql: 'SELECT version_num FROM alembic_version'}]",
        ),
    );
    let columns = "memory_id,scope,context_ref,subject_ref,kind,sensitivity,retention,content,content_format,query_tags,metadata_json,created_at";
    assert_eq!(
        logs(&finished.records),
        [(
            "output".to_owned(),
            format!(
                r#"{{"results":[{{"rowcount":1,"rows":[{{"columns":["group_concat(name)"],"values":["{columns}"]}}]}},{{"rowcount":1,"rows":[{{"columns":["name"],"values":["lifetime"]}}]}},{{"rowcount":1,"rows":[{{"columns":["version_num"],"values":["0002"]}}]}}]}}"#
            )
        )],
        "{}",
        finished.stderr
    );
    assert!(directory.join("floppy.sqlite").is_file());
    // A second invocation finds the head schema and migrates nothing.
    let again = run(
        &directory,
        &harness(None, "[{sql: 'SELECT count(*) AS n FROM bot_memory'}]"),
    );
    assert_eq!(
        logs(&again.records),
        [(
            "output".to_owned(),
            r#"{"results":[{"rowcount":1,"rows":[{"columns":["n"],"values":[0]}]}]}"#.to_owned()
        )],
        "{}",
        again.stderr
    );
}

#[test]
fn a_database_newer_than_the_ability_is_refused_untouched() {
    let directory = unique_scratch("memory");
    let finished = run(
        &directory,
        &harness(
            Some(
                "[{sql: 'CREATE TABLE alembic_version (version_num VARCHAR(32) NOT NULL)'}, {sql: 'INSERT INTO alembic_version VALUES (\\'0003\\')'}]",
            ),
            "[{sql: 'SELECT 1'}]",
        ),
    );
    assert_eq!(
        logs(&finished.records),
        [(
            "failed".to_owned(),
            "Migration could not plan from 0003 to head".to_owned()
        )],
        "{}",
        finished.stderr
    );
}

#[test]
fn a_failing_statement_fails_the_memory_and_commits_nothing() {
    let directory = unique_scratch("memory");
    let finished = run(
        &directory,
        &harness(
            None,
            "[{sql: 'INSERT INTO bot_memory (memory_id, content) VALUES (?1, ?2)', parameters: ['m1', 'x']}, {sql: 'SELECT nope FROM bot_memory'}]",
        ),
    );
    assert_eq!(
        logs(&finished.records),
        [(
            "failed".to_owned(),
            "no such column: nope in SELECT nope FROM bot_memory at offset 7".to_owned()
        )],
        "{}",
        finished.stderr
    );
    let counted = run(
        &directory,
        &harness(None, "[{sql: 'SELECT count(*) AS n FROM bot_memory'}]"),
    );
    assert_eq!(
        logs(&counted.records),
        [(
            "output".to_owned(),
            r#"{"results":[{"rowcount":1,"rows":[{"columns":["n"],"values":[0]}]}]}"#.to_owned()
        )]
    );
}

/// One output record whose results are `results` (JSON text).
fn output(results: &str) -> Vec<(String, String)> {
    vec![("output".to_owned(), format!(r#"{{"results":{results}}}"#))]
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_fresh_database_is_migrated_to_mosfets_head_schema() {
    let server = Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    let database = server.database();
    let directory = unique_scratch("memory");
    let finished = run(
        &directory,
        &harness_on(
            &database,
            "pgsql",
            None,
            "[{sql: 'SELECT string_agg(column_name, \\',\\' ORDER BY ordinal_position) AS columns FROM information_schema.columns WHERE table_name = ?', parameters: ['bot_memory']}, {sql: 'SELECT column_name, data_type FROM information_schema.columns WHERE table_name = ?1 ORDER BY ordinal_position DESC LIMIT 1', parameters: ['bot_behavior']}, {sql: 'SELECT version_num FROM alembic_version'}]",
        ),
    );
    let columns = "memory_id,scope,context_ref,subject_ref,kind,sensitivity,retention,content,content_format,query_tags,metadata_json,created_at";
    assert_eq!(
        logs(&finished.records),
        output(&format!(
            r#"[{{"rowcount":1,"rows":[{{"columns":["columns"],"values":["{columns}"]}}]}},{{"rowcount":1,"rows":[{{"columns":["column_name","data_type"],"values":["lifetime","text"]}}]}},{{"rowcount":1,"rows":[{{"columns":["version_num"],"values":["0002"]}}]}}]"#
        )),
        "{}",
        finished.stderr
    );
    // A second invocation finds the head schema and migrates nothing; the
    // defaulted timestamps are TIMESTAMPTZ.
    let again = run(
        &directory,
        &harness_on(
            &database,
            "pgsql",
            None,
            "[{sql: 'SELECT count(*) AS n FROM bot_memory'}, {sql: 'SELECT data_type FROM information_schema.columns WHERE table_name = ? AND column_name = ?', parameters: ['bot_memory', 'created_at']}]",
        ),
    );
    assert_eq!(
        logs(&again.records),
        output(
            r#"[{"rowcount":1,"rows":[{"columns":["n"],"values":[0]}]},{"rowcount":1,"rows":[{"columns":["data_type"],"values":["timestamp with time zone"]}]}]"#
        ),
        "{}",
        again.stderr
    );
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_database_newer_than_the_ability_is_refused_untouched() {
    let server = Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    let finished = run(
        &unique_scratch("memory"),
        &harness_on(
            &server.database(),
            "pgsql",
            Some(
                "[{sql: 'CREATE TABLE alembic_version (version_num VARCHAR(32) PRIMARY KEY)'}, {sql: 'INSERT INTO alembic_version VALUES (\\'0003\\')'}]",
            ),
            "[{sql: 'SELECT 1'}]",
        ),
    );
    assert_eq!(
        logs(&finished.records),
        [(
            "failed".to_owned(),
            "Migration could not plan from 0003 to head".to_owned()
        )],
        "{}",
        finished.stderr
    );
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_failing_statement_fails_the_memory_and_commits_nothing() {
    let server = Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    let database = server.database();
    let directory = unique_scratch("memory");
    let finished = run(
        &directory,
        &harness_on(
            &database,
            "pgsql",
            None,
            "[{sql: 'INSERT INTO bot_memory (memory_id, content) VALUES (?1, ?2)', parameters: ['m1', 'x']}, {sql: 'SELECT nope FROM bot_memory'}]",
        ),
    );
    assert_eq!(
        logs(&finished.records),
        [(
            "failed".to_owned(),
            r#"column "nope" does not exist"#.to_owned()
        )],
        "{}",
        finished.stderr
    );
    let counted = run(
        &directory,
        &harness_on(
            &database,
            "pgsql",
            None,
            "[{sql: 'SELECT count(*) AS n FROM bot_memory'}]",
        ),
    );
    assert_eq!(
        logs(&counted.records),
        output(r#"[{"rowcount":1,"rows":[{"columns":["n"],"values":[0]}]}]"#),
        "{}",
        counted.stderr
    );
}
