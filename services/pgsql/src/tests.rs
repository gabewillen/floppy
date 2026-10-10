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

const URI: &str = "postgresql://postgres@127.0.0.1:5432/postgres";

#[test]
fn placeholders_are_numbered_outside_literals_identifiers_and_comments() {
    assert_eq!(
        numbered("SELECT * FROM t WHERE a = ? AND b = ?"),
        "SELECT * FROM t WHERE a = $1 AND b = $2"
    );
    assert_eq!(
        numbered("SELECT '?', 'it''s ?', ? FROM \"we?ird\" -- ?\nWHERE x = ? /* ? /* ? */ ? */"),
        "SELECT '?', 'it''s ?', $1 FROM \"we?ird\" -- ?\nWHERE x = $2 /* ? /* ? */ ? */"
    );
    assert_eq!(
        numbered("SELECT E'\\'?', $$ ? $$, $tag$ ? $tag$, ?"),
        "SELECT E'\\'?', $$ ? $$, $tag$ ? $tag$, $1"
    );
    assert_eq!(
        numbered("SELECT ?1, ?2, ?1, ?, ?10"),
        "SELECT $1, $2, $1, $3, $10"
    );
    assert_eq!(numbered("SELECT 'unterminated ?"), "SELECT 'unterminated ?");
}

#[test]
fn statements_that_end_or_start_a_transaction_are_recognized() {
    for sql in [
        "BEGIN",
        "begin transaction",
        "START TRANSACTION",
        "COMMIT",
        "END",
        "ABORT",
        "ROLLBACK",
        "  -- a comment\n/* another */ commit",
        "PREPARE TRANSACTION 'x'",
    ] {
        assert!(controls_transaction(sql), "{sql}");
    }
    for sql in [
        "SELECT 1",
        "SAVEPOINT a",
        "ROLLBACK TO SAVEPOINT a",
        "RELEASE a",
        "PREPARE p AS SELECT 1",
        "INSERT INTO beginnings VALUES (1)",
    ] {
        assert!(!controls_transaction(sql), "{sql}");
    }
}

#[test]
fn the_connection_uri_names_user_address_and_database() {
    let transaction = parse(&[
        ("database", text(URI)),
        ("statements", array(&serde_json::json!([{"sql": "SELECT ?", "parameters": [true, 2.0, 1.5, null, "x"]}]))),
    ])
    .expect("well-formed");
    assert_eq!(
        transaction.database,
        Database {
            address: "127.0.0.1:5432".parse().expect("an address"),
            user: "postgres".into(),
            name: "postgres".into(),
        }
    );
    assert_eq!(transaction.statements[0].sql, "SELECT $1");
    assert_eq!(
        transaction.statements[0].parameters,
        [
            Parameter::Integer(1),
            Parameter::Integer(2),
            Parameter::Real(1.5),
            Parameter::Null,
            Parameter::Text("x".into()),
        ]
    );
    let v6 = parse_database("postgres://u@[::1]:5433/db").expect("an IPv6 URI");
    assert_eq!(v6.address, "[::1]:5433".parse().expect("an address"));
}

#[test]
fn malformed_parameters_are_refused() {
    let statements = array(&serde_json::json!([{"sql": "SELECT 1"}]));
    for (database, expected) in [
        (
            "/memory/floppy.sqlite",
            "pgsql database must be postgresql://USER@IP:PORT/DBNAME",
        ),
        (
            "postgresql://u:secret@127.0.0.1:5432/db",
            "pgsql database names no password",
        ),
        (
            "postgresql://u@localhost:5432/db",
            "pgsql database must be postgresql://USER@IP:PORT/DBNAME",
        ),
        (
            "postgresql://127.0.0.1:5432/db",
            "pgsql database must be postgresql://USER@IP:PORT/DBNAME",
        ),
        (
            "postgresql://u@127.0.0.1:5432/",
            "pgsql database must be postgresql://USER@IP:PORT/DBNAME",
        ),
    ] {
        assert_eq!(
            parse(&[
                ("database", text(database)),
                ("statements", statements.clone())
            ]),
            Err(expected),
            "{database}"
        );
    }
    assert_eq!(
        parse(&[
            ("database", text(URI)),
            ("statements", array(&serde_json::json!([])))
        ]),
        Err("pgsql statements must hold at least one statement")
    );
    assert_eq!(
        parse(&[
            ("database", text(URI)),
            ("statements", statements.clone()),
            ("engine", text("postgres")),
        ]),
        Err("pgsql takes no parameter of that name")
    );
}

#[test]
fn cells_are_typed_like_the_sql_service() {
    assert_eq!(cell("t", 16), Json::Bool(true));
    assert_eq!(cell("\\x00ff", 17), Json::String("00ff".into()));
    assert_eq!(cell("42", 20), Json::from(42));
    assert_eq!(cell("1.5", 701), serde_json::json!(1.5));
    assert_eq!(cell("NaN", 1700), Json::String("NaN".into()));
    assert_eq!(cell("hello", 25), Json::String("hello".into()));
}

/// Against a running server (`FLOPPY_PGLITE_ADDRESS`): a transaction
/// commits, a failing one commits nothing, and transaction control is
/// refused before it is sent.
#[test]
#[ignore = "needs a running PGlite server: FLOPPY_PGLITE_ADDRESS"]
fn transactions_commit_or_roll_back_on_a_server() {
    let address = std::env::var("FLOPPY_PGLITE_ADDRESS").expect("FLOPPY_PGLITE_ADDRESS is set");
    let uri = format!("postgresql://postgres@{address}/postgres");
    let table = format!("pgsql_unit_{}", std::process::id());
    let apply = |statements: Json| {
        let transaction = parse(&[("database", text(&uri)), ("statements", array(&statements))])
            .expect("well-formed");
        let returned = run(&transaction);
        (
            returned.name,
            serde_json::from_str::<Json>(&returned.data).expect("JSON"),
        )
    };
    let (name, data) = apply(serde_json::json!([
        {"sql": format!("CREATE TABLE {table} (id TEXT PRIMARY KEY, n INTEGER, at TIMESTAMPTZ DEFAULT NOW())")},
        {"sql": format!("INSERT INTO {table} (id, n) VALUES (?, ?)"), "parameters": ["a", 1]},
        {"sql": format!("SELECT id, n FROM {table} WHERE id = ?"), "parameters": ["a"]},
    ]));
    assert_eq!(name, RESULT_EVENT, "{data}");
    assert_eq!(
        data,
        serde_json::json!({"results": [
            {"rowcount": 0, "rows": []},
            {"rowcount": 1, "rows": []},
            {"rowcount": 1, "rows": [{"columns": ["id", "n"], "values": ["a", 1]}]},
        ]})
    );
    let (name, data) = apply(serde_json::json!([
        {"sql": format!("INSERT INTO {table} (id, n) VALUES ('b', 2)")},
        {"sql": format!("INSERT INTO {table} (id, n) VALUES ('a', 3)")},
    ]));
    assert_eq!(name, FAILED_EVENT);
    assert!(
        data["message"]
            .as_str()
            .is_some_and(|message| message.contains("duplicate key")),
        "{data}"
    );
    let (name, data) = apply(serde_json::json!([{"sql": "COMMIT"}]));
    assert_eq!(name, FAILED_EVENT);
    assert_eq!(
        data["message"],
        "a pgsql statement may not end or start a transaction"
    );
    let (name, data) = apply(serde_json::json!([
        {"sql": format!("SELECT count(*) AS n FROM {table}")},
        {"sql": format!("DROP TABLE {table}")},
    ]));
    assert_eq!(name, RESULT_EVENT, "{data}");
    assert_eq!(
        data["results"][0]["rows"][0]["values"],
        serde_json::json!([1])
    );
}
