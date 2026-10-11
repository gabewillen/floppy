//! The `pgsql` invoked service: one Postgres transaction of parameterized
//! statements per `<invoke>`, as a `stateforward:scxml/invoked-service` Wasm
//! component. It is the Postgres engine under the original's Memory ability
//! (`providers/postgres_memory`), beside the SQLite engine
//! `services/sql`, and answers exactly as that service does: the same
//! `statements`, the same `sql.result` and `sql.failed` events. Like it, it
//! knows no schema of its own.
//!
//! Postgres is reached over the Postgres wire protocol (protocol 3.0) on
//! `wasi:sockets/tcp`, which the run grants with
//! `--wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network`:
//! `PGlite` served beside `scx` (`pglite-server` from
//! `@electric-sql/pglite-socket`), a local Postgres, or a hosted one
//! (phase 7's spike, `docs/plans/phase7-pglite-defaults.mdscript.md`).
//! Documents name [`SERVICE_URI`]:
//!
//! ```xml
//! <invoke id="sql" type="https://github.com/stateforward/floppy/services/pgsql">
//!   <param name="database" expr="'postgresql://postgres@127.0.0.1:5432/postgres'"/>
//!   <param name="statements" expr="[{sql: recall_sql, parameters: ['self_name']}]"/>
//! </invoke>
//! ```
//!
//! # Parameters (`<param>`)
//!
//! * `database` (required): a Postgres connection URI,
//!   `postgresql://USER@HOST:PORT/DBNAME` (`postgres://` too), whose host
//!   is an IP address (`127.0.0.1`, `[::1]`): the service imports no name
//!   lookup. It names no password (a document carries no secret), so the
//!   server must accept the user without one (`PGlite` does; a Postgres
//!   needs `trust` for it).
//! * `statements` (required): exactly as `services/sql` takes them
//!   (the original's `Statement`): a non-empty JSON array of objects with `sql`
//!   (one statement with positional `?` or numbered `?NNN` placeholders,
//!   which the service rewrites to `$1`, `$2`, ... (`?NNN` to `$NNN`)
//!   outside string literals, quoted
//!   identifiers, dollar-quoted strings, and comments, as the original's
//!   `postgres_memory` rewrites them for a `NUMERIC` adapter) and
//!   optionally `parameters` (strings, finite numbers, booleans as 1 or 0,
//!   and nulls), and optionally `rowcount` (the rowcount the statement
//!   must have, else the whole transaction fails with code `rowcount`).
//! * `transaction` (optional): `open` keeps the transaction open across
//!   events (see "Open transactions"); `statements` is then optional.
//!
//! Any other parameter, a parameter given twice, or a malformed value
//! rejects the start: the `<invoke>` raises `error.execution` and nothing
//! is sent.
//!
//! # The transaction
//!
//! The service connects, starts the session as the URI's user on its
//! database, runs `BEGIN`, then each statement with the extended query
//! protocol (one statement per `Parse`, its parameters bound as text whose
//! types the server infers), and `COMMIT`. A statement that returns
//! columns yields its rows; any other yields the rows its command tag
//! counts (`INSERT 0 1`, `UPDATE 2`), else 0. A statement that would end
//! or start a transaction (`BEGIN`, `START`, `COMMIT`, `END`, `ABORT`,
//! `ROLLBACK` other than `ROLLBACK TO`, `PREPARE TRANSACTION`) is refused
//! before it is sent. On any failure the service sends `ROLLBACK`: nothing
//! is committed. The connection is closed when the invocation ends; the
//! service keeps no state between invocations.
//!
//! A cell is its text value as JSON, typed like `services/sql`'s where
//! Postgres's type says so: `NULL` as `null`, `int2`/`int4`/`int8`,
//! `float4`/`float8` and finite `numeric` as numbers, `bool` as a boolean,
//! `bytea` as its lowercase hex text, anything else as its text.
//!
//! # Events
//!
//! Exactly `services/sql`'s: [`RESULT_EVENT`] `{"results": [{"rowcount":
//! N, "rows": [{"columns": [...], "values": [...]}]}]}` or [`FAILED_EVENT`]
//! `{"code": C, "message": M}`: `code` is the server's `SQLSTATE`
//! (`23505`, `40001`, ...), or `rowcount`, `bound`, `statement`,
//! `connection`, `changed` or `event` ([`Failed`]); `message` is the
//! server's message, or the rule or bound that was broken.
//!
//! # Open transactions
//!
//! Exactly `services/sql`'s (its crate documentation): with `transaction`
//! `open`, `sql.execute {statements}` answers `sql.result`, `sql.commit`
//! answers `sql.committed {}`, `sql.rollback` answers `sql.rolledback {}`,
//! and a failure or any other event answers `sql.failed`, each committing
//! nothing; a cancel commits nothing. The service holds no connection
//! between events: it replays the statements it ran, checks each result
//! against the one it answered (else code `changed`), and commits only on
//! `sql.commit`.
//!
//! # Bounds
//!
//! The same as `services/sql`'s, on wasm builds only
//! ([`scxml_types::wasm_bound`]), plus one server message's bytes
//! ([`MAX_MESSAGE_BYTES`]).
//!
//! # Telemetry
//!
//! As `services/sql`: one observation of `start` and one closed signal
//! event per call through scxml.rs's `telemetry` WIT facade; no attribute
//! carries the SQL, a value, or the URI.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::SocketAddr;

use scxml_event_schema::canonical_json;
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

/// The service's name: the last path segment of its repository service URI
/// before the tag.
pub const SERVICE_NAME: &str = "pgsql";

/// The exact service type URI documents name: tagless, so it names the
/// repository's latest release (decision D19); the manifest names it too.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/pgsql";

/// The event that carries a committed transaction's results, named as
/// `services/sql` names it.
pub const RESULT_EVENT: &str = "sql.result";
/// The event that reports a transaction that did not commit.
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
/// Upper bound on the connection URI's bytes (wasm builds only).
pub const MAX_DATABASE_BYTES: usize = wasm_bound(1024);
/// Upper bound on the rows one transaction returns, all statements
/// together (wasm builds only).
pub const MAX_ROWS: usize = wasm_bound(1024);
/// Upper bound on the encoded result's bytes (wasm builds only): under the
/// session's 96 KiB event bound, leaving room for the event's envelope.
pub const MAX_RESULT_BYTES: usize = wasm_bound(88 * 1024);
/// Upper bound on one server message's bytes (every build): a larger one
/// fails the transaction rather than being read.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Upper bound on a failure message's bytes.
const MAX_FAILURE_BYTES: usize = 512;

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

impl Parameter {
    /// The value in Postgres's text format, or `None` for `NULL`.
    fn text(&self) -> Option<String> {
        match self {
            Self::Null => None,
            Self::Integer(value) => Some(value.to_string()),
            Self::Real(value) => Some(value.to_string()),
            Self::Text(value) => Some(value.clone()),
        }
    }
}

/// One parameterized statement (the original's `Statement`), its placeholders
/// already rewritten to `$n`.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// Exactly one SQL statement with `$n` placeholders.
    pub sql: String,
    /// Its bind values, in order.
    pub parameters: Vec<Parameter>,
    /// The `rowcount` it must have, else the transaction fails (a guard,
    /// for example a version row that must still be the one read).
    pub rowcount: Option<u64>,
}

/// Why a transaction did not commit: a code and a message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failed {
    /// The server's `SQLSTATE` (`23505`, `40001`, ...), or `rowcount` (a
    /// statement's expected rowcount differed), `bound` (a bound was
    /// exceeded), `statement` (a statement that would end or start a
    /// transaction), `connection` (the server could not be reached or broke
    /// the protocol), `changed` or `event` (an open transaction's).
    pub code: String,
    /// The server's message, or the rule that was broken.
    pub message: String,
}

impl Failed {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<String> for Failed {
    fn from(message: String) -> Self {
        Self::new("connection", message)
    }
}

impl From<&str> for Failed {
    fn from(message: &str) -> Self {
        Self::new("connection", message)
    }
}

/// Where the transaction runs: the connection URI's parts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Database {
    /// The server's address.
    pub address: SocketAddr,
    /// The user the session starts as.
    pub user: String,
    /// The database the session opens.
    pub name: String,
}

/// One validated invocation: the database and its transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    /// The connection URI's parts.
    pub database: Database,
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
        let message = failed.message.as_str();
        let mut end = message.len().min(MAX_FAILURE_BYTES);
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
            _ => return Err("pgsql takes no parameter of that name"),
        };
        if slot.replace(value).is_some() {
            return Err("pgsql takes each parameter once");
        }
    }
    let database = match database.ok_or("pgsql requires a database parameter")? {
        Data::String(uri) if !exceeds_wasm_bound(uri.len(), MAX_DATABASE_BYTES) => {
            parse_database(uri)?
        }
        _ => return Err("pgsql database must be a connection URI within its bound"),
    };
    let open = match transaction {
        None => false,
        Some(Data::String(mode)) if mode == "open" => true,
        Some(_) => return Err("pgsql transaction must be open"),
    };
    let statements = match (statements, open) {
        (Some(Data::JsonArray(text)), _) => statements_from_json(text)?,
        (Some(_), _) => return Err("pgsql statements must be a JSON array of statements"),
        (None, true) => Vec::new(),
        (None, false) => return Err("pgsql requires a statements parameter"),
    };
    Ok(Transaction {
        database,
        statements,
        open,
    })
}

/// `postgresql://USER@HOST:PORT/DBNAME` (or `postgres://`), the host an IP
/// address.
fn parse_database(uri: &str) -> Result<Database, &'static str> {
    let malformed = "pgsql database must be postgresql://USER@IP:PORT/DBNAME";
    let rest = uri
        .strip_prefix("postgresql://")
        .or_else(|| uri.strip_prefix("postgres://"))
        .ok_or(malformed)?;
    let (authority, name) = rest.split_once('/').ok_or(malformed)?;
    let (user, host) = authority.rsplit_once('@').ok_or(malformed)?;
    if user.contains(':') {
        return Err("pgsql database names no password");
    }
    let plain = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
    };
    if !plain(user) || !plain(name) {
        return Err(malformed);
    }
    let address = host.parse::<SocketAddr>().map_err(|_| malformed)?;
    Ok(Database {
        address,
        user: user.to_owned(),
        name: name.to_owned(),
    })
}

fn statements_from_json(text: &str) -> Result<Vec<Statement>, &'static str> {
    let malformed = "pgsql statements must be a JSON array of statements";
    let Ok(Json::Array(items)) = serde_json::from_str::<Json>(text) else {
        return Err(malformed);
    };
    if items.is_empty() {
        return Err("pgsql statements must hold at least one statement");
    }
    if exceeds_wasm_bound(items.len(), MAX_STATEMENTS) {
        return Err("pgsql statements exceed their bound");
    }
    items.iter().map(statement_from_json).collect()
}

fn statement_from_json(item: &Json) -> Result<Statement, &'static str> {
    let Json::Object(fields) = item else {
        return Err("pgsql statement must be an object with sql and parameters");
    };
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "sql" | "parameters" | "rowcount"))
    {
        return Err("pgsql statement takes only sql, parameters and rowcount");
    }
    let sql = match fields.get("sql") {
        Some(Json::String(sql)) if !sql.trim().is_empty() && !sql.contains('\0') => sql,
        _ => return Err("pgsql statement sql must be a non-empty string"),
    };
    if exceeds_wasm_bound(sql.len(), MAX_SQL_BYTES) {
        return Err("pgsql statement sql exceeds its bound");
    }
    let parameters = match fields.get("parameters") {
        None => Vec::new(),
        Some(Json::Array(values)) => {
            if exceeds_wasm_bound(values.len(), MAX_PARAMETERS) {
                return Err("pgsql statement parameters exceed their bound");
            }
            values
                .iter()
                .map(parameter_from_json)
                .collect::<Result<_, _>>()?
        }
        Some(_) => return Err("pgsql statement parameters must be an array"),
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
                .ok_or("pgsql statement rowcount must be a non-negative integer")?,
        ),
        Some(_) => return Err("pgsql statement rowcount must be a non-negative integer"),
    };
    Ok(Statement {
        sql: numbered(sql),
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
                Some(real) if real.fract() == 0.0 && real.abs() < 9.007_199_254_740_992e15 =>
                {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "integral and within 2^53, so exact"
                    )]
                    Parameter::Integer(real as i64)
                }
                Some(real) if real.is_finite() => Parameter::Real(real),
                _ => return Err("pgsql statement parameters must be finite numbers"),
            },
        },
        Json::String(text) if !text.contains('\0') => Parameter::Text(text.clone()),
        Json::String(_) => return Err("pgsql statement parameters hold no NUL"),
        Json::Array(_) | Json::Object(_) => {
            return Err("pgsql statement parameters must be strings, numbers, booleans, or nulls");
        }
    })
}

/// `sql` with each `?` placeholder rewritten to `$1`, `$2`, ... in order
/// and each `?NNN` to `$NNN` (SQLite's rules: a bare `?` is one more than
/// the largest number so far), except inside `'...'` and `E'...'` string literals, `"..."` quoted
/// identifiers, `$tag$...$tag$` dollar-quoted strings, `--` comments, and
/// `/* */` comments (which nest in Postgres).
#[must_use]
pub fn numbered(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut next = 1_usize;
    let mut index = 0;
    let mut copied = 0;
    while index < bytes.len() {
        let rest = &bytes[index..];
        let skip = match rest[0] {
            b'\'' => {
                let escapes = index > 0 && matches!(bytes[index - 1], b'E' | b'e');
                quoted(rest, b'\'', escapes)
            }
            b'"' => quoted(rest, b'"', false),
            b'-' if rest.get(1) == Some(&b'-') => rest
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(rest.len()),
            b'/' if rest.get(1) == Some(&b'*') => block_comment(rest),
            b'$' => dollar_quoted(rest),
            b'?' => {
                out.push_str(&sql[copied..index]);
                // `?NNN` names its parameter (SQLite's numbered form); a
                // bare `?` takes the one after the largest so far.
                let digits = rest[1..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit())
                    .count();
                let number = sql[index + 1..index + 1 + digits].parse::<usize>().ok();
                let number = number.unwrap_or(next);
                let _ = write!(out, "${number}");
                next = next.max(number + 1);
                index += 1 + digits;
                copied = index;
                continue;
            }
            _ => 1,
        };
        index += skip.max(1);
    }
    out.push_str(&sql[copied.min(sql.len())..]);
    out
}

/// The length of the quoted run at the start of `rest` (its closing quote
/// included; a doubled quote is part of it; with `escapes`, so is a
/// backslash-escaped one), or all of `rest` when it is unterminated.
fn quoted(rest: &[u8], quote: u8, escapes: bool) -> usize {
    let mut index = 1;
    while index < rest.len() {
        match rest[index] {
            b'\\' if escapes => index += 2,
            byte if byte == quote => {
                if rest.get(index + 1) == Some(&quote) {
                    index += 2;
                } else {
                    return index + 1;
                }
            }
            _ => index += 1,
        }
    }
    rest.len()
}

/// The length of the (nested) block comment at the start of `rest`.
fn block_comment(rest: &[u8]) -> usize {
    let mut depth = 0_usize;
    let mut index = 0;
    while index + 1 < rest.len() {
        match (rest[index], rest[index + 1]) {
            (b'/', b'*') => {
                depth += 1;
                index += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                index += 2;
                if depth == 0 {
                    return index;
                }
            }
            _ => index += 1,
        }
    }
    rest.len()
}

/// The length of the dollar-quoted string at the start of `rest`, or 1 when
/// the `$` opens none (a `$n` placeholder or part of an identifier).
fn dollar_quoted(rest: &[u8]) -> usize {
    let Some(end) = rest[1..].iter().position(|byte| *byte == b'$') else {
        return 1;
    };
    let tag = &rest[..end + 2];
    if !tag[1..tag.len() - 1]
        .iter()
        .all(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        || tag[1..tag.len() - 1]
            .first()
            .is_some_and(u8::is_ascii_digit)
    {
        return 1;
    }
    rest[tag.len()..]
        .windows(tag.len())
        .position(|window| window == tag)
        .map_or(rest.len(), |close| tag.len() + close + tag.len())
}

/// Whether `sql` would end or start a transaction (see "The
/// transaction"): its first keyword after whitespace and comments.
#[must_use]
pub fn controls_transaction(sql: &str) -> bool {
    let words: Vec<String> = strip_leading_comments(sql)
        .split(|character: char| !character.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .take(2)
        .map(str::to_ascii_uppercase)
        .collect();
    match words.first().map(String::as_str) {
        Some("BEGIN" | "START" | "COMMIT" | "END" | "ABORT") => true,
        Some("ROLLBACK") => words.get(1).map(String::as_str) != Some("TO"),
        Some("PREPARE") => words.get(1).map(String::as_str) == Some("TRANSACTION"),
        _ => false,
    }
}

fn strip_leading_comments(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();
        if sql.starts_with("--") {
            sql = sql.find('\n').map_or("", |end| &sql[end..]);
        } else if sql.starts_with("/*") {
            sql = &sql[block_comment(sql.as_bytes()).min(sql.len())..];
        } else {
            return sql;
        }
    }
}

/// One message from the server: its type byte and body.
struct Message {
    kind: u8,
    body: Vec<u8>,
}

/// One session on a connection.
struct Session<S> {
    stream: S,
}

impl<S: Read + Write> Session<S> {
    fn send(&mut self, kind: Option<u8>, body: &[u8]) -> Result<(), String> {
        let length = u32::try_from(body.len() + 4).map_err(|_| "a message is too long")?;
        let mut message = Vec::with_capacity(body.len() + 5);
        message.extend(kind);
        message.extend_from_slice(&length.to_be_bytes());
        message.extend_from_slice(body);
        self.stream
            .write_all(&message)
            .and_then(|()| self.stream.flush())
            .map_err(|error| format!("the connection failed: {error}"))
    }

    fn receive(&mut self) -> Result<Message, String> {
        let mut header = [0_u8; 5];
        self.stream
            .read_exact(&mut header)
            .map_err(|error| format!("the connection failed: {error}"))?;
        let [kind, length @ ..] = header;
        let length = usize::try_from(u32::from_be_bytes(length))
            .ok()
            .and_then(|length| length.checked_sub(4))
            .filter(|length| *length <= MAX_MESSAGE_BYTES)
            .ok_or("the server sent a message over its bound")?;
        let mut body = vec![0_u8; length];
        self.stream
            .read_exact(&mut body)
            .map_err(|error| format!("the connection failed: {error}"))?;
        Ok(Message { kind, body })
    }

    /// Starts the session (protocol 3.0) as `database.user` on
    /// `database.name`, in UTC (so `CURRENT_TIMESTAMP` reads as the SQLite
    /// engine's does), and waits until the server is ready for a query.
    fn start(&mut self, database: &Database) -> Result<(), String> {
        let mut startup = Vec::new();
        startup.extend_from_slice(&196_608_u32.to_be_bytes());
        for text in [
            "user",
            database.user.as_str(),
            "database",
            database.name.as_str(),
            "client_encoding",
            "UTF8",
            "TimeZone",
            "UTC",
            "",
        ] {
            startup.extend_from_slice(text.as_bytes());
            startup.push(0);
        }
        self.send(None, &startup)?;
        loop {
            let message = self.receive()?;
            match message.kind {
                b'R' if message.body.get(..4) == Some(&[0, 0, 0, 0]) => {}
                b'R' => {
                    return Err("the server asks for a password; pgsql connects without one".into());
                }
                b'E' => return Err(server_error(&message.body).message),
                b'Z' => return Ok(()),
                _ => {}
            }
        }
    }

    /// Runs `sql` as a simple query and waits for the server to be ready.
    fn simple(&mut self, sql: &str) -> Result<(), Failed> {
        let mut text = sql.as_bytes().to_vec();
        text.push(0);
        self.send(Some(b'Q'), &text)?;
        let mut failure = None;
        loop {
            let message = self.receive()?;
            match message.kind {
                b'E' => failure = Some(server_error(&message.body)),
                b'Z' => return failure.map_or(Ok(()), Err),
                _ => {}
            }
        }
    }

    /// Runs one statement with the extended query protocol and returns its
    /// result; `rows_returned` counts the transaction's rows so far.
    fn statement(
        &mut self,
        statement: &Statement,
        rows_returned: &mut usize,
    ) -> Result<Json, Failed> {
        let mut parse = vec![0];
        parse.extend_from_slice(statement.sql.as_bytes());
        parse.extend_from_slice(&[0, 0, 0]);
        let count = i16::try_from(statement.parameters.len())
            .map_err(|_| "a statement has too many parameters")?;
        let mut bind = vec![0, 0];
        bind.extend_from_slice(&0_i16.to_be_bytes());
        bind.extend_from_slice(&count.to_be_bytes());
        for parameter in &statement.parameters {
            match parameter.text() {
                None => bind.extend_from_slice(&(-1_i32).to_be_bytes()),
                Some(text) => {
                    let length =
                        i32::try_from(text.len()).map_err(|_| "a parameter is too long")?;
                    bind.extend_from_slice(&length.to_be_bytes());
                    bind.extend_from_slice(text.as_bytes());
                }
            }
        }
        bind.extend_from_slice(&0_i16.to_be_bytes());
        self.send(Some(b'P'), &parse)?;
        self.send(Some(b'B'), &bind)?;
        self.send(Some(b'D'), b"P\0")?;
        self.send(Some(b'E'), &[0, 0, 0, 0, 0])?;
        self.send(Some(b'S'), &[])?;
        let mut columns: Option<Vec<(Json, u32)>> = None;
        let mut rows = Vec::new();
        let mut tag = String::new();
        let mut failure = None;
        loop {
            let message = self.receive()?;
            match message.kind {
                b'T' => columns = Some(row_description(&message.body)?),
                b'D' if failure.is_none() => {
                    *rows_returned += 1;
                    if exceeds_wasm_bound(*rows_returned, MAX_ROWS) {
                        failure = Some(Failed::new(
                            "bound",
                            "the pgsql transaction returns more rows than its bound",
                        ));
                        continue;
                    }
                    let described = columns.as_deref().unwrap_or_default();
                    let values = data_row(&message.body, described)?;
                    let names: Vec<Json> = described.iter().map(|(name, _)| name.clone()).collect();
                    rows.push(serde_json::json!({ "columns": names, "values": values }));
                }
                b'C' => tag = Body(&message.body).cstring()?,
                b'E' => failure = Some(server_error(&message.body)),
                b'Z' => break,
                _ => {}
            }
        }
        if let Some(failure) = failure {
            return Err(failure);
        }
        let (rowcount, result) = if columns.is_some() {
            (
                rows.len() as u64,
                serde_json::json!({ "rowcount": rows.len(), "rows": rows }),
            )
        } else {
            let changed = tag
                .rsplit(' ')
                .next()
                .and_then(|count| count.parse::<u64>().ok())
                .filter(|_| tag.contains(' '))
                .unwrap_or(0);
            (
                changed,
                serde_json::json!({ "rowcount": changed, "rows": [] }),
            )
        };
        if statement
            .rowcount
            .is_some_and(|expected| expected != rowcount)
        {
            return Err(Failed::new(
                "rowcount",
                "a statement's rowcount is not the one it expects",
            ));
        }
        Ok(result)
    }
}

/// A cursor over one message body.
struct Body<'a>(&'a [u8]);

impl Body<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], String> {
        if self.0.len() < count {
            return Err("the server sent a truncated message".into());
        }
        let (head, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(head)
    }

    fn int16(&mut self) -> Result<i16, String> {
        let bytes = self.take(2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn int32(&mut self) -> Result<i32, String> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn cstring(&mut self) -> Result<String, String> {
        let end = self
            .0
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("the server sent an unterminated string")?;
        let text = std::str::from_utf8(self.take(end)?)
            .map_err(|_| "the server sent text that is not UTF-8")?
            .to_owned();
        self.take(1)?;
        Ok(text)
    }
}

/// The columns of a `RowDescription`: each name and type OID.
fn row_description(body: &[u8]) -> Result<Vec<(Json, u32)>, String> {
    let mut body = Body(body);
    let count = body.int16()?;
    (0..count)
        .map(|_| {
            let name = body.cstring()?;
            body.take(6)?;
            let oid = u32::from_be_bytes(
                body.take(4)?
                    .try_into()
                    .map_err(|_| "the server sent a truncated message")?,
            );
            body.take(8)?;
            Ok((Json::String(name), oid))
        })
        .collect()
}

/// One `DataRow`'s values, typed by `columns`.
fn data_row(body: &[u8], columns: &[(Json, u32)]) -> Result<Vec<Json>, String> {
    let mut body = Body(body);
    let count = usize::try_from(body.int16()?).map_err(|_| "a row has a negative width")?;
    if count != columns.len() {
        return Err("a row does not match its description".into());
    }
    columns
        .iter()
        .map(|(_, oid)| {
            let length = body.int32()?;
            let Ok(length) = usize::try_from(length) else {
                return Ok(Json::Null);
            };
            let text = std::str::from_utf8(body.take(length)?)
                .map_err(|_| "a pgsql cell holds text that is not UTF-8")?;
            Ok(cell(text, *oid))
        })
        .collect()
}

/// One non-null cell's text value as JSON, typed by its type OID.
fn cell(text: &str, oid: u32) -> Json {
    match oid {
        // bool
        16 => Json::Bool(text == "t"),
        // bytea, hex output: `\x` and lowercase hex digits
        17 => Json::String(text.strip_prefix("\\x").unwrap_or(text).to_owned()),
        // int8, int2, int4
        20 | 21 | 23 => text
            .parse::<i64>()
            .map_or_else(|_| Json::String(text.to_owned()), Json::from),
        // float4, float8, numeric
        700 | 701 | 1700 => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| Json::String(text.to_owned()), Json::Number),
        _ => Json::String(text.to_owned()),
    }
}

/// An `ErrorResponse`'s `C` (`SQLSTATE`) and `M` (message) fields.
fn server_error(body: &[u8]) -> Failed {
    let mut body = Body(body);
    let mut code = None;
    let mut message = None;
    while let Some(&field) = body.0.first() {
        if field == 0 || body.take(1).is_err() {
            break;
        }
        match body.cstring() {
            Ok(text) if field == b'C' => code = Some(text),
            Ok(text) if field == b'M' => message = Some(text),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Failed {
        code: code.unwrap_or_else(|| "XX000".into()),
        message: message.unwrap_or_else(|| "the server reported an error".into()),
    }
}

/// Runs `replayed` (statements an open transaction ran before, with the
/// results it answered) and then `statements` in one transaction on a
/// started session, commits when `commit` and else rolls back, and returns
/// the results of `statements` alone; on any failure it rolls back. A
/// replayed statement whose result differs from the one answered fails
/// with code `changed`.
fn transact<S: Read + Write>(
    session: &mut Session<S>,
    replayed: &[(Statement, Json)],
    statements: &[Statement],
    commit: bool,
) -> Result<Json, Failed> {
    if statements
        .iter()
        .any(|statement| controls_transaction(&statement.sql))
    {
        return Err(Failed::new(
            "statement",
            "a pgsql statement may not end or start a transaction",
        ));
    }
    session.simple("BEGIN")?;
    let output = execute(session, replayed, statements);
    match output {
        Ok(output) => {
            session.simple(if commit { "COMMIT" } else { "ROLLBACK" })?;
            Ok(output)
        }
        Err(failed) => {
            let _ = session.simple("ROLLBACK");
            Err(failed)
        }
    }
}

fn execute<S: Read + Write>(
    session: &mut Session<S>,
    replayed: &[(Statement, Json)],
    statements: &[Statement],
) -> Result<Json, Failed> {
    let mut rows_returned = 0_usize;
    for (statement, answered) in replayed {
        if session.statement(statement, &mut rows_returned)? != *answered {
            return Err(Failed::new(
                "changed",
                "the database changed under the open transaction",
            ));
        }
    }
    let mut results = Vec::with_capacity(statements.len());
    for statement in statements {
        results.push(session.statement(statement, &mut rows_returned)?);
    }
    let mut output = Map::new();
    output.insert("results".into(), Json::Array(results));
    let output = Json::Object(output);
    if exceeds_wasm_bound(canonical_json(&output).len(), MAX_RESULT_BYTES) {
        return Err(Failed::new(
            "bound",
            "the pgsql transaction's result exceeds its bound",
        ));
    }
    Ok(output)
}

/// Starts a session on `stream`, a connection to `database`, runs
/// [`transact`] and ends the session.
fn transact_on<S: Read + Write>(
    stream: S,
    database: &Database,
    replayed: &[(Statement, Json)],
    statements: &[Statement],
    commit: bool,
) -> Result<Json, Failed> {
    let mut session = Session { stream };
    let output = session
        .start(database)
        .map_err(Failed::from)
        .and_then(|()| transact(&mut session, replayed, statements, commit));
    let _ = session.send(Some(b'X'), &[]);
    output
}

/// Connects to `database` and runs [`transact_on`].
fn transact_at(
    database: &Database,
    replayed: &[(Statement, Json)],
    statements: &[Statement],
    commit: bool,
) -> Result<Json, Failed> {
    let stream = connect(database.address).map_err(|message| {
        Failed::new("connection", format!("pgsql could not connect: {message}"))
    })?;
    transact_on(stream, database, replayed, statements, commit)
}

/// The reply to a batch's or an exchange's output.
fn returned(output: Result<Json, Failed>) -> Returned {
    match output {
        Ok(output) => Returned {
            name: RESULT_EVENT,
            data: canonical_json(&output),
        },
        Err(failed) => Returned::failed(&failed),
    }
}

/// Connects to the transaction's database and applies the transaction.
#[must_use]
pub fn run(transaction: &Transaction) -> Returned {
    returned(transact_at(
        &transaction.database,
        &[],
        &transaction.statements,
        true,
    ))
}

/// An open transaction (the `transaction` parameter `open`), exactly as
/// `services/sql`'s: the statements it ran, each with the result it
/// answered. It holds no connection between events: each `sql.execute`
/// replays them and runs its own in one transaction it rolls back, and
/// `sql.commit` replays them all in one it commits, so a cancelled
/// invocation leaves nothing behind and a database another writer changed
/// meanwhile fails the transaction with code `changed`.
#[derive(Clone, Debug)]
pub struct Open {
    database: Database,
    replayed: Vec<(Statement, Json)>,
}

impl Open {
    /// An open transaction on `database` that has run nothing yet.
    #[must_use]
    pub const fn new(database: Database) -> Self {
        Self {
            database,
            replayed: Vec::new(),
        }
    }

    /// Runs `statements` after the ones run before; returns their results.
    ///
    /// # Errors
    ///
    /// As a batch, or `changed`, or `bound` when the transaction would hold
    /// more than [`MAX_STATEMENTS`] statements.
    pub fn execute(&mut self, statements: &[Statement]) -> Result<Json, Failed> {
        if exceeds_wasm_bound(self.replayed.len() + statements.len(), MAX_STATEMENTS) {
            return Err(Failed::new(
                "bound",
                "the open pgsql transaction exceeds its statements bound",
            ));
        }
        let output = transact_at(&self.database, &self.replayed, statements, false)?;
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
        transact_at(&self.database, &self.replayed, &[], true).map(drop)
    }

    /// Takes one event the invoking session sent: the event to answer and
    /// whether the invocation ends with it.
    #[must_use]
    pub fn receive(&mut self, name: &str, data: &Data) -> (Returned, bool) {
        match name {
            EXECUTE_EVENT => match execute_statements(data) {
                Ok(statements) => {
                    let output = self.execute(&statements);
                    let ended = output.is_err();
                    (returned(output), ended)
                }
                Err(message) => (Returned::failed(&Failed::new("event", message)), true),
            },
            COMMIT_EVENT => match self.commit() {
                Ok(()) => (Returned::empty(COMMITTED_EVENT), true),
                Err(failed) => (Returned::failed(&failed), true),
            },
            ROLLBACK_EVENT => (Returned::empty(ROLLEDBACK_EVENT), true),
            _ => (
                Returned::failed(&Failed::new(
                    "event",
                    "an open pgsql transaction takes only sql.execute, sql.commit and sql.rollback",
                )),
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

#[cfg(not(target_arch = "wasm32"))]
fn connect(address: SocketAddr) -> Result<std::net::TcpStream, String> {
    std::net::TcpStream::connect(address).map_err(|error| error.to_string())
}

#[cfg(target_arch = "wasm32")]
fn connect(address: SocketAddr) -> Result<socket::Tcp, String> {
    socket::Tcp::connect(address)
}

/// The component's connection: `wasi:sockets/tcp` directly.
#[cfg(target_arch = "wasm32")]
mod socket {
    use std::io;
    use std::net::SocketAddr;

    use wasip2::io::streams::{InputStream, OutputStream, StreamError};
    use wasip2::sockets::instance_network::instance_network;
    use wasip2::sockets::network::{
        ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress,
    };
    use wasip2::sockets::tcp::TcpSocket;
    use wasip2::sockets::tcp_create_socket::create_tcp_socket;

    /// The most bytes one write hands the stream.
    const CHUNK: usize = 4096;

    /// One connected TCP socket and its streams (dropped streams first).
    pub struct Tcp {
        input: InputStream,
        output: OutputStream,
        _socket: TcpSocket,
    }

    impl Tcp {
        pub fn connect(address: SocketAddr) -> Result<Self, String> {
            let (family, remote) = match address {
                SocketAddr::V4(v4) => {
                    let [a, b, c, d] = v4.ip().octets();
                    (
                        IpAddressFamily::Ipv4,
                        IpSocketAddress::Ipv4(Ipv4SocketAddress {
                            port: v4.port(),
                            address: (a, b, c, d),
                        }),
                    )
                }
                SocketAddr::V6(v6) => {
                    let segments = v6.ip().segments();
                    (
                        IpAddressFamily::Ipv6,
                        IpSocketAddress::Ipv6(Ipv6SocketAddress {
                            port: v6.port(),
                            flow_info: v6.flowinfo(),
                            address: (
                                segments[0],
                                segments[1],
                                segments[2],
                                segments[3],
                                segments[4],
                                segments[5],
                                segments[6],
                                segments[7],
                            ),
                            scope_id: v6.scope_id(),
                        }),
                    )
                }
            };
            let socket = create_tcp_socket(family).map_err(describe)?;
            socket
                .start_connect(&instance_network(), remote)
                .map_err(describe)?;
            let ready = socket.subscribe();
            let (input, output) = loop {
                match socket.finish_connect() {
                    Ok(streams) => break streams,
                    Err(ErrorCode::WouldBlock) => ready.block(),
                    Err(code) => return Err(describe(code)),
                }
            };
            drop(ready);
            Ok(Self {
                input,
                output,
                _socket: socket,
            })
        }
    }

    fn describe(code: ErrorCode) -> String {
        code.name().to_owned()
    }

    fn failed(error: &StreamError) -> io::Error {
        match error {
            StreamError::Closed => io::Error::from(io::ErrorKind::UnexpectedEof),
            StreamError::LastOperationFailed(error) => io::Error::other(error.to_debug_string()),
        }
    }

    impl io::Read for Tcp {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let wanted = u64::try_from(buf.len()).unwrap_or(u64::MAX);
            match self.input.blocking_read(wanted) {
                Ok(bytes) => {
                    let read = bytes.len().min(buf.len());
                    buf[..read].copy_from_slice(&bytes[..read]);
                    Ok(read)
                }
                Err(StreamError::Closed) => Ok(0),
                Err(error) => Err(failed(&error)),
            }
        }
    }

    impl io::Write for Tcp {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let chunk = &buf[..buf.len().min(CHUNK)];
            self.output
                .blocking_write_and_flush(chunk)
                .map_err(|error| failed(&error))?;
            Ok(chunk.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.output.blocking_flush().map_err(|error| failed(&error))
        }
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
                // The wire exchange uses the blocking WASI 0.2 socket
                // streams: each transaction runs to its commit or rollback
                // before the task next yields.
                if transaction.open {
                    // An open transaction: its first statements, if any, then
                    // each event the invoking session sends, until one ends
                    // it (or the session stops sending: nothing commits).
                    let mut open = Open::new(transaction.database.clone());
                    if !transaction.statements.is_empty() {
                        let output = open.execute(&transaction.statements);
                        let ended = output.is_err();
                        if send(&mut returned, &super::returned(output)).await || ended {
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
                    // A batch reads no event.
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
