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
//!
//! The `stm_` tests run the short-term register (`abilities/stm/stm.scx`,
//! the original's `StmMemory`) as a harness's child over the same memory
//! ability: records and their recent read, capacity eviction, heard and
//! said turns and their recent read, the turns' shedding by halves, both
//! windows, and a register without a database or with one it cannot open.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::fmt::Write as _;
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
/// service `directory`, on the logical clock.
fn run(directory: &Path, document: &str) -> Finished {
    run_on(directory, document, "logical")
}

/// [`run`] on `clock` (`logical` or `wall`).
fn run_on(directory: &Path, document: &str, clock: &str) -> Finished {
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
            clock.to_owned(),
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
    // Decision D165: the original's 0002_stm_turns, floppy's 0003.
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
    // The original's operation timeout, set as the invoker's <param>, on
    // the wall clock (the logical clock does not move while a service
    // works), over a statement that runs for seconds, so the timer always
    // wins.
    let slow = "[{sql: 'WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x &lt; 50000000) SELECT count(*) FROM c'}]";
    let document = harness(None, slow);
    let statements = format!(r#"<param name="statements" expr="{slow}"/>"#);
    assert!(
        document.contains(&statements),
        "the harness passes statements"
    );
    let finished = run_on(
        &unique_scratch("memory"),
        &document.replacen(
            &statements,
            &format!(r#"{statements}<param name="operation_timeout_ms" expr="200"/>"#),
            1,
        ),
        "wall",
    );
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            ("failed", "Memory did not finish within 0.2s."),
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

/// A harness that invokes the short-term register with `database` (an
/// ECMAScript expression) and `params` (more `<param>`s, or none), runs
/// `stages[0]` once it is ready (`bot.ability.offers`) and `stages[n]` on
/// `harness.stage<n>` (a delayed `<send>` an earlier stage arms), and logs
/// each answer: `offers`, `recent` (each entry's stimulus name, payload,
/// envelope, id length and whether it was reinforced), `turns` (each
/// turn's id, direction, speakers, text (its length past 40 characters),
/// cut, conversation and whether its time is UTC) and `failed`, ending
/// after `answers` reads answered or failed.
fn stm_harness(database: &str, params: &str, stages: &[String], answers: usize) -> String {
    let mut transitions = String::new();
    for (n, stage) in stages.iter().enumerate() {
        let event = if n == 0 {
            "bot.ability.offers".to_owned()
        } else {
            format!("harness.stage{n}")
        };
        let offers = if n == 0 {
            r#"<log label="offers" expr="JSON.stringify(_event.data)"/>"#
        } else {
            ""
        };
        write!(
            transitions,
            "\n    <transition event=\"{event}\">{offers}{stage}</transition>"
        )
        .expect("writing to a String cannot fail");
    }
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="running">
  <datamodel>
    <data id="answers" expr="0"/>
  </datamodel>
  <state id="running">
    <invoke id="stm" type="scxml" src="stm/stm.scx">
      <param name="database" expr="{database}"/>{params}
    </invoke>{transitions}
    <transition event="bot.ability.memory.stm.recent.output">
      <log label="recent" expr="JSON.stringify(_event.data.entries.map((e) =&gt; [e.stimulus_name, e.payload, e.event_id, e.source, e.target, e.stm_id.length, e.last_accessed_at !== null]))"/>
      <assign location="answers" expr="answers + 1"/>
    </transition>
    <transition event="bot.ability.memory.stm.recent_turns.output">
      <log label="turns" expr="JSON.stringify(_event.data.turns.map((t) =&gt; [t.id, t.direction, t.speakers, t.text.length &gt; 40 ? t.text.length : t.text, t.cut, t.conversation, /Z$/.test(t.at)]))"/>
      <assign location="answers" expr="answers + 1"/>
    </transition>
    <transition event="bot.ability.failed">
      <log label="failed" expr="_event.data.message"/>
      <assign location="answers" expr="answers + 1"/>
    </transition>
    <transition cond="answers === {answers}" target="done"/>
  </state>
  <final id="done"/>
</scxml>"#
    )
}

/// `<send>` of `bot.ability.memory.stm.<event>` to the register with `data`
/// (an ECMAScript expression), XML-escaped.
fn to_stm(event: &str, data: &str) -> String {
    let data = data
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    format!(
        r##"<send event="bot.ability.memory.stm.{event}" target="#_stm"><content expr="{data}"/></send>"##
    )
}

/// The `<send>` that runs stage `n` after `delay`.
fn stage_after(n: usize, delay: &str) -> String {
    format!(r#"<send event="harness.stage{n}" delay="{delay}"/>"#)
}

/// A turn `id` in `direction` saying `text` (an ECMAScript expression),
/// recorded by `conversation`, with no speakers.
fn stm_turn(id: &str, direction: &str, text: &str, conversation: &str) -> String {
    to_stm(
        "turn",
        &format!(
            "({{id: '{id}', direction: '{direction}', speakers: [], text: {text}, cut: false, conversation: '{conversation}'}})"
        ),
    )
}

/// Runs `document` beside copies of the short-term register and the
/// memory ability on `clock`, granting the sql service a fresh directory.
fn run_stm(document: &str, clock: &str) -> Finished {
    let dir = unique_scratch("stm");
    support::copy_ability("stm", &dir);
    support::copy_ability("memory", &dir);
    let path = support::write(&dir, "harness.scx", document);
    let mut args = support::memory_grant();
    args.extend([
        "--clock".to_owned(),
        clock.to_owned(),
        path.display().to_string(),
    ]);
    let mut run = Run::start(&args, &[]);
    run.end_input();
    run.finish(Duration::from_secs(300))
}

const STM_READY: (&str, &str) = ("offers", r#"{"ability":"stm","choices":[]}"#);
const STM_DATABASE: &str = "'/memory/floppy.sqlite'";
const DEGRADED: &str = "unavailable: StmMemory is degraded.";

#[test]
fn stm_records_are_read_newest_first_and_evicted_beyond_capacity() {
    let stage = [
        to_stm(
            "record",
            "({stimulus_name: 'environment.sound', payload: {kind: 'knock'}})",
        ),
        to_stm(
            "record",
            "({stimulus_name: 'environment.visual', payload: {}})",
        ),
        to_stm(
            "record",
            "({stimulus_name: 'environment.sound', payload: {kind: 'tap'}})",
        ),
        to_stm("recent", "({})"),
        to_stm("recent", "({stimulus_name: 'environment.visual'})"),
        to_stm("recent", "({limit: 1})"),
    ]
    .concat();
    let finished = run_stm(
        &stm_harness(
            STM_DATABASE,
            r#"<param name="capacity" expr="2"/>"#,
            &[stage],
            3,
        ),
        "logical",
    );
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            STM_READY,
            (
                "recent",
                r#"[["environment.sound",{"kind":"tap"},null,null,null,32,false],["environment.visual",{},null,null,null,32,false]]"#
            ),
            (
                "recent",
                r#"[["environment.visual",{},null,null,null,32,false]]"#
            ),
            (
                "recent",
                r#"[["environment.sound",{"kind":"tap"},null,null,null,32,false]]"#
            ),
        ]),
        "{}",
        finished.stderr
    );
}

#[test]
fn stm_records_keep_their_envelope_and_a_reinforced_one_outlives_a_newer_one() {
    // The first record is reinforced after the second arrives, so the
    // third evicts the second.
    let stages = [
        [
            to_stm(
                "record",
                "({stimulus_name: 'environment.sound', payload: {kind: 'knock', count: 2}, event_id: 'e1', source: 'device-a', target: 'floppy'})",
            ),
            to_stm("recent", "({})"),
            stage_after(1, "100ms"),
        ]
        .concat(),
        [
            to_stm(
                "record",
                "({stimulus_name: 'environment.visual', payload: {}})",
            ),
            stage_after(2, "100ms"),
        ]
        .concat(),
        [
            to_stm(
                "reinforce",
                "({stm_ids: ['00000000000000000000000000000001']})",
            ),
            stage_after(3, "100ms"),
        ]
        .concat(),
        [
            to_stm(
                "record",
                "({stimulus_name: 'environment.tick', payload: {n: 1}})",
            ),
            to_stm("recent", "({})"),
        ]
        .concat(),
    ];
    let finished = run_stm(
        &stm_harness(
            STM_DATABASE,
            r#"<param name="capacity" expr="2"/>"#,
            &stages,
            2,
        ),
        "wall",
    );
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            STM_READY,
            (
                "recent",
                r#"[["environment.sound",{"count":2,"kind":"knock"},"e1","device-a","floppy",32,false]]"#
            ),
            (
                "recent",
                r#"[["environment.tick",{"n":1},null,null,null,32,false],["environment.sound",{"count":2,"kind":"knock"},"e1","device-a","floppy",32,true]]"#
            ),
        ]),
        "{}",
        finished.stderr
    );
}

#[test]
fn stm_turns_heard_and_said_are_read_oldest_first_and_long_words_are_cut() {
    let stage = [
        to_stm(
            "turn",
            "({id: 't1', direction: 'heard', speakers: [{ref: 'voice:3f2a9c', role: 'source'}], text: 'Hi there, I am Maya.', cut: false, conversation: 'listening'})",
        ),
        to_stm(
            "turn",
            "({id: 't2', direction: 'said', speakers: [{ref: 'voice:tts:warm', role: 'source', own: true}], text: 'Hello Maya.', cut: false, conversation: 'conversation'})",
        ),
        stm_turn(
            "t3",
            "heard",
            "'word '.repeat(409) + 'longword'",
            "listening",
        ),
        to_stm("recent_turns", "({})"),
        to_stm("recent_turns", "({limit: 1})"),
    ]
    .concat();
    let finished = run_stm(&stm_harness(STM_DATABASE, "", &[stage], 2), "logical");
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            STM_READY,
            ("stm.turn.recorded", "heard cut=false"),
            ("stm.turn.recorded", "said cut=false"),
            ("stm.turn.recorded", "heard cut=true"),
            (
                "turns",
                r#"[["t1","heard",[{"ref":"voice:3f2a9c","role":"source"}],"Hi there, I am Maya.",false,"listening",true],["t2","said",[{"own":true,"ref":"voice:tts:warm","role":"source"}],"Hello Maya.",false,"conversation",true],["t3","heard",[],2044,true,"listening",true]]"#
            ),
            ("turns", r#"[["t3","heard",[],2044,true,"listening",true]]"#),
        ]),
        "{}",
        finished.stderr
    );
}

#[test]
fn stm_turns_beyond_capacity_shed_the_older_half() {
    let mut stage: Vec<String> = (1..=5)
        .map(|n| {
            stm_turn(
                &format!("t{n}"),
                "heard",
                &format!("'turn {n}'"),
                "listening",
            )
        })
        .collect();
    stage.push(to_stm("recent_turns", "({})"));
    let finished = run_stm(
        &stm_harness(
            STM_DATABASE,
            r#"<param name="turn_capacity" expr="4"/>"#,
            &[stage.concat()],
            1,
        ),
        "logical",
    );
    let mut expected = vec![STM_READY];
    expected.extend([("stm.turn.recorded", "heard cut=false"); 5]);
    expected.push((
        "turns",
        r#"[["t4","heard",[],"turn 4",false,"listening",true],["t5","heard",[],"turn 5",false,"listening",true]]"#,
    ));
    assert_eq!(
        logs(&finished.records),
        support::labelled(&expected),
        "{}",
        finished.stderr
    );
}

#[test]
fn stm_reads_keep_to_their_windows() {
    // 1 s windows: the records' cutoff is 1 s back and the turns' floor at
    // most 1.5 s back, so what was written 1.7 s before is not read;
    // within narrows the turns' window and never widens it.
    let stages = [
        [
            to_stm("record", "({stimulus_name: 'old', payload: {}})"),
            stm_turn("t1", "heard", "'old'", "listening"),
            stage_after(1, "1700ms"),
        ]
        .concat(),
        [
            to_stm("record", "({stimulus_name: 'new', payload: {}})"),
            stm_turn("t2", "said", "'new'", "conversation"),
            to_stm("recent", "({})"),
            to_stm("recent_turns", "({})"),
            to_stm("recent_turns", "({within: 3600000})"),
        ]
        .concat(),
    ];
    let finished = run_stm(
        &stm_harness(
            STM_DATABASE,
            r#"<param name="recency_window_ms" expr="1000"/><param name="turn_window_ms" expr="1000"/>"#,
            &stages,
            3,
        ),
        "wall",
    );
    let new_turn = r#"[["t2","said",[],"new",false,"conversation",true]]"#;
    assert_eq!(
        logs(&finished.records),
        support::labelled(&[
            STM_READY,
            ("stm.turn.recorded", "heard cut=false"),
            ("stm.turn.recorded", "said cut=false"),
            ("recent", r#"[["new",{},null,null,null,32,false]]"#),
            ("turns", new_turn),
            ("turns", new_turn),
        ]),
        "{}",
        finished.stderr
    );
}

#[test]
fn stm_without_a_database_holds_nothing_and_one_it_cannot_open_degrades_it() {
    let sends = [
        to_stm(
            "record",
            "({stimulus_name: 'environment.sound', payload: {}})",
        ),
        stm_turn("t1", "heard", "'hi'", "listening"),
        to_stm("recent", "({})"),
        to_stm("recent_turns", "({})"),
    ]
    .concat();
    let absent = run_stm(
        &stm_harness("null", "", std::slice::from_ref(&sends), 2),
        "logical",
    );
    assert_eq!(
        logs(&absent.records),
        support::labelled(&[
            ("memory", "absent"),
            STM_READY,
            ("recent", "[]"),
            ("turns", "[]"),
        ]),
        "{}",
        absent.stderr
    );
    // Degraded, it never offers: the harness sends once the register is
    // running (stage 1, armed on entry), and what arrives while it opens
    // waits.
    let document = stm_harness("'/nowhere/floppy.sqlite'", "", &[String::new(), sends], 3)
        .replacen(
            r#"<state id="running">"#,
            &format!(
                r#"<state id="running"><onentry>{}</onentry>"#,
                stage_after(1, "1ms")
            ),
            1,
        );
    let finished = run_stm(&document, "logical");
    let records = logs(&finished.records);
    assert!(
        records
            .first()
            .is_some_and(|(label, text)| label == "stm.open.failed"
                && text.starts_with("short-term memory could not open its database: ")),
        "{records:?} {}",
        finished.stderr
    );
    assert_eq!(
        records.get(1..).unwrap_or_default(),
        support::labelled(&[
            ("failed", DEGRADED),
            ("stm.dropped", "degraded"),
            ("stm.dropped", "degraded"),
            ("failed", DEGRADED),
            ("failed", DEGRADED),
        ])
        .as_slice(),
        "{}",
        finished.stderr
    );
}
