use super::*;

fn text(value: &str) -> Data {
    Data::String(value.into())
}

fn array(value: &Json) -> Data {
    Data::JsonArray(canonical_json(value))
}

fn parse(parameters: &[(&str, Data)]) -> Result<Transaction, &'static str> {
    parse_parameters(parameters.iter().map(|(name, value)| (*name, value)))
}

/// A database file in a fresh scratch directory under the target directory.
fn database(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("floppy-sql-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch directory is writable");
    dir.join("floppy.sqlite")
        .to_str()
        .expect("the scratch path is UTF-8")
        .to_owned()
}

fn transaction(database: &str, statements: &Json) -> Transaction {
    parse(&[
        ("database", text(database)),
        ("statements", array(statements)),
    ])
    .expect("the statements are well-formed")
}

fn data(returned: &Returned) -> Json {
    serde_json::from_str(&returned.data).expect("the returned data is JSON")
}

/// The table the tests use (mosfet's `bot_memory`, abridged).
const CREATE: &str = "CREATE TABLE IF NOT EXISTS bot_memory (memory_id TEXT PRIMARY KEY, scope TEXT DEFAULT 'memory' NOT NULL, subject_ref TEXT, kind TEXT, content TEXT NOT NULL, content_format TEXT DEFAULT 'text/plain' NOT NULL, metadata_json TEXT DEFAULT '{}' NOT NULL, query_tags TEXT, created_at TEXT DEFAULT (CURRENT_TIMESTAMP) NOT NULL)";

/// A database with [`CREATE`]'s table.
fn created(name: &str) -> String {
    let database = database(name);
    let made = run(&transaction(
        &database,
        &serde_json::json!([{"sql": CREATE}]),
    ));
    assert_eq!(made.name, RESULT_EVENT, "{}", made.data);
    database
}

const INSERT_NAME: &str = "INSERT INTO bot_memory (memory_id, scope, subject_ref, kind, content, content_format, query_tags, created_at) VALUES (?, 'long_term', 'self', 'fact', ?, 'application/json', 'self_name', ?)";
const SELECT_NAMES: &str = "SELECT content FROM bot_memory WHERE query_tags = ? AND subject_ref = ? ORDER BY created_at LIMIT 50";

#[test]
fn the_parameters_are_the_database_and_mosfets_statements() {
    let parsed = parse(&[
        ("database", text("/memory/floppy.sqlite")),
        (
            "statements",
            array(&serde_json::json!([
                {"sql": "SELECT ?, ?, ?, ?, ?, ?", "parameters": ["a", 1, 2.5, true, null, 3.0]},
                {"sql": "SELECT 1", "rowcount": 1}
            ])),
        ),
    ])
    .expect("well-formed parameters");
    assert_eq!(parsed.database, "/memory/floppy.sqlite");
    assert_eq!(
        parsed.statements,
        vec![
            Statement {
                sql: "SELECT ?, ?, ?, ?, ?, ?".into(),
                parameters: vec![
                    Parameter::Text("a".into()),
                    Parameter::Integer(1),
                    Parameter::Real(2.5),
                    Parameter::Integer(1),
                    Parameter::Null,
                    Parameter::Integer(3),
                ],
                rowcount: None,
            },
            Statement {
                sql: "SELECT 1".into(),
                parameters: vec![],
                rowcount: Some(1),
            },
        ]
    );
}

#[test]
fn malformed_parameters_reject_the_start() {
    let good = array(&serde_json::json!([{"sql": "SELECT 1"}]));
    for (parameters, detail) in [
        (
            vec![("statements", good.clone())],
            "sql requires a database parameter",
        ),
        (
            vec![("database", text("/m.sqlite"))],
            "sql requires a statements parameter",
        ),
        (
            vec![("database", text("m.sqlite")), ("statements", good.clone())],
            "sql database must be an absolute file path within its bound",
        ),
        (
            vec![("database", text("/memory/")), ("statements", good.clone())],
            "sql database must be an absolute file path within its bound",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", array(&serde_json::json!([]))),
            ],
            "sql statements must hold at least one statement",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", text("SELECT 1")),
            ],
            "sql statements must be a JSON array of statements",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                (
                    "statements",
                    array(&serde_json::json!([{"sql": "SELECT 1", "extra": 1}])),
                ),
            ],
            "sql statement takes only sql, parameters and rowcount",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", array(&serde_json::json!([{"sql": " "}]))),
            ],
            "sql statement sql must be a non-empty string",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                (
                    "statements",
                    array(&serde_json::json!([{"sql": "SELECT ?", "parameters": [[1]]}])),
                ),
            ],
            "sql statement parameters must be strings, numbers, booleans, or nulls",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", good.clone()),
                ("statements", good.clone()),
            ],
            "sql takes each parameter once",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", good.clone()),
                ("model", text("x")),
            ],
            "sql takes no parameter of that name",
        ),
    ] {
        assert_eq!(parse(&parameters), Err(detail));
    }
}

#[test]
fn an_insert_commits_and_a_later_invocation_recalls_it() {
    let database = created("recall");
    let inserted = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": INSERT_NAME, "parameters": ["n1", "{\"text\":\"Floppy\",\"kind\":\"fact\"}", "2026-10-09T00:00:00.000001+00:00"]},
            {"sql": INSERT_NAME, "parameters": ["n2", "{\"text\":\"Pip\",\"kind\":\"fact\"}", "2026-10-09T00:00:00.000002+00:00"]}
        ]),
    ));
    assert_eq!(inserted.name, RESULT_EVENT);
    assert_eq!(
        data(&inserted),
        serde_json::json!({"results": [{"rowcount": 1, "rows": []}, {"rowcount": 1, "rows": []}]})
    );
    let recalled = run(&transaction(
        &database,
        &serde_json::json!([{"sql": SELECT_NAMES, "parameters": ["self_name", "self"]}]),
    ));
    assert_eq!(recalled.name, RESULT_EVENT);
    assert_eq!(
        recalled.data,
        r#"{"results":[{"rowcount":2,"rows":[{"columns":["content"],"values":["{\"text\":\"Floppy\",\"kind\":\"fact\"}"]},{"columns":["content"],"values":["{\"text\":\"Pip\",\"kind\":\"fact\"}"]}]}]}"#
    );
}

#[test]
fn any_failing_statement_rolls_back_the_whole_transaction() {
    let database = created("rollback");
    let failed = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": INSERT_NAME, "parameters": ["n1", "{}", "t1"]},
            {"sql": INSERT_NAME, "parameters": ["n1", "{}", "t2"]}
        ]),
    ));
    assert_eq!(failed.name, FAILED_EVENT);
    assert_eq!(
        data(&failed),
        serde_json::json!({"code": "SQLITE_CONSTRAINT", "message": "UNIQUE constraint failed: bot_memory.memory_id"})
    );
    let counted = run(&transaction(
        &database,
        &serde_json::json!([{"sql": "SELECT count(*) AS n FROM bot_memory"}]),
    ));
    assert_eq!(
        data(&counted),
        serde_json::json!({"results": [{"rowcount": 1, "rows": [{"columns": ["n"], "values": [0]}]}]})
    );
}

#[test]
fn a_statement_cannot_end_the_transaction_itself() {
    let database = created("commit");
    let failed = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": INSERT_NAME, "parameters": ["n1", "{}", "t1"]},
            {"sql": "COMMIT"}
        ]),
    ));
    assert_eq!(failed.name, FAILED_EVENT);
    let counted = run(&transaction(
        &database,
        &serde_json::json!([{"sql": "SELECT count(*) FROM bot_memory"}]),
    ));
    assert_eq!(
        data(&counted)["results"][0]["rows"][0]["values"],
        serde_json::json!([0])
    );
}

#[test]
fn one_sql_text_holds_exactly_one_statement() {
    let database = database("multiple");
    let failed = run(&transaction(
        &database,
        &serde_json::json!([{"sql": "SELECT 1; SELECT 2"}]),
    ));
    assert_eq!(failed.name, FAILED_EVENT);
}

#[test]
fn cells_are_their_sqlite_values_as_json() {
    let database = database("cells");
    let selected = run(&transaction(
        &database,
        &serde_json::json!([{"sql": "SELECT NULL AS a, 7 AS b, 2.5 AS c, 'x' AS d, x'00ff' AS e"}]),
    ));
    assert_eq!(
        data(&selected),
        serde_json::json!({"results": [{"rowcount": 1, "rows": [{
            "columns": ["a", "b", "c", "d", "e"],
            "values": [null, 7, 2.5, "x", "00ff"]
        }]}]})
    );
}

#[test]
fn a_missing_directory_fails_without_creating_it() {
    let failed = run(&transaction(
        "/nonexistent-floppy-sql/floppy.sqlite",
        &serde_json::json!([{"sql": "SELECT 1"}]),
    ));
    assert_eq!(failed.name, FAILED_EVENT);
}

#[test]
fn defaults_fill_what_a_statement_leaves_out() {
    let database = created("defaults");
    let selected = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": "INSERT INTO bot_memory (memory_id, content) VALUES (lower(hex(randomblob(16))), ?)", "parameters": ["hello"]},
            {"sql": "SELECT scope, content_format, metadata_json, length(memory_id), created_at IS NOT NULL FROM bot_memory"}
        ]),
    ));
    assert_eq!(
        data(&selected)["results"][1]["rows"][0]["values"],
        serde_json::json!(["memory", "text/plain", "{}", 32, 1])
    );
}

#[test]
fn a_failure_message_is_bounded_on_a_character_boundary() {
    let long = "é".repeat(MAX_MESSAGE_BYTES);
    let failed = Returned::failed(&Failed {
        code: "bound",
        message: long,
    });
    let message = data(&failed)["message"]
        .as_str()
        .expect("a message")
        .to_owned();
    assert!(message.len() <= MAX_MESSAGE_BYTES);
    assert!(message.chars().all(|character| character == 'é'));
}

#[test]
fn a_statement_whose_rowcount_differs_fails_the_transaction() {
    let database = created("guard");
    let failed = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": INSERT_NAME, "parameters": ["n1", "{}", "t1"]},
            {"sql": "UPDATE bot_memory SET content = 'x' WHERE memory_id = 'nope'", "rowcount": 1}
        ]),
    ));
    assert_eq!(
        data(&failed),
        serde_json::json!({"code": "rowcount", "message": "a statement's rowcount is not the one it expects"})
    );
    let guarded = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": INSERT_NAME, "parameters": ["n1", "{}", "t1"], "rowcount": 1},
            {"sql": "SELECT count(*) FROM bot_memory", "rowcount": 1}
        ]),
    ));
    assert_eq!(guarded.name, RESULT_EVENT, "{}", guarded.data);
}

fn object(value: &Json) -> Data {
    Data::JsonObject(canonical_json(value))
}

fn execute(open: &mut Open, statements: &Json) -> (Returned, bool) {
    open.receive(
        EXECUTE_EVENT,
        &object(&serde_json::json!({ "statements": statements })),
    )
}

fn count(database: &str) -> Json {
    let counted = run(&transaction(
        database,
        &serde_json::json!([{"sql": "SELECT count(*) AS n FROM bot_memory"}]),
    ));
    data(&counted)["results"][0]["rows"][0]["values"][0].clone()
}

#[test]
fn the_transaction_parameter_opens_it_and_makes_statements_optional() {
    let parsed = parse(&[
        ("database", text("/memory/floppy.sqlite")),
        ("transaction", text("open")),
    ])
    .expect("an open transaction needs no statements");
    assert!(parsed.open);
    assert!(parsed.statements.is_empty());
    assert_eq!(
        parse(&[
            ("database", text("/memory/floppy.sqlite")),
            ("transaction", text("immediate")),
        ]),
        Err("sql transaction must be open")
    );
    assert_eq!(
        parse(&[("database", text("/memory/floppy.sqlite"))]),
        Err("sql requires a statements parameter")
    );
}

#[test]
fn an_open_transaction_reads_its_own_writes_and_commits_them() {
    let database = created("open-commit");
    let mut open = Open::new(database.clone());
    let (inserted, ended) = execute(
        &mut open,
        &serde_json::json!([{"sql": INSERT_NAME, "parameters": ["m1", "\"Floppy\"", "2026-10-10T00:00:00"]}]),
    );
    assert!(!ended);
    assert_eq!(inserted.name, RESULT_EVENT, "{}", inserted.data);
    // Its own write is visible to it, and to no one else yet.
    let (selected, ended) = execute(
        &mut open,
        &serde_json::json!([{"sql": SELECT_NAMES, "parameters": ["self_name", "self"]}]),
    );
    assert!(!ended);
    assert_eq!(
        data(&selected)["results"][0]["rows"][0]["values"],
        serde_json::json!(["\"Floppy\""])
    );
    assert_eq!(count(&database), 0);
    let (committed, ended) = open.receive(COMMIT_EVENT, &object(&serde_json::json!({})));
    assert!(ended);
    assert_eq!(
        (committed.name, committed.data.as_str()),
        (COMMITTED_EVENT, "{}")
    );
    assert_eq!(count(&database), 1);
}

#[test]
fn a_rollback_a_failure_or_another_event_commits_nothing() {
    let database = created("open-rollback");
    let insert = serde_json::json!([{"sql": INSERT_NAME, "parameters": ["m1", "x", "t"]}]);
    let mut open = Open::new(database.clone());
    let _ = execute(&mut open, &insert);
    let (rolled, ended) = open.receive(ROLLBACK_EVENT, &Data::Null);
    assert!(ended);
    assert_eq!(rolled.name, ROLLEDBACK_EVENT);
    let mut failing = Open::new(database.clone());
    let _ = execute(&mut failing, &insert);
    let (failed, ended) = execute(&mut failing, &serde_json::json!([{"sql": "SELECT nope"}]));
    assert!(ended);
    assert_eq!(data(&failed)["code"], "SQLITE_ERROR");
    let mut stray = Open::new(database.clone());
    let _ = execute(&mut stray, &insert);
    let (refused, ended) = stray.receive("sql.whatever", &Data::Null);
    assert!(ended);
    assert_eq!(data(&refused)["code"], "event");
    let (malformed, ended) = Open::new(database.clone()).receive(EXECUTE_EVENT, &Data::Null);
    assert!(ended);
    assert_eq!(data(&malformed)["code"], "event");
    assert_eq!(count(&database), 0);
}

#[test]
fn a_database_changed_under_an_open_transaction_fails_it_with_changed() {
    let database = created("open-changed");
    let mut open = Open::new(database.clone());
    let (read, _) = execute(
        &mut open,
        &serde_json::json!([{"sql": "SELECT count(*) FROM bot_memory"}]),
    );
    assert_eq!(read.name, RESULT_EVENT);
    // Another writer commits meanwhile: the count the document decided on
    // no longer holds.
    let other = run(&transaction(
        &database,
        &serde_json::json!([{"sql": INSERT_NAME, "parameters": ["m9", "x", "t"]}]),
    ));
    assert_eq!(other.name, RESULT_EVENT);
    let _ = execute(
        &mut open,
        &serde_json::json!([{"sql": INSERT_NAME, "parameters": ["m1", "x", "t"]}]),
    );
    let (failed, ended) = open.receive(COMMIT_EVENT, &Data::Null);
    assert!(ended);
    assert_eq!(data(&failed)["code"], "changed");
    assert_eq!(count(&database), 1);
}
