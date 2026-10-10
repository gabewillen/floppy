//! The `sql` invoked service: one SQLite transaction of parameterized
//! statements per `<invoke>`, as a `stateforward:scxml/invoked-service` Wasm
//! component. It is the engine under mosfet's Memory ability
//! (`mosfet/abilities/memory/store.py` on `providers/sqlite_memory`), which
//! floppy runs as `abilities/memory.scx`; it knows no schema of its own.
//!
//! Storage is I/O, so it is a service, never executable content: the
//! invoking session keeps processing events and timers while a transaction
//! runs in another session's component, a `<send delay>` in the invoking
//! state bounds it, and leaving that state cancels it. One invocation is one
//! transaction of ordered, parameterized statements that all commit or all
//! roll back, mosfet's `InputData`; the invoking document owns its SQL.
//! Documents name [`SERVICE_URI`]:
//!
//! ```xml
//! <invoke id="sql" type="https://github.com/stateforward/floppy/services/sql">
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
//!   `--dir https://github.com/stateforward/floppy/services/sql=HOST::GUEST`;
//!   the file is created on first use.
//! * `statements` (required): a non-empty JSON array of objects with `sql`
//!   (a non-empty string holding exactly one SQL statement with positional
//!   `?` placeholders) and optionally `parameters` (an array of its bind
//!   values: strings, finite numbers, booleans, and nulls; an integral
//!   number within 64 bits binds as an integer, a boolean as 1 or 0), and
//!   optionally `rowcount` (a non-negative integer: the rowcount the
//!   statement must have, else the whole transaction fails with code
//!   `rowcount`; a guard such as a version row that must still be the one
//!   read), and nothing else (mosfet's `Statement`, `extra="forbid"`, with
//!   its `StatementResult.rowcount` as the guard).
//!
//! * `transaction` (optional): `open` keeps the transaction open across
//!   events (see "Open transactions"); `statements` is then optional, and
//!   any given run at once. Without it the invocation is one batch.
//!
//! Any other parameter, a parameter given twice, or a malformed value
//! rejects the start: the `<invoke>` raises `error.execution` and the
//! database is not opened.
//!
//! # The transaction
//!
//! The service opens the file (read-write, created when missing) and runs
//! the statements in order inside one `BEGIN IMMEDIATE` transaction
//! ([`apply`]) and commits. A statement that returns columns yields its
//! rows; any other yields the rows it changed. A statement that would end
//! or widen the transaction (`BEGIN`, `COMMIT`, `ROLLBACK`, `ATTACH`,
//! `DETACH`) is refused before it runs and fails it. On any failure nothing
//! is committed. The connection is closed when the invocation ends: the
//! service keeps no state between invocations, and an invocation cancelled
//! before it commits leaves at most a journal that SQLite rolls back on the
//! next open.
//!
//! SQLite is the amalgamation compiled into the component with SQLite's own
//! WASI port (`SQLITE_WASI`: single-threaded, no extension loading, the
//! `unix-dotfile` locking VFS, no WAL); it reaches the granted directory
//! through `wasi:filesystem` and reads the wall clock for
//! `CURRENT_TIMESTAMP`. It has no entropy source (no `/dev/urandom`, and
//! the component imports no `wasi:random`), so SQLite seeds its PRNG from
//! the time in seconds: `random()` and `randomblob()` repeat across
//! invocations in the same second and are not identifiers. A statement that
//! needs a fresh key derives it inside its transaction (for example from
//! `max(rowid)`).
//!
//! # Events
//!
//! A batch's `invoke` writes exactly one event to the stream it returns, then
//! resolves its result future with no data (the invoking session receives
//! `done.invoke.<id>` after the event), named like floppy's other services
//! (`generate.result`, `generate.failed`):
//!
//! * [`RESULT_EVENT`] `{"results": [{"rowcount": N, "rows": [{"columns":
//!   [...], "values": [...]}]}]}`: one result per statement, in order
//!   (mosfet's `OutputData`). A cell is its SQLite value as JSON (`NULL` as
//!   `null`, an integer or a real as a number, text as a string, a blob as
//!   its lowercase hex text).
//! * [`FAILED_EVENT`] `{"code": C, "message": M}` (mosfet's `FailureData`
//!   and a code a document can branch on): `code` is SQLite's primary
//!   result-code name (`SQLITE_CONSTRAINT`, `SQLITE_BUSY`, ...), `rowcount`
//!   (a statement's expected rowcount differed), or `bound`; `message` is
//!   SQLite's message or the rule that was broken. Messages never carry a
//!   bound value or a path.
//!
//! The invoking document describes both with `event:schema` on the
//! transitions that take them (`abilities/schemas/`). A batch reads no
//! event: it drops the event stream `invoke` receives. `code` is also
//! `changed` or `event` in an open transaction (below).
//!
//! # Open transactions
//!
//! With `transaction` `open`, one invocation is one transaction over several
//! exchanges, for work whose next statement depends on a decision the
//! document makes in between. The invoking session sends it events
//! (`<send target="#_<invokeid>">`):
//!
//! * [`EXECUTE_EVENT`] `{statements}` runs them after the ones before and
//!   answers [`RESULT_EVENT`] with their results; the transaction stays
//!   open.
//! * [`COMMIT_EVENT`] commits and answers [`COMMITTED_EVENT`] `{}`.
//! * [`ROLLBACK_EVENT`] answers [`ROLLEDBACK_EVENT`] `{}` and commits
//!   nothing.
//!
//! Any failure answers [`FAILED_EVENT`] and commits nothing, as does any
//! other event (code `event`); each of these ends the invocation
//! (`done.invoke.<id>` follows). Leaving the invoking state cancels it, and
//! a cancelled open transaction commits nothing.
//!
//! The service holds no connection and no lock between events (the host
//! drops a cancelled invocation's instance without running it again, so a
//! lock held across events could outlive it). It keeps the statements it ran
//! with the results it answered: each `sql.execute` replays them and runs
//! its own in one `IMMEDIATE` transaction that it rolls back, and
//! `sql.commit` replays them all in one that it commits. A replayed
//! statement whose result differs from the one answered (another writer
//! changed the database meanwhile) fails the transaction with code
//! `changed`: optimistic concurrency, serializable as seen by the document,
//! which decides whether to start over. A statement whose result depends on
//! the clock or on randomness fails the replay the same way, so an open
//! transaction reads neither. The statements of one open transaction, and
//! the rows and result bytes of each replay, are bounded as a batch's.
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
pub const SERVICE_NAME: &str = "sql";

/// The exact service type URI documents name: tagless, so it names the
/// repository's latest release (decision D19); the manifest names it too.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/sql";

/// The event that carries a committed transaction's results (mosfet's
/// `OutputData`).
pub const RESULT_EVENT: &str = "sql.result";
/// The event that reports a transaction that did not commit (mosfet's
/// `FailureData`).
pub const FAILED_EVENT: &str = "sql.failed";
/// The event that runs statements in an open transaction.
pub const EXECUTE_EVENT: &str = "sql.execute";
/// The event that commits an open transaction.
pub const COMMIT_EVENT: &str = "sql.commit";
/// The event that rolls an open transaction back.
pub const ROLLBACK_EVENT: &str = "sql.rollback";
/// The event that reports an open transaction committed.
pub const COMMITTED_EVENT: &str = "sql.committed";
/// The event that reports an open transaction rolled back.
pub const ROLLEDBACK_EVENT: &str = "sql.rolledback";

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
    /// The `rowcount` it must have, else the transaction fails (a guard,
    /// for example a version row that must still be the one read).
    pub rowcount: Option<u64>,
}

/// Why a transaction did not commit: a closed code and a message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failed {
    /// SQLite's primary result-code name (`SQLITE_CONSTRAINT`,
    /// `SQLITE_BUSY`, ...), or `rowcount` (a statement's expected rowcount
    /// differed), or `bound` (a bound was exceeded).
    pub code: &'static str,
    /// SQLite's message, or the rule that was broken.
    pub message: String,
}

impl Failed {
    fn bound(message: &str) -> Self {
        Self {
            code: "bound",
            message: message.into(),
        }
    }
}

impl From<rusqlite::Error> for Failed {
    fn from(error: rusqlite::Error) -> Self {
        let code = match &error {
            rusqlite::Error::SqliteFailure(failure, _) => sqlite_code(failure.code),
            _ => "SQLITE_ERROR",
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

/// SQLite's name of a primary result code.
const fn sqlite_code(code: rusqlite::ErrorCode) -> &'static str {
    use rusqlite::ErrorCode as Code;
    match code {
        Code::InternalMalfunction => "SQLITE_INTERNAL",
        Code::PermissionDenied => "SQLITE_PERM",
        Code::OperationAborted => "SQLITE_ABORT",
        Code::DatabaseBusy => "SQLITE_BUSY",
        Code::DatabaseLocked => "SQLITE_LOCKED",
        Code::OutOfMemory => "SQLITE_NOMEM",
        Code::ReadOnly => "SQLITE_READONLY",
        Code::OperationInterrupted => "SQLITE_INTERRUPT",
        Code::SystemIoFailure => "SQLITE_IOERR",
        Code::DatabaseCorrupt => "SQLITE_CORRUPT",
        Code::NotFound => "SQLITE_NOTFOUND",
        Code::DiskFull => "SQLITE_FULL",
        Code::CannotOpen => "SQLITE_CANTOPEN",
        Code::FileLockingProtocolFailed => "SQLITE_PROTOCOL",
        Code::SchemaChanged => "SQLITE_SCHEMA",
        Code::TooBig => "SQLITE_TOOBIG",
        Code::ConstraintViolation => "SQLITE_CONSTRAINT",
        Code::TypeMismatch => "SQLITE_MISMATCH",
        Code::ApiMisuse => "SQLITE_MISUSE",
        Code::NoLargeFileSupport => "SQLITE_NOLFS",
        Code::AuthorizationForStatementDenied => "SQLITE_AUTH",
        Code::ParameterOutOfRange => "SQLITE_RANGE",
        Code::NotADatabase => "SQLITE_NOTADB",
        _ => "SQLITE_ERROR",
    }
}

/// One validated invocation: the database and its transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    /// The database file's absolute guest path.
    pub database: String,
    /// The ordered statements of the one transaction (at least one in a
    /// batch; any number, run at once, in an open transaction).
    pub statements: Vec<Statement>,
    /// Whether the transaction stays open for `sql.execute`, `sql.commit`
    /// and `sql.rollback` (the `transaction` parameter `open`).
    pub open: bool,
}

/// The one event an invocation returns: its name and canonical JSON data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Returned {
    /// [`RESULT_EVENT`] or [`FAILED_EVENT`].
    pub name: &'static str,
    /// Canonical JSON object text.
    pub data: String,
}

impl Returned {
    /// `name` with the empty object.
    #[must_use]
    pub fn empty(name: &'static str) -> Self {
        Self {
            name,
            data: "{}".into(),
        }
    }

    /// [`FAILED_EVENT`] with the failure's code and its message, cut to a
    /// bounded length on a character boundary.
    #[must_use]
    pub fn failed(failed: &Failed) -> Self {
        let message = &failed.message;
        let mut end = message.len().min(MAX_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let message = message.get(..end).unwrap_or_default();
        Self {
            name: FAILED_EVENT,
            data: canonical_json(&serde_json::json!({ "code": failed.code, "message": message })),
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
    let mut transaction = None;
    for (name, value) in parameters {
        let slot = match name {
            "database" => &mut database,
            "statements" => &mut statements,
            "transaction" => &mut transaction,
            _ => return Err("sql takes no parameter of that name"),
        };
        if slot.replace(value).is_some() {
            return Err("sql takes each parameter once");
        }
    }
    let database = match database.ok_or("sql requires a database parameter")? {
        Data::String(path)
            if path.starts_with('/')
                && !path.ends_with('/')
                && !path.contains('\0')
                && !exceeds_wasm_bound(path.len(), MAX_DATABASE_PATH_BYTES) =>
        {
            path.clone()
        }
        _ => return Err("sql database must be an absolute file path within its bound"),
    };
    let open = match transaction {
        None => false,
        Some(Data::String(mode)) if mode == "open" => true,
        Some(_) => return Err("sql transaction must be open"),
    };
    let statements = match (statements, open) {
        (Some(Data::JsonArray(text)), _) => statements_from_json(text)?,
        (Some(_), _) => return Err("sql statements must be a JSON array of statements"),
        (None, true) => Vec::new(),
        (None, false) => return Err("sql requires a statements parameter"),
    };
    Ok(Transaction {
        database,
        statements,
        open,
    })
}

fn statements_from_json(text: &str) -> Result<Vec<Statement>, &'static str> {
    let malformed = "sql statements must be a JSON array of statements";
    let Ok(Json::Array(items)) = serde_json::from_str::<Json>(text) else {
        return Err(malformed);
    };
    if items.is_empty() {
        return Err("sql statements must hold at least one statement");
    }
    if exceeds_wasm_bound(items.len(), MAX_STATEMENTS) {
        return Err("sql statements exceed their bound");
    }
    items.iter().map(statement_from_json).collect()
}

fn statement_from_json(item: &Json) -> Result<Statement, &'static str> {
    let Json::Object(fields) = item else {
        return Err("sql statement must be an object with sql and parameters");
    };
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "sql" | "parameters" | "rowcount"))
    {
        return Err("sql statement takes only sql, parameters and rowcount");
    }
    let sql = match fields.get("sql") {
        Some(Json::String(sql)) if !sql.trim().is_empty() => sql.clone(),
        _ => return Err("sql statement sql must be a non-empty string"),
    };
    if exceeds_wasm_bound(sql.len(), MAX_SQL_BYTES) {
        return Err("sql statement sql exceeds its bound");
    }
    let parameters = match fields.get("parameters") {
        None => Vec::new(),
        Some(Json::Array(values)) => {
            if exceeds_wasm_bound(values.len(), MAX_PARAMETERS) {
                return Err("sql statement parameters exceed their bound");
            }
            values
                .iter()
                .map(parameter_from_json)
                .collect::<Result<_, _>>()?
        }
        Some(_) => return Err("sql statement parameters must be an array"),
    };
    let rowcount = match fields.get("rowcount") {
        None | Some(Json::Null) => None,
        Some(Json::Number(number)) => Some(
            number
                .as_u64()
                .or_else(|| {
                    number
                        .as_f64()
                        .filter(|real| {
                            real.fract() == 0.0 && (0.0..9.007_199_254_740_992e15).contains(real)
                        })
                        .map(|real| {
                            #[expect(
                                clippy::cast_possible_truncation,
                                clippy::cast_sign_loss,
                                reason = "integral, non-negative and within 2^53, so exact"
                            )]
                            let whole = real as u64;
                            whole
                        })
                })
                .ok_or("sql statement rowcount must be a non-negative integer")?,
        ),
        Some(_) => return Err("sql statement rowcount must be a non-negative integer"),
    };
    Ok(Statement {
        sql,
        parameters,
        rowcount,
    })
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
                _ => return Err("sql statement parameters must be finite numbers"),
            },
        },
        Json::String(text) => Parameter::Text(text.clone()),
        Json::Array(_) | Json::Object(_) => {
            return Err("sql statement parameters must be strings, numbers, booleans, or nulls");
        }
    })
}

/// Runs `statements` in one `BEGIN IMMEDIATE` transaction on `connection`
/// and commits, returning mosfet's `OutputData` as JSON; on any failure
/// nothing is committed.
///
/// # Errors
///
/// SQLite's code and message, or the bound or rule that was broken.
pub fn apply(connection: &mut Connection, statements: &[Statement]) -> Result<Json, Failed> {
    transact(connection, &[], statements, true)
}

/// Runs `replayed` (statements an open transaction ran before, with the
/// results it answered) and then `statements` in one `BEGIN IMMEDIATE`
/// transaction, commits when `commit` and else rolls back, and returns the
/// results of `statements` alone. A replayed statement whose result differs
/// from the one answered fails with code `changed`.
fn transact(
    connection: &mut Connection,
    replayed: &[(Statement, Json)],
    statements: &[Statement],
    commit: bool,
) -> Result<Json, Failed> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
    let all: Vec<Statement> = replayed
        .iter()
        .map(|(statement, _)| statement.clone())
        .chain(statements.iter().cloned())
        .collect();
    let output = execute(&transaction, &all);
    transaction.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let Json::Array(mut results) = output?
        .get_mut("results")
        .map(Json::take)
        .unwrap_or_default()
    else {
        return Err(Failed::bound("the sql transaction has no results"));
    };
    let fresh = results.split_off(replayed.len());
    if results
        .iter()
        .zip(replayed)
        .any(|(result, (_, answered))| result != answered)
    {
        return Err(Failed {
            code: "changed",
            message: "the database changed under the open transaction".into(),
        });
    }
    if commit {
        transaction.commit()?;
    } else {
        transaction.rollback()?;
    }
    let mut output = Map::new();
    output.insert("results".into(), Json::Array(fresh));
    Ok(Json::Object(output))
}

/// An open transaction (the `transaction` parameter `open`): the statements
/// it ran, each with the result it answered. It holds no connection and no
/// lock between events: each `sql.execute` replays them and runs its own
/// statements in one transaction it rolls back, and `sql.commit` replays
/// them all in one transaction it commits, so a cancelled invocation leaves
/// nothing behind, and a database another writer changed meanwhile fails
/// the transaction with code `changed` (optimistic concurrency).
#[derive(Clone, Debug)]
pub struct Open {
    database: String,
    replayed: Vec<(Statement, Json)>,
}

impl Open {
    /// An open transaction on `database` that has run nothing yet.
    #[must_use]
    pub const fn new(database: String) -> Self {
        Self {
            database,
            replayed: Vec::new(),
        }
    }

    /// Runs `statements` after the ones run before; returns their results.
    ///
    /// # Errors
    ///
    /// As [`apply`], or `changed`, or `bound` when the transaction would
    /// hold more than [`MAX_STATEMENTS`] statements.
    pub fn execute(&mut self, statements: &[Statement]) -> Result<Json, Failed> {
        if exceeds_wasm_bound(self.replayed.len() + statements.len(), MAX_STATEMENTS) {
            return Err(Failed::bound(
                "the open sql transaction exceeds its statements bound",
            ));
        }
        let output = transact(
            &mut connect(&self.database)?,
            &self.replayed,
            statements,
            false,
        )?;
        if let Some(Json::Array(results)) = output.get("results") {
            self.replayed
                .extend(statements.iter().cloned().zip(results.iter().cloned()));
        }
        Ok(output)
    }

    /// Commits every statement run so far.
    ///
    /// # Errors
    ///
    /// As [`Open::execute`].
    pub fn commit(&self) -> Result<(), Failed> {
        transact(&mut connect(&self.database)?, &self.replayed, &[], true).map(drop)
    }

    /// Takes one event the invoking session sent: the event to answer and
    /// whether the invocation ends with it (see the crate documentation,
    /// "Open transactions").
    #[must_use]
    pub fn receive(&mut self, name: &str, data: &Data) -> (Returned, bool) {
        match name {
            EXECUTE_EVENT => match execute_statements(data) {
                Ok(statements) => match self.execute(&statements) {
                    Ok(output) => (
                        Returned {
                            name: RESULT_EVENT,
                            data: canonical_json(&output),
                        },
                        false,
                    ),
                    Err(failed) => (Returned::failed(&failed), true),
                },
                Err(message) => (
                    Returned::failed(&Failed {
                        code: "event",
                        message: message.into(),
                    }),
                    true,
                ),
            },
            COMMIT_EVENT => match self.commit() {
                Ok(()) => (Returned::empty(COMMITTED_EVENT), true),
                Err(failed) => (Returned::failed(&failed), true),
            },
            ROLLBACK_EVENT => (Returned::empty(ROLLEDBACK_EVENT), true),
            _ => (
                Returned::failed(&Failed {
                    code: "event",
                    message: "an open sql transaction takes only sql.execute, sql.commit and sql.rollback".into(),
                }),
                true,
            ),
        }
    }
}

/// The statements of an `sql.execute` event's data `{statements}`.
fn execute_statements(data: &Data) -> Result<Vec<Statement>, &'static str> {
    let malformed = "sql.execute data must be an object with only statements";
    let Data::JsonObject(text) = data else {
        return Err(malformed);
    };
    let Ok(Json::Object(fields)) = serde_json::from_str::<Json>(text) else {
        return Err(malformed);
    };
    match (fields.len(), fields.get("statements")) {
        (1, Some(statements @ Json::Array(_))) => statements_from_json(&statements.to_string()),
        _ => Err(malformed),
    }
}

/// Opens `database` read-write, creating it when missing.
fn connect(database: &str) -> Result<Connection, Failed> {
    Ok(Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

/// Runs each statement in the open transaction and builds the results.
fn execute(connection: &Connection, statements: &[Statement]) -> Result<Json, Failed> {
    let mut results = Vec::with_capacity(statements.len());
    let mut rows_returned = 0_usize;
    for statement in statements {
        let mut prepared = connection.prepare(&statement.sql)?;
        let parameters = statement
            .parameters
            .iter()
            .map(|parameter| match parameter {
                Parameter::Null => Cell::Null,
                Parameter::Integer(value) => Cell::Integer(*value),
                Parameter::Real(value) => Cell::Real(*value),
                Parameter::Text(value) => Cell::Text(value.clone()),
            });
        let (rowcount, rows) = if prepared.column_count() > 0 {
            let columns: Vec<Json> = prepared
                .column_names()
                .into_iter()
                .map(|name| Json::String(name.to_owned()))
                .collect();
            let width = columns.len();
            let mut rows = prepared.query(rusqlite::params_from_iter(parameters))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                rows_returned += 1;
                if exceeds_wasm_bound(rows_returned, MAX_ROWS) {
                    return Err(Failed::bound(
                        "the sql transaction returns more rows than its bound",
                    ));
                }
                let values = (0..width)
                    .map(|index| cell(row.get_ref(index)?))
                    .collect::<Result<Vec<_>, Failed>>()?;
                out.push(serde_json::json!({ "columns": columns, "values": values }));
            }
            (out.len(), out)
        } else {
            (
                prepared.execute(rusqlite::params_from_iter(parameters))?,
                Vec::new(),
            )
        };
        if statement
            .rowcount
            .is_some_and(|expected| u64::try_from(rowcount).ok() != Some(expected))
        {
            return Err(Failed {
                code: "rowcount",
                message: "a statement's rowcount is not the one it expects".into(),
            });
        }
        results.push(serde_json::json!({ "rowcount": rowcount, "rows": rows }));
    }
    let mut output = Map::new();
    output.insert("results".into(), Json::Array(results));
    let output = Json::Object(output);
    if exceeds_wasm_bound(canonical_json(&output).len(), MAX_RESULT_BYTES) {
        return Err(Failed::bound(
            "the sql transaction's result exceeds its bound",
        ));
    }
    Ok(output)
}

/// One cell as JSON.
fn cell(value: ValueRef<'_>) -> Result<Json, Failed> {
    Ok(match value {
        ValueRef::Null => Json::Null,
        ValueRef::Integer(value) => Json::from(value),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Json::Number)
            .ok_or_else(|| Failed {
                code: "SQLITE_MISMATCH",
                message: "an sql cell holds a non-finite real".into(),
            })?,
        ValueRef::Text(bytes) => Json::String(
            std::str::from_utf8(bytes)
                .map_err(|_| Failed {
                    code: "SQLITE_MISMATCH",
                    message: "an sql cell holds text that is not UTF-8".into(),
                })?
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

/// Opens the transaction's database (read-write, created when missing) and
/// applies the transaction: [`RESULT_EVENT`] with the
/// results, or [`FAILED_EVENT`] with why nothing was committed.
#[must_use]
pub fn run(transaction: &Transaction) -> Returned {
    let mut connection = match connect(&transaction.database) {
        Ok(connection) => connection,
        Err(failed) => return Returned::failed(&failed),
    };
    match apply(&mut connection, &transaction.statements) {
        Ok(output) => Returned {
            name: RESULT_EVENT,
            data: canonical_json(&output),
        },
        Err(failed) => Returned::failed(&failed),
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

    use super::{Open, Returned, Transaction};

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
            mut events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            let transaction = observed(Operation::Start, || plan(request))?;
            let (mut returned, returned_reader) = wit_stream::new::<Event>();
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            wit_bindgen::spawn(async move {
                if transaction.open {
                    // An open transaction: its first statements, if any, then
                    // each event the invoking session sends, until one ends
                    // it (or the session stops sending: nothing commits).
                    let mut open = Open::new(transaction.database.clone());
                    if !transaction.statements.is_empty() {
                        let (reply, ended) = match open.execute(&transaction.statements) {
                            Ok(output) => (
                                Returned {
                                    name: super::RESULT_EVENT,
                                    data: scxml_event_schema::canonical_json(&output),
                                },
                                false,
                            ),
                            Err(failed) => (Returned::failed(&failed), true),
                        };
                        if send(&mut returned, &reply).await || ended {
                            drop(returned);
                            let _ = done.write(Ok(None)).await;
                            return;
                        }
                    }
                    while let Some(event) = events.next().await {
                        let (reply, ended) = match native_value(event.data) {
                            Ok(data) => open.receive(&event.name, &data),
                            Err(_) => open.receive("", &scxml_types::DataValue::Null),
                        };
                        if send(&mut returned, &reply).await {
                            return;
                        }
                        if ended {
                            break;
                        }
                    }
                } else {
                    // A batch: SQLite is synchronous, so the transaction runs
                    // to its commit or rollback before the task next yields.
                    // The service reads no event.
                    drop(events);
                    if send(&mut returned, &super::run(&transaction)).await {
                        return;
                    }
                }
                // The last event, then the result: `done.invoke.<id>`
                // follows it.
                drop(returned);
                let _ = done.write(Ok(None)).await;
            });
            Ok((returned_reader, done_reader))
        }
    }

    /// Writes `reply`; true when the invoking session no longer reads.
    async fn send(returned: &mut wit_bindgen::StreamWriter<Event>, reply: &Returned) -> bool {
        telemetry::event(
            Operation::Send,
            if reply.name == super::FAILED_EVENT {
                SignalEvent::InputRejected
            } else {
                SignalEvent::TransformationCompleted
            },
            None,
        );
        returned.write_one(event(reply)).await.is_some()
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
