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
    let dir = std::env::temp_dir().join(format!("floppy-memory-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch directory is writable");
    dir.join("memory.sqlite")
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
                {"sql": "SELECT 1"}
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
            },
            Statement {
                sql: "SELECT 1".into(),
                parameters: vec![],
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
            "memory requires a database parameter",
        ),
        (
            vec![("database", text("/m.sqlite"))],
            "memory requires a statements parameter",
        ),
        (
            vec![("database", text("m.sqlite")), ("statements", good.clone())],
            "memory database must be an absolute file path within its bound",
        ),
        (
            vec![("database", text("/memory/")), ("statements", good.clone())],
            "memory database must be an absolute file path within its bound",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", array(&serde_json::json!([]))),
            ],
            "memory statements must hold at least one statement",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", text("SELECT 1")),
            ],
            "memory statements must be a JSON array of statements",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                (
                    "statements",
                    array(&serde_json::json!([{"sql": "SELECT 1", "extra": 1}])),
                ),
            ],
            "memory statement takes only sql and parameters",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", array(&serde_json::json!([{"sql": " "}]))),
            ],
            "memory statement sql must be a non-empty string",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                (
                    "statements",
                    array(&serde_json::json!([{"sql": "SELECT ?", "parameters": [[1]]}])),
                ),
            ],
            "memory statement parameters must be strings, numbers, booleans, or nulls",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", good.clone()),
                ("statements", good.clone()),
            ],
            "memory takes each parameter once",
        ),
        (
            vec![
                ("database", text("/m.sqlite")),
                ("statements", good.clone()),
                ("model", text("x")),
            ],
            "memory takes no parameter of that name",
        ),
    ] {
        assert_eq!(parse(&parameters), Err(detail));
    }
}

#[test]
fn an_insert_commits_and_a_later_invocation_recalls_it() {
    let database = database("recall");
    let inserted = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": INSERT_NAME, "parameters": ["n1", "{\"text\":\"Floppy\",\"kind\":\"fact\"}", "2026-10-09T00:00:00.000001+00:00"]},
            {"sql": INSERT_NAME, "parameters": ["n2", "{\"text\":\"Pip\",\"kind\":\"fact\"}", "2026-10-09T00:00:00.000002+00:00"]}
        ]),
    ));
    assert_eq!(inserted.name, OUTPUT_EVENT);
    assert_eq!(
        data(&inserted),
        serde_json::json!({"results": [{"rowcount": 1, "rows": []}, {"rowcount": 1, "rows": []}]})
    );
    let recalled = run(&transaction(
        &database,
        &serde_json::json!([{"sql": SELECT_NAMES, "parameters": ["self_name", "self"]}]),
    ));
    assert_eq!(recalled.name, OUTPUT_EVENT);
    assert_eq!(
        recalled.data,
        r#"{"results":[{"rowcount":2,"rows":[{"columns":["content"],"values":["{\"text\":\"Floppy\",\"kind\":\"fact\"}"]},{"columns":["content"],"values":["{\"text\":\"Pip\",\"kind\":\"fact\"}"]}]}]}"#
    );
}

#[test]
fn any_failing_statement_rolls_back_the_whole_transaction() {
    let database = database("rollback");
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
        serde_json::json!({"message": "UNIQUE constraint failed: bot_memory.memory_id"})
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
    let database = database("commit");
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
fn the_database_is_migrated_to_mosfets_head_schema() {
    let database = database("schema");
    let columns = run(&transaction(
        &database,
        &serde_json::json!([
            {"sql": "SELECT name FROM pragma_table_info('bot_memory') ORDER BY cid"},
            {"sql": "SELECT name FROM pragma_table_info('bot_behavior') ORDER BY cid"},
            {"sql": "SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name"},
            {"sql": "PRAGMA user_version"}
        ]),
    ));
    let names = |index: usize| -> Vec<Json> {
        data(&columns)["results"][index]["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["values"][0].clone())
            .collect()
    };
    assert_eq!(
        names(0),
        serde_json::json!([
            "memory_id",
            "scope",
            "context_ref",
            "subject_ref",
            "kind",
            "sensitivity",
            "retention",
            "content",
            "content_format",
            "query_tags",
            "metadata_json",
            "created_at"
        ])
        .as_array()
        .expect("array")
        .clone()
    );
    assert_eq!(names(1).last(), Some(&Json::from("lifetime")));
    assert_eq!(
        names(2),
        serde_json::json!([
            "bot_behavior_trigger_event_idx",
            "bot_memory_context_idx",
            "bot_memory_query_idx",
            "bot_memory_scope_idx",
            "bot_stm_memory_created_idx",
            "bot_stm_memory_recent_idx"
        ])
        .as_array()
        .expect("array")
        .clone()
    );
    assert_eq!(names(3), vec![Json::from(SCHEMA_VERSION)]);
}

#[test]
fn a_database_from_a_newer_service_is_refused() {
    let database = database("newer");
    let mut connection = Connection::open(&database).expect("the scratch database opens");
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .expect("the version is written");
    drop(connection);
    let refused = run(&transaction(
        &database,
        &serde_json::json!([{"sql": "SELECT 1"}]),
    ));
    assert_eq!(
        data(&refused),
        serde_json::json!({"message": "the memory database's schema version is not one this service knows"})
    );
    connection = Connection::open(&database).expect("the scratch database opens");
    let tables: i64 = connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))
        .expect("the catalog reads");
    assert_eq!(tables, 0, "nothing was migrated");
}

#[test]
fn defaults_fill_what_a_statement_leaves_out() {
    let database = database("defaults");
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
    let failed = Returned::failed(&long);
    let message = data(&failed)["message"]
        .as_str()
        .expect("a message")
        .to_owned();
    assert!(message.len() <= MAX_MESSAGE_BYTES);
    assert!(message.chars().all(|character| character == 'é'));
}
