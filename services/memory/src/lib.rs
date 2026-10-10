//! The `memory` invoked service: mosfet's Memory ability
//! (`mosfet/abilities/memory/store.py`) as a
//! `stateforward:scxml/invoked-service` Wasm component, backed by SQLite.
//!
//! Storage is I/O, so it is a service, never executable content: the
//! invoking session keeps processing events and timers while a transaction
//! runs in another session's component, a `<send delay>` in the invoking
//! state bounds it, and leaving that state cancels it. One invocation is one
//! transaction of ordered, parameterized statements that all commit or all
//! roll back, exactly mosfet's `InputData`; the invoking document owns its
//! SQL, as mosfet's domain code owns its `SQLAlchemy` clauses. Documents name
//! [`SERVICE_URI`]:
//!
//! ```xml
//! <invoke idlocation="mem" type="https://github.com/stateforward/floppy/services/memory">
//!   <param name="database" expr="'/memory/floppy.sqlite'"/>
//!   <param name="statements" expr="[{sql: recall_sql, parameters: ['self_name']}]"/>
//! </invoke>
//! ```
//!
//! # Parameters (`<param>`)
//!
//! * `database` (required): the database file's absolute guest path, a
//!   non-empty string that starts with `/`, does not end with `/`, and holds
//!   no NUL. Its directory is one the run grants with
//!   `--dir https://github.com/stateforward/floppy/services/memory=HOST::GUEST`;
//!   the file is created on first use.
//! * `statements` (required): a non-empty JSON array of objects with `sql`
//!   (a non-empty string holding exactly one SQL statement with positional
//!   `?` placeholders) and optionally `parameters` (an array of its bind
//!   values: strings, finite numbers, booleans, and nulls; an integral
//!   number within 64 bits binds as an integer, a boolean as 1 or 0), and
//!   nothing else (mosfet's `Statement`, `extra="forbid"`).
//!
//! Any other parameter, a parameter given twice, or a malformed value
//! rejects the start: the `<invoke>` raises `error.execution` and the
//! database is not opened.
//!
//! # The transaction
//!
//! The service opens the file (read-write, created when missing), brings it
//! to the head schema ([`migrate`]: mosfet's Alembic revisions `0001` and
//! `0002` as `PRAGMA user_version` 1 and 2, the same tables, columns,
//! defaults, and indexes), then runs the statements in order inside one
//! `BEGIN IMMEDIATE` transaction ([`apply`]) and commits. A statement that
//! returns columns yields its rows; any other yields the rows it changed.
//! A statement that would end or widen the transaction (`BEGIN`, `COMMIT`,
//! `ROLLBACK`, `ATTACH`, `DETACH`) is refused before it runs and fails it. On any failure nothing is committed. The connection is closed
//! when the invocation ends: the service keeps no state between
//! invocations, and an invocation cancelled before it commits leaves at
//! most a journal that SQLite rolls back on the next open.
//!
//! SQLite is the amalgamation compiled into the component with SQLite's own
//! WASI port (`SQLITE_WASI`: single-threaded, no extension loading, the
//! `unix-dotfile` locking VFS, no WAL); it reaches the granted directory
//! through `wasi:filesystem` and reads the wall clock for
//! `CURRENT_TIMESTAMP`.
//!
//! # Events
//!
//! `invoke` writes exactly one event to the stream it returns, then
//! resolves its result future with no data (the invoking session receives
//! `done.invoke.<id>` after the event). The names are mosfet's Memory
//! ability's:
//!
//! * [`OUTPUT_EVENT`] `{"results": [{"rowcount": N, "rows": [{"columns":
//!   [...], "values": [...]}]}]}`: one result per statement, in order
//!   (mosfet's `OutputData`). A cell is its SQLite value as JSON (`NULL` as
//!   `null`, an integer or a real as a number, text as a string, a blob as
//!   its lowercase hex text).
//! * [`FAILED_EVENT`] `{"message": M}` (mosfet's `FailureData`): SQLite's
//!   message, or the bound that was exceeded. Messages never carry a value
//!   or a path.
//!
//! The invoking document describes both with `event:schema` on the
//! transitions that take them (`abilities/schemas/`). The service reads no
//! event: it drops the event stream `invoke` receives.
//!
//! # Bounds
//!
//! The statements per transaction ([`MAX_STATEMENTS`]), each statement's SQL
//! bytes ([`MAX_SQL_BYTES`]) and parameters ([`MAX_PARAMETERS`]), the
//! database path ([`MAX_DATABASE_PATH_BYTES`]), the rows returned
//! ([`MAX_ROWS`]), and the encoded result ([`MAX_RESULT_BYTES`], under the
//! session's 96 KiB event bound) are bounded on wasm builds only
//! ([`scxml_types::wasm_bound`]). A result over its bound fails the
//! transaction rather than returning part of it.
//!
//! # Telemetry
//!
//! `invoke` records one observation through scxml.rs's `telemetry` WIT
//! facade (operation `start`: its closed outcome, `ok`, `rejected`, or
//! `failed`, and one closed signal event), and one closed signal event
//! (operation `send`) when it returns its event. No attribute carries the
//! SQL, a value, or the path, so the cardinality is the product of the
//! closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::fmt::Write as _;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::{Value as Cell, ValueRef};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use scxml_event_schema::canonical_json;
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

/// The service's name: the last path segment of its repository service URI
/// before the tag.
pub const SERVICE_NAME: &str = "memory";

/// The exact service type URI documents name: tagless, so it names the
/// repository's latest release (decision D19); the manifest names it too.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/memory";

/// The event that carries a committed transaction's results (mosfet's
/// `Memory.output_event`).
pub const OUTPUT_EVENT: &str = "bot.ability.memory.output";
/// The event that reports a transaction that did not commit (mosfet's
/// `Ability.failed_event`).
pub const FAILED_EVENT: &str = "bot.ability.failed";

/// The head schema version ([`migrate`]).
pub const SCHEMA_VERSION: i64 = 2;

/// Upper bound on the statements of one transaction (wasm builds only).
pub const MAX_STATEMENTS: usize = wasm_bound(64);
/// Upper bound on one statement's SQL bytes (wasm builds only).
pub const MAX_SQL_BYTES: usize = wasm_bound(64 * 1024);
/// Upper bound on one statement's parameters (wasm builds only).
pub const MAX_PARAMETERS: usize = wasm_bound(256);
/// Upper bound on the database path's bytes (wasm builds only).
pub const MAX_DATABASE_PATH_BYTES: usize = wasm_bound(1024);
/// Upper bound on the rows one transaction returns, all statements
/// together (wasm builds only).
pub const MAX_ROWS: usize = wasm_bound(1024);
/// Upper bound on the encoded result's bytes (wasm builds only): under the
/// session's 96 KiB event bound, leaving room for the event's envelope.
pub const MAX_RESULT_BYTES: usize = wasm_bound(88 * 1024);
/// Upper bound on a failure message's bytes.
const MAX_MESSAGE_BYTES: usize = 512;

/// One bind value.
#[derive(Clone, Debug, PartialEq)]
pub enum Parameter {
    /// `NULL`.
    Null,
    /// An integer (also a boolean, as 1 or 0).
    Integer(i64),
    /// A real.
    Real(f64),
    /// Text.
    Text(String),
}

/// One parameterized statement (mosfet's `Statement`).
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// Exactly one SQL statement with positional placeholders.
    pub sql: String,
    /// Its bind values, in order.
    pub parameters: Vec<Parameter>,
}

/// One validated invocation: the database and its transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    /// The database file's absolute guest path.
    pub database: String,
    /// The ordered statements of the one transaction (at least one).
    pub statements: Vec<Statement>,
}

/// The one event an invocation returns: its name and canonical JSON data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Returned {
    /// [`OUTPUT_EVENT`] or [`FAILED_EVENT`].
    pub name: &'static str,
    /// Canonical JSON object text.
    pub data: String,
}

impl Returned {
    /// [`FAILED_EVENT`] with `message`, cut to a bounded length on a
    /// character boundary.
    #[must_use]
    pub fn failed(message: &str) -> Self {
        let mut end = message.len().min(MAX_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let message = message.get(..end).unwrap_or_default();
        Self {
            name: FAILED_EVENT,
            data: canonical_json(&serde_json::json!({ "message": message })),
        }
    }
}

/// Reads one invocation's parameters (see the crate documentation,
/// "Parameters").
///
/// # Errors
///
/// A static detail for an unknown or repeated parameter, a missing or
/// malformed one, or one outside its bound.
pub fn parse_parameters<'a>(
    parameters: impl IntoIterator<Item = (&'a str, &'a Data)>,
) -> Result<Transaction, &'static str> {
    let mut database = None;
    let mut statements = None;
    for (name, value) in parameters {
        let slot = match name {
            "database" => &mut database,
            "statements" => &mut statements,
            _ => return Err("memory takes no parameter of that name"),
        };
        if slot.replace(value).is_some() {
            return Err("memory takes each parameter once");
        }
    }
    let database = match database.ok_or("memory requires a database parameter")? {
        Data::String(path)
            if path.starts_with('/')
                && !path.ends_with('/')
                && !path.contains('\0')
                && !exceeds_wasm_bound(path.len(), MAX_DATABASE_PATH_BYTES) =>
        {
            path.clone()
        }
        _ => return Err("memory database must be an absolute file path within its bound"),
    };
    let statements = match statements.ok_or("memory requires a statements parameter")? {
        Data::JsonArray(text) => statements_from_json(text)?,
        _ => return Err("memory statements must be a JSON array of statements"),
    };
    Ok(Transaction {
        database,
        statements,
    })
}

fn statements_from_json(text: &str) -> Result<Vec<Statement>, &'static str> {
    let malformed = "memory statements must be a JSON array of statements";
    let Ok(Json::Array(items)) = serde_json::from_str::<Json>(text) else {
        return Err(malformed);
    };
    if items.is_empty() {
        return Err("memory statements must hold at least one statement");
    }
    if exceeds_wasm_bound(items.len(), MAX_STATEMENTS) {
        return Err("memory statements exceed their bound");
    }
    items.iter().map(statement_from_json).collect()
}

fn statement_from_json(item: &Json) -> Result<Statement, &'static str> {
    let Json::Object(fields) = item else {
        return Err("memory statement must be an object with sql and parameters");
    };
    if fields.keys().any(|key| key != "sql" && key != "parameters") {
        return Err("memory statement takes only sql and parameters");
    }
    let sql = match fields.get("sql") {
        Some(Json::String(sql)) if !sql.trim().is_empty() => sql.clone(),
        _ => return Err("memory statement sql must be a non-empty string"),
    };
    if exceeds_wasm_bound(sql.len(), MAX_SQL_BYTES) {
        return Err("memory statement sql exceeds its bound");
    }
    let parameters = match fields.get("parameters") {
        None => Vec::new(),
        Some(Json::Array(values)) => {
            if exceeds_wasm_bound(values.len(), MAX_PARAMETERS) {
                return Err("memory statement parameters exceed their bound");
            }
            values
                .iter()
                .map(parameter_from_json)
                .collect::<Result<_, _>>()?
        }
        Some(_) => return Err("memory statement parameters must be an array"),
    };
    Ok(Statement { sql, parameters })
}

fn parameter_from_json(value: &Json) -> Result<Parameter, &'static str> {
    Ok(match value {
        Json::Null => Parameter::Null,
        Json::Bool(value) => Parameter::Integer(i64::from(*value)),
        Json::Number(number) => match number.as_i64() {
            Some(integer) => Parameter::Integer(integer),
            None => match number.as_f64() {
                // An integral real within 64 bits (JSON may spell one as
                // `2.0`) binds as an integer, as the data model meant it.
                Some(real) if real.fract() == 0.0 && real.abs() < 9.007_199_254_740_992e15 =>
                {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "integral and within 2^53, so exact"
                    )]
                    Parameter::Integer(real as i64)
                }
                Some(real) if real.is_finite() => Parameter::Real(real),
                _ => return Err("memory statement parameters must be finite numbers"),
            },
        },
        Json::String(text) => Parameter::Text(text.clone()),
        Json::Array(_) | Json::Object(_) => {
            return Err("memory statement parameters must be strings, numbers, booleans, or nulls");
        }
    })
}

/// mosfet's revision `0001` (the create_all-era schema, SQLite DDL).
const MIGRATION_0001: &str = "
CREATE TABLE bot_memory (
    memory_id TEXT NOT NULL,
    scope TEXT DEFAULT 'memory' NOT NULL,
    context_ref TEXT,
    subject_ref TEXT,
    kind TEXT,
    sensitivity TEXT,
    retention TEXT,
    content TEXT NOT NULL,
    content_format TEXT DEFAULT 'text/plain' NOT NULL,
    query_tags TEXT,
    metadata_json TEXT DEFAULT '{}' NOT NULL,
    created_at TEXT DEFAULT (CURRENT_TIMESTAMP) NOT NULL,
    PRIMARY KEY (memory_id)
);
CREATE INDEX bot_memory_context_idx ON bot_memory (context_ref, subject_ref);
CREATE INDEX bot_memory_query_idx ON bot_memory (query_tags);
CREATE INDEX bot_memory_scope_idx ON bot_memory (scope, created_at);
CREATE TABLE bot_behavior (
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    description TEXT DEFAULT '' NOT NULL,
    examples_json TEXT DEFAULT '[]' NOT NULL,
    status TEXT DEFAULT 'ACTIVE' NOT NULL,
    status_reason TEXT DEFAULT '' NOT NULL,
    status_updated_at TEXT DEFAULT '' NOT NULL,
    used_count TEXT DEFAULT '0' NOT NULL,
    last_used_at TEXT DEFAULT '' NOT NULL,
    failed_count TEXT DEFAULT '0' NOT NULL,
    last_failed_at TEXT DEFAULT '' NOT NULL,
    created_at TEXT DEFAULT (CURRENT_TIMESTAMP) NOT NULL,
    updated_at TEXT DEFAULT (CURRENT_TIMESTAMP) NOT NULL,
    PRIMARY KEY (name)
);
CREATE TABLE bot_behavior_trigger (
    behavior_name TEXT NOT NULL,
    \"trigger\" TEXT NOT NULL,
    PRIMARY KEY (behavior_name, \"trigger\"),
    FOREIGN KEY(behavior_name) REFERENCES bot_behavior (name) ON DELETE CASCADE
);
CREATE INDEX bot_behavior_trigger_event_idx ON bot_behavior_trigger (\"trigger\");
CREATE TABLE bot_stm_memory (
    stm_id TEXT NOT NULL,
    stimulus_name TEXT NOT NULL,
    payload_json TEXT DEFAULT '{}' NOT NULL,
    event_id TEXT,
    source TEXT,
    target TEXT,
    created_at TEXT NOT NULL,
    last_accessed_at TEXT,
    PRIMARY KEY (stm_id)
);
CREATE INDEX bot_stm_memory_recent_idx ON bot_stm_memory (stimulus_name, created_at);
CREATE INDEX bot_stm_memory_created_idx ON bot_stm_memory (created_at);
";

/// mosfet's revision `0002` (behavior lifetime: turn behaviors versus
/// persistent routines).
const MIGRATION_0002: &str =
    "ALTER TABLE bot_behavior ADD COLUMN lifetime TEXT DEFAULT 'turn' NOT NULL;";

/// The revisions, in order: revision `n` brings `user_version` `n - 1` to
/// `n`.
const MIGRATIONS: [&str; 2] = [MIGRATION_0001, MIGRATION_0002];

/// Brings `connection`'s database to [`SCHEMA_VERSION`] in one transaction
/// (`PRAGMA user_version` records the revision).
///
/// # Errors
///
/// SQLite's message, or a refusal of a database whose version is newer than
/// this service's or negative.
pub fn migrate(connection: &mut Connection) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let version: i64 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if !(0..=SCHEMA_VERSION).contains(&version) {
        return Err("the memory database's schema version is not one this service knows".into());
    }
    for (index, migration) in MIGRATIONS.iter().enumerate() {
        let revision = i64::try_from(index).map_err(|error| error.to_string())? + 1;
        if revision <= version {
            continue;
        }
        transaction
            .execute_batch(migration)
            .map_err(|error| error.to_string())?;
        transaction
            .pragma_update(None, "user_version", revision)
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

/// Runs `statements` in one `BEGIN IMMEDIATE` transaction on `connection`
/// and commits, returning mosfet's `OutputData` as JSON; on any failure
/// nothing is committed.
///
/// # Errors
///
/// SQLite's message, or the bound or rule that was broken.
pub fn apply(connection: &mut Connection, statements: &[Statement]) -> Result<Json, String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    // A statement may not end or widen the one transaction: SQLite refuses
    // to prepare `BEGIN`, `COMMIT`, `ROLLBACK`, `ATTACH`, and `DETACH`
    // (savepoints nest inside it and stay allowed). The authorizer is
    // removed again before the commit or the rollback.
    transaction.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Transaction { .. } | AuthAction::Attach { .. } | AuthAction::Detach { .. } => {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }));
    let output = execute(&transaction, statements);
    transaction.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let output = output?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(output)
}

/// Runs each statement in the open transaction and builds the results.
fn execute(connection: &Connection, statements: &[Statement]) -> Result<Json, String> {
    let mut results = Vec::with_capacity(statements.len());
    let mut rows_returned = 0_usize;
    for statement in statements {
        let mut prepared = connection
            .prepare(&statement.sql)
            .map_err(|error| error.to_string())?;
        let parameters = statement
            .parameters
            .iter()
            .map(|parameter| match parameter {
                Parameter::Null => Cell::Null,
                Parameter::Integer(value) => Cell::Integer(*value),
                Parameter::Real(value) => Cell::Real(*value),
                Parameter::Text(value) => Cell::Text(value.clone()),
            });
        let result = if prepared.column_count() > 0 {
            let columns: Vec<Json> = prepared
                .column_names()
                .into_iter()
                .map(|name| Json::String(name.to_owned()))
                .collect();
            let width = columns.len();
            let mut rows = prepared
                .query(rusqlite::params_from_iter(parameters))
                .map_err(|error| error.to_string())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(|error| error.to_string())? {
                rows_returned += 1;
                if exceeds_wasm_bound(rows_returned, MAX_ROWS) {
                    return Err("the memory transaction returns more rows than its bound".into());
                }
                let values = (0..width)
                    .map(|index| {
                        row.get_ref(index)
                            .map_err(|error| error.to_string())
                            .and_then(cell)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                out.push(serde_json::json!({ "columns": columns, "values": values }));
            }
            serde_json::json!({ "rowcount": out.len(), "rows": out })
        } else {
            let changed = prepared
                .execute(rusqlite::params_from_iter(parameters))
                .map_err(|error| error.to_string())?;
            serde_json::json!({ "rowcount": changed, "rows": [] })
        };
        results.push(result);
    }
    let mut output = Map::new();
    output.insert("results".into(), Json::Array(results));
    let output = Json::Object(output);
    if exceeds_wasm_bound(canonical_json(&output).len(), MAX_RESULT_BYTES) {
        return Err("the memory transaction's result exceeds its bound".into());
    }
    Ok(output)
}

/// One cell as JSON.
fn cell(value: ValueRef<'_>) -> Result<Json, String> {
    Ok(match value {
        ValueRef::Null => Json::Null,
        ValueRef::Integer(value) => Json::from(value),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Json::Number)
            .ok_or("a memory cell holds a non-finite real")?,
        ValueRef::Text(bytes) => Json::String(
            std::str::from_utf8(bytes)
                .map_err(|_| "a memory cell holds text that is not UTF-8")?
                .to_owned(),
        ),
        ValueRef::Blob(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                let _ = write!(hex, "{byte:02x}");
            }
            Json::String(hex)
        }
    })
}

/// Opens the transaction's database (read-write, created when missing),
/// migrates it, and applies the transaction: [`OUTPUT_EVENT`] with the
/// results, or [`FAILED_EVENT`] with why nothing was committed.
#[must_use]
pub fn run(transaction: &Transaction) -> Returned {
    let opened = Connection::open_with_flags(
        &transaction.database,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    let mut connection = match opened {
        Ok(connection) => connection,
        Err(error) => return Returned::failed(&error.to_string()),
    };
    let applied =
        migrate(&mut connection).and_then(|()| apply(&mut connection, &transaction.statements));
    match applied {
        Ok(output) => Returned {
            name: OUTPUT_EVENT,
            data: canonical_json(&output),
        },
        Err(message) => Returned::failed(&message),
    }
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "invoked-service",
        path: "../../scxml.rs/crates/scxml-wit/wit/scxml",
        // The world's `wasi:clocks` interfaces too.
        generate_all,
    });

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, InvokeRequest,
    };

    use super::{Returned, Transaction};

    /// The invocation's result: `ok` (no done data), or its error.
    type Done = Result<Option<DataValue>, ComponentError>;

    struct Component;

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    fn native_value(value: DataValue) -> Result<scxml_types::DataValue, ComponentError> {
        let invalid = || error(ErrorCode::InvalidInput, "a parameter value is invalid");
        Ok(match value {
            DataValue::Absent => scxml_types::DataValue::Absent,
            DataValue::Undefined => scxml_types::DataValue::Undefined,
            DataValue::Null => scxml_types::DataValue::Null,
            DataValue::Boolean(value) => scxml_types::DataValue::Bool(value),
            DataValue::Number(value) => scxml_types::DataValue::Number(
                scxml_types::FiniteNumber::new(value).map_err(|_| invalid())?,
            ),
            DataValue::Text(value) => scxml_types::DataValue::String(value),
            DataValue::JsonArray(value) => {
                scxml_types::DataValue::canonical_json_array(&value).map_err(|_| invalid())?
            }
            DataValue::JsonObject(value) => {
                scxml_types::DataValue::canonical_json_object(&value).map_err(|_| invalid())?
            }
            DataValue::Xml(value) => scxml_types::DataValue::Xml(value),
        })
    }

    fn event(reply: &Returned) -> Event {
        Event {
            name: reply.name.to_owned(),
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: DataValue::JsonObject(reply.data.clone()),
            raw: None,
            trace_context: None,
        }
    }

    fn plan(request: InvokeRequest) -> Result<Transaction, ComponentError> {
        if request.type_uri != super::SERVICE_URI {
            return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
        }
        let parameters = request
            .parameters
            .into_iter()
            .map(|entry| Ok((entry.name, native_value(entry.value)?)))
            .collect::<Result<Vec<_>, ComponentError>>()?;
        super::parse_parameters(
            parameters
                .iter()
                .map(|(name, value)| (name.as_str(), value)),
        )
        .map_err(|detail| error(ErrorCode::InvalidInput, detail))
    }

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its task is spawned"
    )]
    impl Guest for Component {
        async fn invoke(
            request: InvokeRequest,
            _events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            // The service accepts no event: its input stream is dropped.
            let transaction = observed(Operation::Start, || plan(request))?;
            let (mut returned, returned_reader) = wit_stream::new::<Event>();
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            wit_bindgen::spawn(async move {
                // SQLite is synchronous: the transaction runs to its commit
                // or rollback before the task next yields.
                let reply = super::run(&transaction);
                telemetry::event(
                    Operation::Send,
                    if reply.name == super::OUTPUT_EVENT {
                        SignalEvent::TransformationCompleted
                    } else {
                        SignalEvent::InputRejected
                    },
                    None,
                );
                // The one event, then the result: `done.invoke.<id>` follows
                // it.
                if returned.write_one(event(&reply)).await.is_some() {
                    return;
                }
                drop(returned);
                let _ = done.write(Ok(None)).await;
            });
            Ok((returned_reader, done_reader))
        }
    }

    /// One call's closed telemetry: its outcome and one event (no attribute
    /// carries input).
    fn observed<T>(
        operation: Operation,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(failure) if failure.code == ErrorCode::InvalidInput => Outcome::Rejected,
            Err(_) => Outcome::Failed,
        };
        telemetry::observe(operation, outcome, None, None);
        telemetry::event(
            operation,
            if result.is_ok() {
                SignalEvent::TransformationCompleted
            } else {
                SignalEvent::InputRejected
            },
            None,
        );
        result
    }

    export!(Component);
}

#[cfg(test)]
mod tests;
