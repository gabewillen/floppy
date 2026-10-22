//! The memory ability (`abilities/memory/memory.scx`, the original's Memory) run as a
//! harness document's child through `scx run`: it migrates the database to
//! The original's head schema and applies one transaction of statements through
//! the sql service (the migration ability, `abilities/migration/migration.scx`,
//! brings it to the original's Alembic head first), answering its parent with `bot.ability.memory.output`
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
      <log label="kind" expr="_event.data.kind"/>
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
fn a_fresh_database_is_migrated_to_the_original_head_schema() {
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
                r#"{{"results":[{{"rowcount":1,"rows":[{{"columns":["group_concat(name)"],"values":["{columns}"]}}]}},{{"rowcount":1,"rows":[{{"columns":["name"],"values":["lifetime"]}}]}},{{"rowcount":1,"rows":[{{"columns":["version_num"],"values":["0003"]}}]}}]}}"#
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
fn the_head_schema_keeps_the_short_term_turns() {
    // Decision D126: the original's 0002_stm_turns, floppy's 0003.
    let directory = unique_scratch("memory");
    let finished = run(
        &directory,
        &harness(
            None,
            "[{sql: 'SELECT group_concat(name) FROM pragma_table_info(\\'bot_stm_turn\\')'}, {sql: 'SELECT name FROM sqlite_master WHERE type = \\'index\\' AND tbl_name = \\'bot_stm_turn\\''}]",
        ),
    );
    assert_eq!(
        logs(&finished.records),
        output(
            r#"[{"rowcount":1,"rows":[{"columns":["group_concat(name)"],"values":["seq,turn_id,at,direction,speakers_json,text,cut,conversation"]}]},{"rowcount":1,"rows":[{"columns":["name"],"values":["bot_stm_turn_at_idx"]}]}]"#
        ),
        "{}",
        finished.stderr
    );
}

#[test]
fn a_database_newer_than_the_ability_is_refused_untouched() {
    let directory = unique_scratch("memory");
    let finished = run(
        &directory,
        &harness(
            Some(
                "[{sql: 'CREATE TABLE alembic_version (version_num VARCHAR(32) NOT NULL)'}, {sql: 'INSERT INTO alembic_version VALUES (\\'0004\\')'}]",
            ),
            "[{sql: 'SELECT 1'}]",
        ),
    );
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            ("failed", "Migration could not plan from 0004 to head"),
            ("kind", "unavailable"),
        ]),
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
        support::labelled(&[
            (
                "failed",
                "no such column: nope in SELECT nope FROM bot_memory at offset 7"
            ),
            ("kind", "store_failed"),
        ]),
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

#[test]
fn statements_the_service_refuses_fail_as_invalid_input() {
    let finished = run(&unique_scratch("memory"), &harness(None, "[{sql: ''}]"));
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            ("failed", "Memory could not run (error.execution)"),
            ("kind", "invalid_input"),
        ]),
        "{}",
        finished.stderr
    );
}

#[test]
fn an_operation_over_its_timeout_fails_as_timeout() {
    // The original's operation timeout, set as the invoker's <param>.
    let document = harness(None, "[{sql: 'SELECT 1'}]");
    let statements = r#"<param name="statements" expr="[{sql: 'SELECT 1'}]"/>"#;
    assert!(
        document.contains(statements),
        "the harness passes statements"
    );
    let finished = run(
        &unique_scratch("memory"),
        &document.replacen(
            statements,
            &format!(r#"{statements}<param name="operation_timeout_ms" expr="1"/>"#),
            1,
        ),
    );
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            ("failed", "Memory did not finish within 0.001s."),
            ("kind", "timeout"),
        ]),
        "{}",
        finished.stderr
    );
}

/// One output record whose results are `results` (JSON text).
fn output(results: &str) -> Vec<(String, String)> {
    vec![("output".to_owned(), format!(r#"{{"results":{results}}}"#))]
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_fresh_database_is_migrated_to_the_original_head_schema() {
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
            r#"[{{"rowcount":1,"rows":[{{"columns":["columns"],"values":["{columns}"]}}]}},{{"rowcount":1,"rows":[{{"columns":["column_name","data_type"],"values":["lifetime","text"]}}]}},{{"rowcount":1,"rows":[{{"columns":["version_num"],"values":["0003"]}}]}}]"#
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
                "[{sql: 'CREATE TABLE alembic_version (version_num VARCHAR(32) PRIMARY KEY)'}, {sql: 'INSERT INTO alembic_version VALUES (\\'0004\\')'}]",
            ),
            "[{sql: 'SELECT 1'}]",
        ),
    );
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            ("failed", "Migration could not plan from 0004 to head"),
            ("kind", "unavailable"),
        ]),
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
        support::labelled(&[
            ("failed", r#"column "nope" does not exist"#),
            ("kind", "store_failed"),
        ]),
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

/// The recall statement `id` of the document at `path` (relative to the
/// workspace), as the document holds it: a `<data>` element's text, or
/// its `expr`, an ECMAScript string literal.
fn recall_select(path: &str, id: &str) -> String {
    let text =
        std::fs::read_to_string(support::workspace().join(path)).expect("the document is readable");
    let unescaped = |value: &str| {
        value
            .replace("&apos;", "'")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    };
    let element = format!(r#"<data id="{id}""#);
    let start = text
        .find(&element)
        .expect("the document has the data element")
        + element.len();
    let rest = &text[start..];
    if let Some(literal) = rest.strip_prefix(r#" expr="'"#) {
        let end = literal
            .find(r#"'"/>"#)
            .expect("the expr is one string literal");
        unescaped(&literal[..end]).replace("\\'", "'")
    } else {
        let body = rest.strip_prefix('>').expect("the data element holds text");
        let end = body.find("</data>").expect("the data element is closed");
        unescaped(&body[..end])
    }
}

/// Every recall takes the most recent window of its rows and reads it
/// oldest first (the original's `recent_select`): newest by `created_at`,
/// ties by `memory_id` (the insertion order floppy's ids keep), so once
/// more rows match than the window holds, the oldest are the ones left
/// out.
#[test]
fn every_recall_reads_the_most_recent_window_oldest_first() {
    // (document, data id, the select's parameters, the rows' tag)
    let recalls = [
        (
            "abilities/reasoning/reasoning.scx",
            "episode_select",
            serde_json::json!(["cognitive_episode", "c"]),
            "cognitive_episode",
        ),
        (
            "abilities/reflection/reflection.scx",
            "select_episodes",
            serde_json::json!(["c", "c"]),
            "cognitive_episode",
        ),
        (
            "abilities/conversation/conversation.scx",
            "memory_select",
            serde_json::json!(["conversation_memory", "c"]),
            "conversation_memory",
        ),
        (
            "abilities/identity/identity.scx",
            "name_select",
            serde_json::json!(["self_name", "self"]),
            "self_name",
        ),
    ];
    for (path, id, parameters, tag) in recalls {
        // 52 rows in one statement (one created_at for all but the first,
        // so memory_id breaks their ties), the first stamped later than
        // all of them.
        let statements = vec![
            serde_json::json!({
                "sql": "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 52) INSERT INTO bot_memory (memory_id, scope, context_ref, subject_ref, kind, sensitivity, retention, content, content_format, query_tags, created_at) SELECT printf('%032d', i), 'memory', 'c', 'self', 'fact', 'standard', 'retain', CAST(i AS TEXT), 'application/json', ?1, CASE WHEN i = 1 THEN '2999-01-01 00:00:00' ELSE '2000-01-01 00:00:00' END FROM n",
                "parameters": [tag],
            }),
            serde_json::json!({"sql": recall_select(path, id), "parameters": parameters}),
        ];
        let expression = serde_json::Value::Array(statements)
            .to_string()
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;");
        let directory = unique_scratch("memory");
        let finished = run(&directory, &harness(None, &expression));
        let records = logs(&finished.records);
        let [(label, text)] = records.as_slice() else {
            panic!("{path}: one record: {records:?} {}", finished.stderr);
        };
        assert_eq!(label, "output", "{path}: {text} {}", finished.stderr);
        let output: serde_json::Value = serde_json::from_str(text).expect("the output is JSON");
        let recalled: Vec<String> = output["results"][1]["rows"]
            .as_array()
            .expect("the select's rows")
            .iter()
            .map(|row| row["values"][0].as_str().unwrap_or_default().to_owned())
            .collect();
        // The newest 50: the later-stamped row 1, then 52 down to 4; read
        // oldest first.
        let expected: Vec<String> = (4..=52)
            .chain(std::iter::once(1))
            .map(|n| n.to_string())
            .collect();
        assert_eq!(recalled, expected, "{path}");
    }
}
