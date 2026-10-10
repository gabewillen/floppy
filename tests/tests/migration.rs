//! The migration ability (`abilities/migration.scx`, Alembic's
//! `command.upgrade` and `command.downgrade`) run as a harness document's
//! child through `scx run`, with mosfet's memory revisions
//! (`abilities/memory/*.json`): it moves a database between revisions in
//! one transaction, keeps Alembic's `alembic_version`, and answers its
//! parent with `bot.ability.migration.output {from, to}` or
//! `bot.ability.failed`.
//!
//! The `postgres_` tests run the same walk on the Postgres engine (the
//! pgsql service) with the Postgres revisions, each against a fresh server
//! ([`support::Pglite`]); they are ignored unless `FLOPPY_PGLITE_SERVER`
//! (or `FLOPPY_POSTGRES_BIN`) is set: `cargo test -p floppy-tests --test
//! migration -- --include-ignored`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::Path;
use std::time::Duration;

use support::{Pglite, Run, logs, service_uri, unique_scratch};

/// The SQLite database every SQLite case uses (in the granted directory).
const SQLITE: &str = "/memory/floppy.sqlite";

/// A harness that migrates the database to `target` (an ECMAScript
/// expression), then runs `inspect` (sql statements) and logs both answers.
fn harness(target: &str, inspect: &str) -> String {
    harness_on(SQLITE, "sql", "sqlite", target, inspect)
}

/// [`harness`] on `database` (a file path or a Postgres connection URI)
/// with the `dialect` revisions, inspected through the floppy service
/// `engine`.
fn harness_on(database: &str, engine: &str, dialect: &str, target: &str, inspect: &str) -> String {
    let engine = service_uri(engine);
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="migrating">
  <datamodel>
    <data id="revision_0001" src="memory/{dialect}/0001_initial_memory_schema.json"/>
    <data id="revision_0002" src="memory/{dialect}/0002_behavior_lifetime.json"/>
  </datamodel>
  <state id="migrating">
    <invoke id="migration" type="scxml" src="migration.scx">
      <param name="database" expr="'{database}'"/>
      <param name="revisions" expr="[revision_0001, revision_0002]"/>
      <param name="target" expr="{target}"/>
    </invoke>
    <transition event="bot.ability.migration.output" target="inspecting">
      <log label="output" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="bot.ability.failed" target="inspecting">
      <log label="failed" expr="_event.data.message"/>
    </transition>
  </state>
  <state id="inspecting">
    <invoke id="inspect" type="{engine}">
      <param name="database" expr="'{database}'"/>
      <param name="statements" expr="{inspect}"/>
    </invoke>
    <transition event="sql.result" target="done">
      <log label="inspect" expr="JSON.stringify(_event.data.results.map(result =&gt; result.rows.map(row =&gt; row.values)))"/>
    </transition>
    <transition event="sql.failed" target="done">
      <log label="inspect.failed" expr="_event.data.message"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"#
    )
}

/// Migrates the database in `directory` to `target`, then inspects it;
/// returns the logs.
fn migrate(directory: &Path, target: &str, inspect: &str) -> Vec<(String, String)> {
    run(directory, &harness(target, inspect))
}

/// Runs `document` beside a copy of the memory ability's revisions and the
/// migration ability, granting the sql service `directory`; returns the
/// logs.
fn run(directory: &Path, document: &str) -> Vec<(String, String)> {
    let dir = unique_scratch("migration-ability");
    support::copy_memory(&dir);
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
    let finished = run.finish(Duration::from_secs(300));
    let logs = logs(&finished.records);
    assert!(!logs.is_empty(), "{}", finished.stderr);
    logs
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    support::labelled(pairs)
}

/// The version row, the bot tables, and whether `bot_behavior` has lifetime.
const SCHEMA: &str = "[{sql: 'SELECT version_num FROM alembic_version'}, {sql: 'SELECT count(*) FROM sqlite_master WHERE type = \\'table\\' AND name LIKE \\'bot_%\\''}, {sql: 'SELECT count(*) FROM pragma_table_info(\\'bot_behavior\\') WHERE name = \\'lifetime\\''}]";

#[test]
fn upgrade_and_downgrade_walk_the_revisions_and_keep_alembic_version() {
    let directory = unique_scratch("migration");
    assert_eq!(
        migrate(&directory, "'head'", SCHEMA),
        pairs(&[
            ("output", r#"{"from":null,"to":"0002"}"#),
            ("inspect", r#"[[["0002"]],[[4]],[[1]]]"#),
        ])
    );
    // At head already: nothing is applied.
    assert_eq!(
        migrate(&directory, "'head'", SCHEMA),
        pairs(&[
            ("output", r#"{"from":"0002","to":"0002"}"#),
            ("inspect", r#"[[["0002"]],[[4]],[[1]]]"#),
        ])
    );
    assert_eq!(
        migrate(&directory, "'-1'", SCHEMA),
        pairs(&[
            ("output", r#"{"from":"0002","to":"0001"}"#),
            ("inspect", r#"[[["0001"]],[[4]],[[0]]]"#),
        ])
    );
    assert_eq!(
        migrate(&directory, "'base'", SCHEMA),
        pairs(&[
            ("output", r#"{"from":"0001","to":null}"#),
            ("inspect", "[[],[[0]],[[0]]]"),
        ])
    );
}

#[test]
fn a_database_at_an_older_revision_is_upgraded_in_place() {
    let directory = unique_scratch("migration");
    assert_eq!(
        migrate(
            &directory,
            "'0001'",
            "[{sql: 'INSERT INTO bot_behavior (name, source) VALUES (\\'greet\\', \\'x\\')'}]"
        ),
        pairs(&[
            ("output", r#"{"from":null,"to":"0001"}"#),
            ("inspect", "[[]]"),
        ])
    );
    // Rows stored before 0002 take its server default.
    assert_eq!(
        migrate(
            &directory,
            "'head'",
            "[{sql: 'SELECT name, lifetime FROM bot_behavior'}]"
        ),
        pairs(&[
            ("output", r#"{"from":"0001","to":"0002"}"#),
            ("inspect", r#"[[["greet","turn"]]]"#),
        ])
    );
}

#[test]
fn an_unknown_target_fails_and_commits_nothing() {
    let directory = unique_scratch("migration");
    assert_eq!(
        migrate(
            &directory,
            "'0009'",
            "[{sql: 'SELECT count(*) FROM alembic_version'}, {sql: 'SELECT count(*) FROM sqlite_master WHERE name LIKE \\'bot_%\\''}]"
        ),
        pairs(&[
            ("failed", "Migration could not plan from null to 0009"),
            ("inspect", "[[[0]],[[0]]]"),
        ])
    );
}

/// The version row, the bot tables, and whether `bot_behavior` has
/// lifetime, on Postgres.
const POSTGRES_SCHEMA: &str = "[{sql: 'SELECT version_num FROM alembic_version'}, {sql: 'SELECT count(*) FROM information_schema.tables WHERE table_schema = \\'public\\' AND table_name LIKE \\'bot_%\\''}, {sql: 'SELECT count(*) FROM information_schema.columns WHERE table_name = \\'bot_behavior\\' AND column_name = \\'lifetime\\''}]";

#[test]
#[ignore = "needs a Postgres server: FLOPPY_PGLITE_SERVER or FLOPPY_POSTGRES_BIN"]
fn postgres_upgrade_and_downgrade_walk_the_revisions_and_keep_alembic_version() {
    let server = Pglite::start().expect("FLOPPY_PGLITE_SERVER or FLOPPY_POSTGRES_BIN is set");
    let database = server.database();
    let directory = unique_scratch("migration");
    let migrate = |target: &str| {
        run(
            &directory,
            &harness_on(&database, "pgsql", "postgresql", target, POSTGRES_SCHEMA),
        )
    };
    assert_eq!(
        migrate("'head'"),
        pairs(&[
            ("output", r#"{"from":null,"to":"0002"}"#),
            ("inspect", r#"[[["0002"]],[[4]],[[1]]]"#),
        ])
    );
    assert_eq!(
        migrate("'-1'"),
        pairs(&[
            ("output", r#"{"from":"0002","to":"0001"}"#),
            ("inspect", r#"[[["0001"]],[[4]],[[0]]]"#),
        ])
    );
    assert_eq!(
        migrate("'base'"),
        pairs(&[
            ("output", r#"{"from":"0001","to":null}"#),
            ("inspect", "[[],[[0]],[[0]]]"),
        ])
    );
    assert_eq!(
        migrate("'+2'"),
        pairs(&[
            ("output", r#"{"from":null,"to":"0002"}"#),
            ("inspect", r#"[[["0002"]],[[4]],[[1]]]"#),
        ])
    );
}
