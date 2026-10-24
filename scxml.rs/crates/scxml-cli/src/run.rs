//! `scx run`: one long-lived live session of a document over
//! newline-delimited JSON (`docs/live-session.md`, "The `run` command").
//!
//! Standard input carries one input per line (an event, a snapshot request,
//! or a close request); standard output carries one record per line
//! (`opened`, `log`, `call`, `snapshot`, `done`, `error`, `closed`);
//! standard error carries diagnostics only. The session runs through the
//! shipped components under the Wasmtime session host (`SessionHost::open`);
//! a build without the `wasmtime` feature fails closed before anything runs.
//!
//! `BasicHTTP` (SCXML C.2) is off by default: every POST outside the
//! session's own address space raises `error.communication`.
//! `--http-allow` lets POSTs reach the allowlisted origins and prefixes,
//! and `--http-listen` serves every session's `BasicHTTP` location, which
//! `--http-public-url` sets when the listener sits behind a proxy or
//! tunnel.
//!
//! Exit status: 0 when the root reached a top-level final state, 1 when
//! the session failed, 2 for usage, compilation, component, or I/O
//! failures before or around the session, 3 when the session closed while
//! still running (end of input, a `close` input, or the end of a replayed
//! log), and 128 + N when signal N (`SIGINT`, `SIGTERM`) closed it.

use std::path::PathBuf;

use scxml_snapshot::{Options, TraceContextPolicy};

use crate::components::ComponentArgs;

/// Options of `run`.
pub const RUN_OPTIONS: &[&str] = &[
    "--components",
    "--components-manifest-sha256",
    "--extension-package",
    "--extension-component",
    "--extension-component-sha256",
    "--service-package",
    "--service-component",
    "--service-component-sha256",
    "--data-model-package",
    "--data-model-component",
    "--data-model-component-sha256",
    "--dir",
    "--wasi",
    "--env",
    "--env-file",
    "--offline",
    "--cache-dir",
    "--github-api-base",
    "--github-raw-base",
    "--github-download-base",
    "--github-token-env",
    "--fuel",
    "--max-commands",
    "--session-id",
    "--clock",
    "--snapshot-on",
    "--exclude",
    "--trace-context",
    "--on-eof",
    "--record",
    "--replay",
    "--http-allow",
    "--http-listen",
    "--http-public-url",
];

/// The loopback address `--http-listen PORT` binds.
const DEFAULT_LISTEN_IP: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// How the session's logical clock moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockMode {
    /// The process's monotonic wall clock: delayed sends fire at their real
    /// deadline (the default).
    Wall,
    /// The deterministic logical clock: after each event every pending
    /// delayed send fires at once, in deadline order.
    Logical,
}

/// When `run` writes snapshot records on its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotOn {
    /// After every call (start, event, timer).
    Call,
    /// Once, when the root finishes.
    Done,
    /// Never (the default); a `{"type":"snapshot"}` input still asks for
    /// one.
    Never,
}

/// What `run` does at the end of standard input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnEof {
    /// Keep firing delayed sends until the root finishes or nothing is
    /// scheduled, then close (the default).
    Wait,
    /// Close at once.
    Close,
}

/// Parsed `run` arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunArgs {
    /// The SCXML document.
    pub document: PathBuf,
    /// Release components and their manifest digest.
    pub components: ComponentArgs,
    /// Extension packages, extension components, and their environment.
    pub(crate) namespaces: crate::extensions::RunFlags,
    /// Invoked-service packages and their components.
    pub(crate) services: crate::services::ServiceFlags,
    /// Data-model packages and their components.
    pub(crate) data_models: crate::datamodels::DataModelFlags,
    /// Store fuel of every host call.
    pub fuel: u64,
    /// Engine-command budget of each call (`u64::MAX`, no budget, unless
    /// `--max-commands` sets one).
    pub max_commands: u64,
    /// The root `_sessionid`.
    pub session_id: String,
    /// The clock.
    pub clock: ClockMode,
    /// Automatic snapshots.
    pub snapshot_on: SnapshotOn,
    /// Options of every snapshot `run` takes.
    pub options: Options,
    /// End-of-input policy.
    pub on_eof: OnEof,
    /// Where to write the replay log.
    pub record: Option<PathBuf>,
    /// A replay log to reproduce instead of reading standard input.
    pub replay: Option<PathBuf>,
    /// `BasicHTTP` network flags.
    pub http: HttpArgs,
}

/// The `BasicHTTP` network of `run`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HttpArgs {
    /// `--http-allow` entries (origins or URL prefixes), in order.
    pub allow: Vec<String>,
    /// `--http-listen` address.
    pub listen: Option<std::net::SocketAddr>,
    /// `--http-public-url` base.
    pub public_url: Option<String>,
}

impl HttpArgs {
    /// Whether any network flag was given.
    #[must_use]
    pub fn any(&self) -> bool {
        !self.allow.is_empty() || self.listen.is_some() || self.public_url.is_some()
    }
}

/// `PORT` (bound on 127.0.0.1) or `IP:PORT`.
fn listen_address(value: &str) -> Result<std::net::SocketAddr, String> {
    if let Ok(port) = value.parse::<u16>() {
        return Ok(std::net::SocketAddr::new(DEFAULT_LISTEN_IP, port));
    }
    value.parse().map_err(|_| {
        format!("--http-listen requires PORT or IP:PORT (for example 8080 or 127.0.0.1:8080), not `{value}`")
    })
}

/// Marker error: the session already reported its outcome on standard
/// output; only this exit status remains.
#[derive(Debug)]
pub struct Exited(pub u8);

impl std::fmt::Display for Exited {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "the session ended with exit status {}", self.0)
    }
}

impl std::error::Error for Exited {}

fn positive(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .ok_or_else(|| format!("{flag} requires a positive integer"))
}

/// Parses the arguments after `run`.
///
/// # Errors
///
/// A usage message.
#[allow(
    clippy::too_many_lines,
    reason = "one flat table maps every run flag to its field and its usage error"
)]
pub fn parse(args: &[String]) -> Result<RunArgs, String> {
    let parsed = crate::snapshot::split(
        args,
        RUN_OPTIONS,
        &[crate::repository::RepositoryFlags::OFFLINE],
        &|flag| match flag {
            "--components" => "DIR",
            "--components-manifest-sha256" => "HEX",
            "--extension-package" | "--service-package" | "--data-model-package" => "MANIFEST.json",
            "--extension-component-sha256"
            | "--service-component-sha256"
            | "--data-model-component-sha256" => "URI=HEX",
            "--dir" => "URI=HOST::GUEST",
            "--wasi" => "URI=inherit-network or URI=allow-ip-name-lookup",
            "--env" => "NAME or NAME=VALUE",
            "--env-file" => "PATH",
            "--fuel" | "--max-commands" => "N",
            "--session-id" => "NAME",
            "--clock" => "wall or logical",
            "--snapshot-on" => "call, done, or never",
            "--exclude" => "SECTION",
            "--trace-context" => "strip or preserve",
            "--on-eof" => "wait or close",
            "--record" | "--replay" => "FILE",
            "--http-allow" => "ORIGIN or URL-PREFIX",
            flag if crate::repository::RepositoryFlags::expects(flag).is_some() => {
                crate::repository::RepositoryFlags::expects(flag).unwrap_or("a value")
            }
            "--http-listen" => "PORT or IP:PORT",
            "--http-public-url" => "URL",
            _ => "URI=PATH",
        },
    )?;
    let document = crate::snapshot::one_operand(&parsed.operands, "run", "DOC.scx")?;
    let mut components_dir = None;
    let mut manifest = None;
    let mut namespaces = crate::extensions::RunFlags::default();
    let mut services = crate::services::ServiceFlags::default();
    let mut data_models = crate::datamodels::DataModelFlags::default();
    let mut fuel = None;
    let mut max_commands = None;
    let mut session_id = None;
    let mut clock = None;
    let mut snapshot_on = None;
    let mut options = Options::default();
    let mut trace_context = None;
    let mut on_eof = None;
    let mut record = None;
    let mut replay = None;
    let mut http = HttpArgs::default();
    for (flag, value) in parsed.options {
        let value = value.unwrap_or_default();
        match flag {
            "--components" => components_dir = Some(PathBuf::from(value)),
            "--components-manifest-sha256" => manifest = Some(value.to_owned()),
            "--fuel" => fuel = Some(positive(value, "--fuel")?),
            "--max-commands" => max_commands = Some(positive(value, "--max-commands")?),
            "--session-id" => {
                if scxml_types::EventName::new(value.to_owned()).is_err() || value.contains('.') {
                    return Err("--session-id requires a valid SCXML NAME".into());
                }
                session_id = Some(value.to_owned());
            }
            "--clock" => {
                clock = Some(match value {
                    "wall" => ClockMode::Wall,
                    "logical" => ClockMode::Logical,
                    _ => return Err("--clock requires wall or logical".into()),
                });
            }
            "--snapshot-on" => {
                snapshot_on = Some(match value {
                    "call" => SnapshotOn::Call,
                    "done" => SnapshotOn::Done,
                    "never" => SnapshotOn::Never,
                    _ => return Err("--snapshot-on requires call, done, or never".into()),
                });
            }
            "--exclude" => options.exclude.push(crate::snapshot::parse_section(value)?),
            "--trace-context" => {
                trace_context = Some(match value {
                    "strip" => TraceContextPolicy::Strip,
                    "preserve" => TraceContextPolicy::Preserve,
                    _ => return Err("--trace-context requires strip or preserve".into()),
                });
            }
            "--on-eof" => {
                on_eof = Some(match value {
                    "wait" => OnEof::Wait,
                    "close" => OnEof::Close,
                    _ => return Err("--on-eof requires wait or close".into()),
                });
            }
            "--record" => record = Some(PathBuf::from(value)),
            "--replay" => replay = Some(PathBuf::from(value)),
            "--http-allow" => {
                if value.is_empty() {
                    return Err("--http-allow requires an origin or URL prefix".into());
                }
                http.allow.push(value.to_owned());
            }
            "--http-listen" => http.listen = Some(listen_address(value)?),
            "--http-public-url" => http.public_url = Some(value.to_owned()),
            service if crate::services::ServiceFlags::FLAGS.contains(&service) => {
                services.accept(service, value)?;
            }
            data_model if crate::datamodels::DataModelFlags::FLAGS.contains(&data_model) => {
                data_models.accept(data_model, value)?;
            }
            namespace => namespaces.accept(namespace, value)?,
        }
    }
    options.exclude.sort_unstable();
    options.exclude.dedup();
    if let Some(policy) = trace_context {
        options.trace_context = policy;
    }
    if replay.is_some() {
        for (given, flag) in [
            (record.is_some(), "--record"),
            (clock.is_some(), "--clock"),
            (on_eof.is_some(), "--on-eof"),
            (snapshot_on.is_some(), "--snapshot-on"),
            (max_commands.is_some(), "--max-commands"),
            (session_id.is_some(), "--session-id"),
            (fuel.is_some(), "--fuel"),
        ] {
            if given {
                return Err(format!(
                    "--replay reproduces the recorded session; {flag} comes from the log and cannot be given with it"
                ));
            }
        }
        if http.any() {
            return Err(
                "--replay reproduces the recorded session's BasicHTTP network from the log without touching the network; --http-allow, --http-listen, and --http-public-url cannot be given with it"
                    .into(),
            );
        }
    }
    if http.public_url.is_some() && http.listen.is_none() {
        return Err(
            "--http-public-url names where the --http-listen listener is reached; give --http-listen too"
                .into(),
        );
    }
    if http.public_url.is_none() && http.listen.is_some_and(|addr| addr.ip().is_unspecified()) {
        return Err(
            "--http-listen on an unspecified address (0.0.0.0 or ::) needs --http-public-url: sessions must advertise a reachable location"
                .into(),
        );
    }
    let components = ComponentArgs::resolve(components_dir, manifest)?;
    Ok(RunArgs {
        document,
        components,
        namespaces,
        services,
        data_models,
        fuel: fuel.unwrap_or(DEFAULT_FUEL),
        // Without `--max-commands` a call has no command budget: resource
        // limits apply to the Wasm components only, whose own per-call
        // command bound stops every machine call.
        max_commands: max_commands.unwrap_or(u64::MAX),
        session_id: session_id.unwrap_or_else(|| "session".into()),
        clock: clock.unwrap_or(ClockMode::Wall),
        snapshot_on: snapshot_on.unwrap_or(SnapshotOn::Never),
        options,
        on_eof: on_eof.unwrap_or(OnEof::Wait),
        record,
        replay,
        http,
    })
}

/// Default store fuel of every host call (the host's own default).
#[cfg(feature = "wasmtime")]
const DEFAULT_FUEL: u64 = scxml_host_wasmtime::DEFAULT_FUEL;
/// Default store fuel of every host call (the host's own default).
#[cfg(not(feature = "wasmtime"))]
const DEFAULT_FUEL: u64 = 1_000_000_000;

/// Runs `run`.
///
/// # Errors
///
/// [`Exited`] carrying the exit status once the session ran, or any
/// failure before it (usage, compilation, components, I/O).
#[cfg(not(feature = "wasmtime"))]
pub fn execute(_args: &RunArgs) -> Result<(), Box<dyn std::error::Error>> {
    Err(
        "[wasmtime-disabled] run executes only through the shipped components under \
         Wasmtime; rebuild scx with --features wasmtime. No document was executed"
            .into(),
    )
}

#[cfg(feature = "wasmtime")]
pub use live::execute;

/// JSON record rendering shared by standard output and the replay log.
#[cfg(feature = "wasmtime")]
mod json {
    use std::fmt::Write as _;

    /// One JSON object built key by key, in insertion order.
    pub(crate) struct Object(String);

    impl Object {
        pub(crate) fn new() -> Self {
            Self(String::from("{"))
        }

        /// A record whose first key is `"type"`.
        pub(crate) fn record(kind: &str) -> Self {
            Self::new().string("type", kind)
        }

        fn key(mut self, key: &str) -> Self {
            if self.0.len() > 1 {
                self.0.push(',');
            }
            self.0.push_str(&string(key));
            self.0.push(':');
            self
        }

        pub(crate) fn string(self, key: &str, value: &str) -> Self {
            let mut object = self.key(key);
            object.0.push_str(&string(value));
            object
        }

        pub(crate) fn number(self, key: &str, value: u64) -> Self {
            let mut object = self.key(key);
            let _ = write!(object.0, "{value}");
            object
        }

        pub(crate) fn boolean(self, key: &str, value: bool) -> Self {
            let mut object = self.key(key);
            object.0.push_str(if value { "true" } else { "false" });
            object
        }

        /// `value` must already be one JSON value.
        pub(crate) fn raw(self, key: &str, value: &str) -> Self {
            let mut object = self.key(key);
            object.0.push_str(value);
            object
        }

        pub(crate) fn null(self, key: &str) -> Self {
            self.raw(key, "null")
        }

        pub(crate) fn finish(mut self) -> String {
            self.0.push('}');
            self.0
        }
    }

    /// One JSON string literal.
    pub(crate) fn string(value: &str) -> String {
        serde_json::Value::String(value.to_owned()).to_string()
    }

    /// One JSON array of strings.
    pub(crate) fn strings(values: &[String]) -> String {
        let items: Vec<String> = values.iter().map(|value| string(value)).collect();
        format!("[{}]", items.join(","))
    }
}

/// The live driver (`wasmtime` feature).
#[cfg(feature = "wasmtime")]
mod live {
    use std::fs;
    use std::io::{self, BufRead as _, Write as _};
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use scxml_host_wasmtime::{
        Applied, BasicHttpListener, CommunicationSender, HttpAllowlist, HttpClientConfig,
        InboundHttp, InputRecord, ListenerConfig, ListenerHandle, LiveCallResult, LiveClock,
        LiveHttp, LiveSession, LiveStatus, NetworkClient, PublicUrl, RunFailure, ScriptedEvent,
        SessionHostError, SessionOpen, SessionOutput, WallClock,
    };
    use scxml_snapshot::Options;
    use scxml_types::{
        BaggagePolicy, DataValue, Event, EventName, EventType, SendId, TraceContext,
    };
    use sha2::{Digest as _, Sha256};

    use super::json::{self, Object};
    use super::{ClockMode, Exited, HttpArgs, OnEof, RunArgs, SnapshotOn};

    /// The replay log format.
    const LOG_FORMAT: &str = "scxml.live-log/v1";

    impl ClockMode {
        const fn as_str(self) -> &'static str {
            match self {
                Self::Wall => "wall",
                Self::Logical => "logical",
            }
        }
    }

    /// Exit status: the root finished.
    const EXIT_DONE: u8 = 0;
    /// Exit status: the session failed.
    const EXIT_FAILED: u8 = 1;
    /// Exit status: closed while running.
    const EXIT_CLOSED: u8 = 3;

    /// One `data`/`xml` value as record fields: JSON kinds under `data`
    /// (canonical JSON text embedded as is), XML under `xml`; absent and
    /// undefined data add nothing.
    fn with_data(object: Object, value: &DataValue) -> Object {
        match value {
            DataValue::Absent | DataValue::Undefined => object,
            DataValue::Null => object.null("data"),
            DataValue::Bool(value) => object.boolean("data", *value),
            DataValue::Number(number) => match scxml_snapshot::Value::number(number.get()) {
                scxml_snapshot::Value::Number(text) => object.raw("data", &text),
                _ => object.null("data"),
            },
            DataValue::String(text) => object.string("data", text),
            DataValue::JsonArray(text) | DataValue::JsonObject(text) => object.raw("data", text),
            DataValue::Xml(text) => object.string("xml", text),
        }
    }

    /// Reads `data` or `xml` of one input or log object.
    fn read_data(fields: &serde_json::Map<String, serde_json::Value>) -> Result<DataValue, String> {
        match (fields.get("data"), fields.get("xml")) {
            (Some(_), Some(_)) => Err("`data` and `xml` are mutually exclusive".into()),
            (None, None) => Ok(DataValue::Absent),
            (None, Some(serde_json::Value::String(xml))) => Ok(DataValue::Xml(xml.clone())),
            (None, Some(_)) => Err("`xml` must be a string".into()),
            (Some(value), None) => json_data(value),
        }
    }

    fn json_data(value: &serde_json::Value) -> Result<DataValue, String> {
        let invalid = |error: scxml_types::DataValueError| format!("`data` is invalid: {error}");
        Ok(match value {
            serde_json::Value::Null => DataValue::Null,
            serde_json::Value::Bool(value) => DataValue::Bool(*value),
            serde_json::Value::Number(number) => DataValue::Number(
                number
                    .as_f64()
                    .ok_or_else(|| "`data` number is out of range".to_owned())
                    .and_then(|value| scxml_types::FiniteNumber::new(value).map_err(invalid))?,
            ),
            serde_json::Value::String(text) => DataValue::String(text.clone()),
            serde_json::Value::Array(_) => {
                DataValue::canonical_json_array(&value.to_string()).map_err(invalid)?
            }
            serde_json::Value::Object(_) => {
                DataValue::canonical_json_object(&value.to_string()).map_err(invalid)?
            }
        })
    }

    /// One parsed standard-input line.
    enum Input {
        Event(ScriptedEvent),
        Snapshot,
        Close,
    }

    fn parse_input(line: &str) -> Result<Input, String> {
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|error| format!("not JSON: {error}"))?;
        let serde_json::Value::Object(fields) = value else {
            return Err("an input line must be a JSON object".into());
        };
        let kind = match fields.get("type") {
            None => "event",
            Some(serde_json::Value::String(kind)) => kind.as_str(),
            Some(_) => return Err("`type` must be a string".into()),
        };
        let allowed: &[&str] = match kind {
            "event" => &["type", "name", "data", "xml"],
            "snapshot" | "close" => &["type"],
            other => {
                return Err(format!(
                    "unknown input type `{other}` (expected event, snapshot, or close)"
                ));
            }
        };
        if let Some(unknown) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(format!("unknown field `{unknown}` in a `{kind}` input"));
        }
        match kind {
            "snapshot" => Ok(Input::Snapshot),
            "close" => Ok(Input::Close),
            _ => {
                let Some(serde_json::Value::String(name)) = fields.get("name") else {
                    return Err("an event needs a string `name`".into());
                };
                Ok(Input::Event(ScriptedEvent {
                    data: read_data(&fields)?,
                    ..ScriptedEvent::named(name.clone())
                }))
            }
        }
    }

    /// What the standard-input, signal, and `BasicHTTP` threads deliver.
    enum Message {
        /// One line (1-based number) or why it was unusable.
        Line(u64, Result<String, String>),
        /// Standard input ended.
        Eof,
        /// Reading standard input failed.
        ReadFailed(String),
        /// A termination signal.
        Signal(i32),
        /// One inbound `BasicHTTP` message from the listener.
        Http(InboundHttp),
        /// An accepted outbound `BasicHTTP` request settled.
        CommunicationSettled,
        /// The HTTP response of an invoked service's request arrived.
        HttpResponse,
    }

    /// Reads standard input line by line into `sender`.
    fn read_stdin(sender: &mpsc::Sender<Message>) {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut number = 0_u64;
        loop {
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => {
                    let _ = sender.send(Message::Eof);
                    return;
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = sender.send(Message::ReadFailed(error.to_string()));
                    return;
                }
            }
            let ended = line.last() != Some(&b'\n');
            if !ended {
                line.pop();
            }
            number += 1;
            let message = String::from_utf8(line).map_err(|_| "the line is not UTF-8".to_owned());
            if sender.send(Message::Line(number, message)).is_err() {
                return;
            }
            if ended {
                let _ = sender.send(Message::Eof);
                return;
            }
        }
    }

    /// Forwards `SIGINT` and `SIGTERM` into `sender`.
    #[cfg(unix)]
    fn watch_signals(sender: mpsc::Sender<Message>) -> Result<(), io::Error> {
        use signal_hook::consts::{SIGINT, SIGTERM};
        let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM])?;
        std::thread::Builder::new()
            .name("scxml-run-signals".into())
            .spawn(move || {
                for signal in signals.forever() {
                    if sender.send(Message::Signal(signal)).is_err() {
                        return;
                    }
                }
            })?;
        Ok(())
    }

    /// No portable termination signals outside Unix.
    #[cfg(not(unix))]
    fn watch_signals(_sender: mpsc::Sender<Message>) -> Result<(), io::Error> {
        Ok(())
    }

    /// The stable code and message of one session failure.
    fn failure_code(failure: &RunFailure) -> String {
        match failure {
            RunFailure::CommandBudget => "command-budget".into(),
            RunFailure::CommandBound => "command-bound".into(),
            RunFailure::Machine { code, .. } => format!("machine-{code}"),
            RunFailure::Harness { code, .. } => (*code).to_owned(),
            RunFailure::Host(_) => "host-failure".into(),
            RunFailure::Snapshot(_) => "snapshot-failed".into(),
        }
    }

    /// Whether an `error` record ends the session.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Severity {
        /// The session failed: an `error`, and `fatal` is true.
        Fatal,
        /// The session goes on: a `warning`, and `fatal` is false.
        Recoverable,
    }

    /// Standard output (records) and the optional replay log.
    struct Sink {
        stdout: io::Stdout,
        log: Option<io::BufWriter<fs::File>>,
    }

    impl Sink {
        fn emit(&mut self, line: &str) -> io::Result<()> {
            let mut stdout = self.stdout.lock();
            stdout.write_all(crate::mask::scrub_json(line).as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()
        }

        fn log(&mut self, line: &str) -> io::Result<()> {
            if let Some(log) = &mut self.log {
                log.write_all(line.as_bytes())?;
                log.write_all(b"\n")?;
                log.flush()?;
            }
            Ok(())
        }

        fn outputs(&mut self, outputs: Vec<SessionOutput>) -> io::Result<()> {
            for output in outputs {
                let line = match output {
                    SessionOutput::Log {
                        session,
                        parent,
                        entry,
                    } => {
                        let mut object = Object::record("log").string("session", &session);
                        if let Some(parent) = &parent {
                            object = object.string("parent", parent);
                        }
                        if let Some(label) = &entry.label {
                            object = object.string("label", label);
                        }
                        object.string("text", &entry.text).finish()
                    }
                    SessionOutput::Done { data } => match data {
                        Some(data) => with_data(Object::record("done"), &data).finish(),
                        None => Object::record("done").finish(),
                    },
                };
                self.emit(&line)?;
            }
            Ok(())
        }

        fn error(
            &mut self,
            code: &str,
            message: &str,
            severity: Severity,
            line: Option<u64>,
        ) -> io::Result<()> {
            let fatal = severity == Severity::Fatal;
            let severity = if fatal { "error" } else { "warning" };
            let location = line
                .map(|line| format!("stdin line {line}: "))
                .unwrap_or_default();
            let _ = writeln!(
                io::stderr(),
                "{severity}[{code}]: {location}{}",
                crate::mask::scrub(message)
            );
            let mut object = Object::record("error")
                .string("code", code)
                .string("message", message)
                .boolean("fatal", fatal);
            if let Some(line) = line {
                object = object.number("line", line);
            }
            self.emit(&object.finish())
        }

        // A snapshot record carries a canonical document whose `sha256`
        // covers its bytes: it is written exactly (never masked), with the
        // protections of the canonical artifacts it is.
        fn snapshot(&mut self, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
            let text = std::str::from_utf8(bytes)?.trim_end();
            let line = Object::record("snapshot").raw("document", text).finish();
            let mut stdout = self.stdout.lock();
            stdout.write_all(line.as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            Ok(())
        }
    }

    /// One recorded input as a log line.
    fn log_line(record: &InputRecord) -> Result<String, Box<dyn std::error::Error>> {
        Ok(match record {
            InputRecord::Start { epoch_ms } => Object::record("start")
                .number("epoch_ms", *epoch_ms)
                .finish(),
            InputRecord::Event { at_ms, event } => with_data(
                Object::record("event")
                    .number("at_ms", *at_ms)
                    .string("name", &event.name),
                &event.data,
            )
            .finish(),
            InputRecord::Advance { to_ms } => {
                Object::record("advance").number("to_ms", *to_ms).finish()
            }
            InputRecord::Idle => Object::record("idle").finish(),
            InputRecord::Snapshot { options } => Object::record("snapshot")
                .raw("options", &serde_json::to_string(options)?)
                .finish(),
            InputRecord::Http {
                at_ms,
                session_id,
                path,
                event,
                status,
            } => Object::record("http")
                .number("at_ms", *at_ms)
                .string("session", session_id)
                .raw("path", &json::strings(path))
                .number("status", u64::from(*status))
                .raw("event", &event_object(event))
                .finish(),
            InputRecord::CommunicationErrors { at_ms, senders } => {
                let senders: Vec<String> = senders
                    .iter()
                    .map(|sender| {
                        Object::new()
                            .string("session", &sender.session_id)
                            .raw("path", &json::strings(&sender.path))
                            .string("sendid", &sender.send_id)
                            .finish()
                    })
                    .collect();
                Object::record("communication-error")
                    .number("at_ms", *at_ms)
                    .raw("senders", &format!("[{}]", senders.join(",")))
                    .finish()
            }
            InputRecord::OutboundHttp { accepted } => Object::record("outbound")
                .boolean("accepted", *accepted)
                .finish(),
            InputRecord::HttpResponse {
                at_ms,
                path,
                request,
                response,
            } => {
                let object = Object::record("http-response")
                    .number("at_ms", *at_ms)
                    .raw("path", &json::strings(path))
                    .number("request", *request);
                match response {
                    Ok(response) => {
                        let object = object.number("status", u64::from(response.status));
                        // A UTF-8 body is written as text, any other as hex.
                        match std::str::from_utf8(&response.body) {
                            Ok(text) => object.string("body", text),
                            Err(_) => object.string("body_hex", &hex_encode(&response.body)),
                        }
                        .finish()
                    }
                    Err(failure) => object.string("failure", failure_name(*failure)).finish(),
                }
            }
            InputRecord::HttpResponsePart {
                at_ms,
                path,
                request,
                part,
            } => {
                let object = Object::record("http-response-part")
                    .number("at_ms", *at_ms)
                    .raw("path", &json::strings(path))
                    .number("request", *request);
                match part {
                    Ok(scxml_host_wasmtime::ExtensionHttpPart::Status(status)) => {
                        object.number("status", u64::from(*status)).finish()
                    }
                    // A UTF-8 body part is written as text, any other as hex.
                    Ok(scxml_host_wasmtime::ExtensionHttpPart::Body(body)) => {
                        match std::str::from_utf8(body) {
                            Ok(text) => object.string("body", text),
                            Err(_) => object.string("body_hex", &hex_encode(body)),
                        }
                        .finish()
                    }
                    Ok(scxml_host_wasmtime::ExtensionHttpPart::End) => {
                        object.boolean("end", true).finish()
                    }
                    Err(failure) => object.string("failure", failure_name(*failure)).finish(),
                }
            }
        })
    }

    /// The record name of a transport failure.
    const fn failure_name(failure: scxml_host_wasmtime::ExtensionHttpFailure) -> &'static str {
        match failure {
            scxml_host_wasmtime::ExtensionHttpFailure::NotAllowed => "not-allowed",
            scxml_host_wasmtime::ExtensionHttpFailure::Transport => "transport",
            scxml_host_wasmtime::ExtensionHttpFailure::ResponseTooLarge => "response-too-large",
        }
    }

    /// The transport failure a record names.
    fn read_failure(failure: &str) -> Result<scxml_host_wasmtime::ExtensionHttpFailure, String> {
        Ok(match failure {
            "not-allowed" => scxml_host_wasmtime::ExtensionHttpFailure::NotAllowed,
            "transport" => scxml_host_wasmtime::ExtensionHttpFailure::Transport,
            "response-too-large" => scxml_host_wasmtime::ExtensionHttpFailure::ResponseTooLarge,
            _ => return Err("an http-response `failure` is not a failure".into()),
        })
    }

    /// `bytes` as lowercase hex.
    fn hex_encode(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
    }

    /// Lowercase or uppercase hex as bytes; `None` for anything else.
    fn hex_decode(text: &str) -> Option<Vec<u8>> {
        if !text.len().is_multiple_of(2) {
            return None;
        }
        text.as_bytes()
            .chunks(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            })
            .collect()
    }

    /// One recorded inbound `BasicHTTP` event (always `external`, without
    /// an invoke id) as a JSON object.
    fn event_object(event: &Event) -> String {
        let mut object = Object::new().string("name", event.name.as_str());
        if let Some(send_id) = &event.send_id {
            object = object.string("sendid", send_id.as_str());
        }
        if let Some(origin) = &event.origin {
            object = object.string("origin", origin);
        }
        if let Some(origin_type) = &event.origin_type {
            object = object.string("origintype", origin_type);
        }
        object = with_data(object, &event.data);
        if let Some(raw) = &event.raw {
            object = object.string("raw", raw);
        }
        if let Some(context) = &event.trace_context {
            let mut carrier = Object::new().string("traceparent", &context.traceparent);
            if let Some(tracestate) = &context.tracestate {
                carrier = carrier.string("tracestate", tracestate);
            }
            if let Some(baggage) = &context.baggage {
                carrier = carrier.string("baggage", baggage);
            }
            carrier = carrier.string(
                "baggage_policy",
                match context.baggage_policy {
                    BaggagePolicy::Drop => "drop",
                    BaggagePolicy::Propagate => "propagate",
                },
            );
            object = object.raw("trace_context", &carrier.finish());
        }
        object.finish()
    }

    /// Reads one recorded inbound event (see [`event_object`]).
    fn read_event(value: Option<&serde_json::Value>) -> Result<Event, String> {
        let Some(serde_json::Value::Object(fields)) = value else {
            return Err("an http record needs an `event` object".into());
        };
        if let Some(unknown) = fields.keys().find(|key| {
            ![
                "name",
                "sendid",
                "origin",
                "origintype",
                "data",
                "xml",
                "raw",
                "trace_context",
            ]
            .contains(&key.as_str())
        }) {
            return Err(format!("unknown event field `{unknown}`"));
        }
        let text = |key: &str| -> Result<Option<String>, String> {
            match fields.get(key) {
                None => Ok(None),
                Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
                Some(_) => Err(format!("event `{key}` must be a string")),
            }
        };
        let name = text("name")?.ok_or("an event needs a string `name`")?;
        let trace_context = match fields.get("trace_context") {
            None => None,
            Some(serde_json::Value::Object(carrier)) => {
                if let Some(unknown) = carrier.keys().find(|key| {
                    !["traceparent", "tracestate", "baggage", "baggage_policy"]
                        .contains(&key.as_str())
                }) {
                    return Err(format!("unknown trace_context field `{unknown}`"));
                }
                let field = |key: &str| -> Result<Option<String>, String> {
                    match carrier.get(key) {
                        None => Ok(None),
                        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
                        Some(_) => Err(format!("trace_context `{key}` must be a string")),
                    }
                };
                let policy = match field("baggage_policy")?.as_deref() {
                    Some("drop") => BaggagePolicy::Drop,
                    Some("propagate") => BaggagePolicy::Propagate,
                    _ => {
                        return Err(
                            "trace_context `baggage_policy` must be drop or propagate".into()
                        );
                    }
                };
                Some(
                    TraceContext::new(
                        field("traceparent")?.ok_or("trace_context needs a `traceparent`")?,
                        field("tracestate")?,
                        field("baggage")?,
                        policy,
                    )
                    .map_err(|error| format!("trace_context is invalid: {error}"))?,
                )
            }
            Some(_) => return Err("event `trace_context` must be an object".into()),
        };
        let event = Event {
            name: EventName::new(name)
                .map_err(|error| format!("event name is invalid: {error}"))?,
            event_type: EventType::External,
            send_id: text("sendid")?
                .map(SendId::new)
                .transpose()
                .map_err(|error| format!("event sendid is invalid: {error}"))?,
            origin: text("origin")?,
            origin_type: text("origintype")?,
            invoke_id: None,
            data: read_data(fields)?,
            trace_context,
            raw: text("raw")?,
        };
        event
            .validate()
            .map_err(|error| format!("the event is invalid: {error:?}"))?;
        Ok(event)
    }

    /// Reads one record's `path`: the invoke ids from the root to the
    /// session it names (its lineage; empty for the root), each a valid
    /// invoke id. Replay delivers only to a session of exactly this
    /// lineage.
    fn read_path(value: Option<&serde_json::Value>) -> Result<Vec<String>, String> {
        let Some(serde_json::Value::Array(path)) = value else {
            return Err("a session needs a `path` array".into());
        };
        path.iter()
            .map(|id| {
                id.as_str()
                    .filter(|id| scxml_types::InvokeId::new(*id).is_ok())
                    .map(str::to_owned)
                    .ok_or_else(|| "a `path` holds invoke ids".to_owned())
            })
            .collect()
    }

    /// Reads the senders of one `communication-error` record.
    fn read_senders(value: Option<&serde_json::Value>) -> Result<Vec<CommunicationSender>, String> {
        let Some(serde_json::Value::Array(entries)) = value else {
            return Err("`senders` must be an array".into());
        };
        entries
            .iter()
            .map(|entry| {
                let serde_json::Value::Object(fields) = entry else {
                    return Err("a sender must be an object".to_owned());
                };
                if let Some(unknown) = fields
                    .keys()
                    .find(|key| !["session", "path", "sendid"].contains(&key.as_str()))
                {
                    return Err(format!("unknown sender field `{unknown}`"));
                }
                let Some(serde_json::Value::String(session_id)) = fields.get("session") else {
                    return Err("a sender needs a string `session`".into());
                };
                let path = read_path(fields.get("path"))?;
                // The failed `<send>`'s id, carried as `_event.sendid`.
                let Some(serde_json::Value::String(send_id)) = fields.get("sendid") else {
                    return Err("a sender needs a string `sendid`".into());
                };
                if scxml_types::SendId::new(send_id.clone()).is_err() {
                    return Err("a sender `sendid` must be a valid send id".into());
                }
                Ok(CommunicationSender {
                    session_id: session_id.clone(),
                    path,
                    send_id: send_id.clone(),
                })
            })
            .collect()
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one match over the closed record types"
    )]
    fn log_record(
        fields: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<InputRecord>, String> {
        let kind = fields.get("type").and_then(serde_json::Value::as_str);
        let number = |key: &str| {
            fields
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("`{key}` must be an unsigned integer"))
        };
        let only =
            |allowed: &[&str]| match fields.keys().find(|key| !allowed.contains(&key.as_str())) {
                Some(unknown) => Err(format!("unknown field `{unknown}`")),
                None => Ok(()),
            };
        match kind {
            Some("start") => {
                only(&["type", "epoch_ms"])?;
                Ok(Some(InputRecord::Start {
                    epoch_ms: number("epoch_ms")?,
                }))
            }
            Some("event") => {
                only(&["type", "at_ms", "name", "data", "xml"])?;
                let Some(serde_json::Value::String(name)) = fields.get("name") else {
                    return Err("an event needs a string `name`".into());
                };
                Ok(Some(InputRecord::Event {
                    at_ms: number("at_ms")?,
                    event: ScriptedEvent {
                        data: read_data(fields)?,
                        ..ScriptedEvent::named(name.clone())
                    },
                }))
            }
            Some("advance") => {
                only(&["type", "to_ms"])?;
                Ok(Some(InputRecord::Advance {
                    to_ms: number("to_ms")?,
                }))
            }
            Some("idle") => {
                only(&["type"])?;
                Ok(Some(InputRecord::Idle))
            }
            Some("snapshot") => {
                only(&["type", "options"])?;
                let options: Options = serde_json::from_value(
                    fields
                        .get("options")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .map_err(|error| format!("`options` is invalid: {error}"))?;
                Ok(Some(InputRecord::Snapshot { options }))
            }
            Some("http") => {
                only(&["type", "at_ms", "session", "path", "status", "event"])?;
                let Some(serde_json::Value::String(session_id)) = fields.get("session") else {
                    return Err("an http record needs a string `session`".into());
                };
                let path = read_path(fields.get("path"))?;
                let status = u16::try_from(number("status")?)
                    .map_err(|_| "`status` must be an HTTP status".to_owned())?;
                Ok(Some(InputRecord::Http {
                    at_ms: number("at_ms")?,
                    session_id: session_id.clone(),
                    path,
                    event: read_event(fields.get("event"))?,
                    status,
                }))
            }
            Some("communication-error") => {
                only(&["type", "at_ms", "senders"])?;
                Ok(Some(InputRecord::CommunicationErrors {
                    at_ms: number("at_ms")?,
                    senders: read_senders(fields.get("senders"))?,
                }))
            }
            Some("outbound") => {
                only(&["type", "accepted"])?;
                let Some(serde_json::Value::Bool(accepted)) = fields.get("accepted") else {
                    return Err("an outbound record needs a boolean `accepted`".into());
                };
                Ok(Some(InputRecord::OutboundHttp {
                    accepted: *accepted,
                }))
            }
            Some("http-response") => {
                only(&[
                    "type", "at_ms", "path", "request", "status", "body", "body_hex", "failure",
                ])?;
                let path = read_path(fields.get("path"))?;
                let response = match (fields.get("failure"), fields.get("status")) {
                    (Some(serde_json::Value::String(failure)), None) => Err(read_failure(failure)?),
                    (None, Some(_)) => {
                        let status = u16::try_from(number("status")?)
                            .map_err(|_| "`status` must be an HTTP status".to_owned())?;
                        let body = match (fields.get("body"), fields.get("body_hex")) {
                            (Some(serde_json::Value::String(body)), None) => {
                                body.as_bytes().to_vec()
                            }
                            (None, Some(serde_json::Value::String(body))) => {
                                hex_decode(body).ok_or("an http-response `body_hex` is not hex")?
                            }
                            _ => {
                                return Err(
                                    "an http-response needs exactly one of `body` and `body_hex`"
                                        .into(),
                                );
                            }
                        };
                        Ok(scxml_host_wasmtime::ExtensionHttpResponse { status, body })
                    }
                    _ => {
                        return Err(
                            "an http-response needs exactly one of `status` and `failure`".into(),
                        );
                    }
                };
                Ok(Some(InputRecord::HttpResponse {
                    at_ms: number("at_ms")?,
                    path,
                    request: number("request")?,
                    response,
                }))
            }
            Some("http-response-part") => {
                only(&[
                    "type", "at_ms", "path", "request", "status", "body", "body_hex", "end",
                    "failure",
                ])?;
                let path = read_path(fields.get("path"))?;
                let part = match (
                    fields.get("status"),
                    fields.get("body"),
                    fields.get("body_hex"),
                    fields.get("end"),
                    fields.get("failure"),
                ) {
                    (Some(_), None, None, None, None) => {
                        Ok(scxml_host_wasmtime::ExtensionHttpPart::Status(
                            u16::try_from(number("status")?)
                                .map_err(|_| "`status` must be an HTTP status".to_owned())?,
                        ))
                    }
                    (None, Some(serde_json::Value::String(body)), None, None, None) => Ok(
                        scxml_host_wasmtime::ExtensionHttpPart::Body(body.as_bytes().to_vec()),
                    ),
                    (None, None, Some(serde_json::Value::String(body)), None, None) => {
                        Ok(scxml_host_wasmtime::ExtensionHttpPart::Body(
                            hex_decode(body)
                                .ok_or("an http-response-part `body_hex` is not hex")?,
                        ))
                    }
                    (None, None, None, Some(serde_json::Value::Bool(true)), None) => {
                        Ok(scxml_host_wasmtime::ExtensionHttpPart::End)
                    }
                    (None, None, None, None, Some(serde_json::Value::String(failure))) => {
                        Err(read_failure(failure)?)
                    }
                    _ => {
                        return Err(
                            "an http-response-part needs exactly one of `status`, `body`, `body_hex`, `end` (true), and `failure`"
                                .into(),
                        );
                    }
                };
                Ok(Some(InputRecord::HttpResponsePart {
                    at_ms: number("at_ms")?,
                    path,
                    request: number("request")?,
                    part,
                }))
            }
            Some("close") => {
                only(&["type", "reason"])?;
                Ok(None)
            }
            _ => Err("unknown record type".into()),
        }
    }

    /// The session identity a replay log binds.
    struct Identity {
        ir_sha256: String,
        components_manifest_sha256: String,
        session_id: String,
        max_commands: u64,
        fuel: u64,
        clock: &'static str,
        namespaces: Vec<(String, String)>,
        /// Each bound invoked service's URI and component SHA-256, sorted.
        services: Vec<(String, String)>,
        /// The advertised `BasicHTTP` base (`--http-listen`); every
        /// session's location, so replay advertises the same one.
        http_public_url: Option<String>,
    }

    impl Identity {
        fn header(&self) -> String {
            let namespaces: Vec<String> = self
                .namespaces
                .iter()
                .map(|(uri, sha256)| {
                    Object::new()
                        .string("uri", uri)
                        .string("extension_component_sha256", sha256)
                        .finish()
                })
                .collect();
            let services: Vec<String> = self
                .services
                .iter()
                .map(|(uri, sha256)| {
                    Object::new()
                        .string("uri", uri)
                        .string("service_component_sha256", sha256)
                        .finish()
                })
                .collect();
            let header = Object::record("header")
                .string("format", LOG_FORMAT)
                .string("ir_sha256", &self.ir_sha256)
                .string(
                    "components_manifest_sha256",
                    &self.components_manifest_sha256,
                )
                .string("session_id", &self.session_id)
                .number("max_commands", self.max_commands)
                .number("fuel", self.fuel)
                .string("clock", self.clock)
                .raw("namespaces", &format!("[{}]", namespaces.join(",")))
                .raw("services", &format!("[{}]", services.join(",")));
            match &self.http_public_url {
                Some(url) => header.string("http_public_url", url),
                None => header.null("http_public_url"),
            }
            .finish()
        }

        /// What of the component bindings a log `header` records differs
        /// from this identity's: its namespace extension components or its
        /// invoked service components.
        fn binding_mismatch(&self, header: &serde_json::Value) -> Option<&'static str> {
            let listed = |bindings: &[(String, String)], digest: &str| {
                serde_json::Value::Array(
                    bindings
                        .iter()
                        .map(|(uri, sha256)| serde_json::json!({"uri": uri, digest: sha256}))
                        .collect(),
                )
            };
            if header.get("namespaces")
                != Some(&listed(&self.namespaces, "extension_component_sha256"))
            {
                return Some("set of namespace extension components");
            }
            if header.get("services") != Some(&listed(&self.services, "service_component_sha256")) {
                return Some("set of invoked service components");
            }
            None
        }
    }

    /// A parsed replay log.
    struct Log {
        session_id: String,
        max_commands: u64,
        fuel: u64,
        http_public_url: Option<String>,
        records: Vec<InputRecord>,
    }

    fn read_log(path: &Path, identity: &Identity) -> Result<Log, Box<dyn std::error::Error>> {
        let unreadable = |error: &dyn std::fmt::Display| {
            format!(
                "[replay-unreadable] cannot read the replay log `{}`: {error}",
                path.display()
            )
        };
        let (mut file, metadata) = crate::open_regular_file(path)
            .map_err(|error| unreadable(&error))?
            .ok_or_else(|| unreadable(&"not a regular file"))?;
        // The native CLI reads a log of any size, as it reads a source.
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
        std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|error| unreadable(&error))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("[replay-invalid] `{}` is not UTF-8", path.display()))?;
        let invalid = |line: usize, message: &str| {
            format!("[replay-invalid] {}:{line}: {message}", path.display())
        };
        let mut lines = text
            .lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line));
        let (_, header) = lines.next().ok_or_else(|| invalid(1, "the log is empty"))?;
        let header: serde_json::Value =
            serde_json::from_str(header).map_err(|error| invalid(1, &error.to_string()))?;
        let field = |key: &str| header.get(key).and_then(serde_json::Value::as_str);
        if header.get("type").and_then(serde_json::Value::as_str) != Some("header")
            || field("format") != Some(LOG_FORMAT)
        {
            return Err(invalid(
                1,
                &format!("the first line must be a `{LOG_FORMAT}` header"),
            )
            .into());
        }
        let session_id = field("session_id")
            .ok_or_else(|| invalid(1, "`session_id` is missing"))?
            .to_owned();
        let max_commands = header
            .get("max_commands")
            .and_then(serde_json::Value::as_u64)
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid(1, "`max_commands` is missing"))?;
        let fuel = header
            .get("fuel")
            .and_then(serde_json::Value::as_u64)
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid(1, "`fuel` is missing"))?;
        let http_public_url = match header.get("http_public_url") {
            Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(url)) => Some(url.clone()),
            _ => {
                return Err(invalid(1, "`http_public_url` must be a string or null").into());
            }
        };
        let mismatch = |what: &str| {
            format!(
                "[replay-identity-mismatch] the replay log `{}` was recorded with a different {what}; replay needs the same document, components, namespace bindings, and service bindings",
                path.display()
            )
        };
        if field("ir_sha256") != Some(identity.ir_sha256.as_str()) {
            return Err(mismatch("document (lowered Machine IR)").into());
        }
        if field("components_manifest_sha256") != Some(identity.components_manifest_sha256.as_str())
        {
            return Err(mismatch("component release").into());
        }
        if let Some(what) = identity.binding_mismatch(&header) {
            return Err(mismatch(what).into());
        }
        let mut records = Vec::new();
        for (number, line) in lines {
            if line.is_empty() {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_str(line).map_err(|error| invalid(number, &error.to_string()))?;
            let serde_json::Value::Object(fields) = value else {
                return Err(invalid(number, "a record must be a JSON object").into());
            };
            match log_record(&fields).map_err(|message| invalid(number, &message))? {
                Some(record) => records.push(record),
                None => break,
            }
        }
        Ok(Log {
            session_id,
            max_commands,
            fuel,
            http_public_url,
            records,
        })
    }

    /// The live session and its output.
    struct Driver {
        session: LiveSession,
        sink: Sink,
        clock: ClockMode,
        snapshot_on: SnapshotOn,
        options: Options,
        snapshot_done: bool,
        /// Every input thread's channel (standard input, signals, and the
        /// `BasicHTTP` listener and client).
        messages: (mpsc::Sender<Message>, mpsc::Receiver<Message>),
        /// The serving `BasicHTTP` listener, if any: the session then stays
        /// open after the end of input; a panicked listener thread fails
        /// the run, and closing stops it.
        listener: Option<ListenerHandle>,
    }

    /// How often a loop with a serving listener checks it.
    const LISTENER_CHECK: Duration = Duration::from_millis(250);

    /// Whether a `BasicHTTP` listener serves the session.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Listening {
        Serving,
        Absent,
    }

    /// How long one wait for input lasts when the next timer is due in
    /// `due`.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Window {
        /// Until the timer is due.
        UntilDue(Duration),
        /// A [`LISTENER_CHECK`] slice, after which the listener is checked
        /// and the wait recomputed: a panicked listener is reported within
        /// a slice even while a distant timer is pending.
        Slice(Duration),
    }

    /// What one wait for input ended with.
    enum Received {
        /// An input arrived.
        Message(Message),
        /// The next timer is due.
        TimerDue,
        /// A listener slice ended: check the listener and wait again.
        Recheck,
    }

    /// The wait before the next input or timer, sliced while a listener
    /// serves.
    fn receive_window(due: Duration, listening: Listening) -> Window {
        match listening {
            Listening::Serving if due > LISTENER_CHECK => Window::Slice(LISTENER_CHECK),
            Listening::Serving | Listening::Absent => Window::UntilDue(due),
        }
    }

    /// Why a driver loop ended.
    #[derive(Clone, Copy)]
    enum End {
        Finished,
        Failed,
        Closed(&'static str, u8),
    }

    impl Driver {
        fn flush(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            let outputs = self.session.take_outputs();
            self.sink.outputs(outputs)?;
            for record in self.session.take_recorded() {
                let line = log_line(&record)?;
                self.sink.log(&line)?;
            }
            Ok(())
        }

        /// Reports one failed call: `Ok(true)` when the session failed.
        fn failure(
            &mut self,
            error: &SessionHostError,
            line: Option<u64>,
        ) -> Result<bool, Box<dyn std::error::Error>> {
            self.flush()?;
            let fatal = self.session.status() == LiveStatus::Failed;
            let severity = if fatal {
                Severity::Fatal
            } else {
                Severity::Recoverable
            };
            self.sink.error(
                &failure_code(&error.failure),
                &error.to_string(),
                severity,
                line,
            )?;
            Ok(fatal)
        }

        fn call_record(
            &mut self,
            cause: &str,
            step: &LiveCallResult,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.flush()?;
            let configuration = match self.session.configuration() {
                Ok(configuration) => configuration,
                Err(error) => {
                    self.failure(&error, None)?;
                    return Ok(());
                }
            };
            let status = match step.status {
                LiveStatus::Running => "running",
                LiveStatus::Finished => "finished",
                LiveStatus::Failed => "failed",
            };
            let mut object = Object::record("call")
                .string("cause", cause)
                .number("at_ms", step.at_ms)
                .string("status", status)
                .raw("configuration", &json::strings(&configuration));
            object = match step.next_deadline {
                Some(deadline) => object.number("next_deadline_ms", deadline),
                None => object.null("next_deadline_ms"),
            };
            self.sink
                .emit(&object.number("commands", step.commands).finish())?;
            let finished = step.status == LiveStatus::Finished;
            if self.snapshot_on == SnapshotOn::Call
                || (self.snapshot_on == SnapshotOn::Done && finished && !self.snapshot_done)
            {
                self.snapshot_done |= finished;
                self.snapshot(None)?;
            }
            Ok(())
        }

        fn snapshot(&mut self, line: Option<u64>) -> Result<bool, Box<dyn std::error::Error>> {
            let options = self.options.clone();
            match self.session.snapshot(&options) {
                Ok(bytes) => {
                    self.flush()?;
                    self.sink.snapshot(&bytes)?;
                    Ok(false)
                }
                Err(error) => self.failure(&error, line),
            }
        }

        /// On the logical clock every pending delayed send fires right
        /// after an input's call, folded into its record.
        fn settle(
            &mut self,
            result: Result<LiveCallResult, SessionHostError>,
        ) -> Result<LiveCallResult, SessionHostError> {
            match (result, self.clock) {
                (Ok(step), ClockMode::Logical) if step.status == LiveStatus::Running => {
                    self.session.run_until_idle().map(|idle| LiveCallResult {
                        at_ms: step.at_ms,
                        commands: step.commands.saturating_add(idle.commands),
                        ..idle
                    })
                }
                (result, _) => result,
            }
        }

        /// Writes the record of one input's call, or its failure.
        fn outcome(
            &mut self,
            cause: &str,
            result: Result<LiveCallResult, SessionHostError>,
            line: Option<u64>,
        ) -> Result<Option<End>, Box<dyn std::error::Error>> {
            match self.settle(result) {
                Ok(step) => {
                    self.call_record(cause, &step)?;
                    Ok(self.ended())
                }
                Err(error) => Ok(self.failure(&error, line)?.then_some(End::Failed)),
            }
        }

        /// One event: processed at its arrival; on the logical clock every
        /// pending delayed send then fires.
        fn event(
            &mut self,
            event: &ScriptedEvent,
            line: Option<u64>,
        ) -> Result<Option<End>, Box<dyn std::error::Error>> {
            let sent = self.session.send(event);
            self.outcome("event", sent, line)
        }

        /// One inbound `BasicHTTP` message: the component's response goes
        /// back to the client, and an enqueued event's call is recorded as
        /// cause `http`.
        fn http(
            &mut self,
            inbound: InboundHttp,
        ) -> Result<Option<End>, Box<dyn std::error::Error>> {
            // The listener already answered 503 for a message the session
            // did not take in time: it is dropped, never an event.
            let Some(inbound) = inbound.claim() else {
                return Ok(None);
            };
            let receipt = match inbound.request() {
                Ok(request) => self.session.receive_http(request),
                Err(rejection) => {
                    let response = self.session.http_rejection(rejection);
                    match response {
                        Ok(response) => inbound.respond(response),
                        // Dropped unanswered: the listener answers 503.
                        Err(error) => {
                            drop(inbound);
                            self.failure(&error, None)?;
                        }
                    }
                    return Ok(None);
                }
            };
            inbound.respond(receipt.response);
            match receipt.result {
                Ok(None) => {
                    self.flush()?;
                    Ok(self.ended())
                }
                Ok(Some(step)) => self.outcome("http", Ok(step), None),
                Err(error) => self.outcome("http", Err(error), None),
            }
        }

        /// Delivers every answer to an invoked service's request that
        /// arrived (a response, or a part of a streamed one) to its
        /// invocation, each in a call of its own (cause `http-response`).
        fn http_responses(&mut self) -> Result<Option<End>, Box<dyn std::error::Error>> {
            loop {
                let ended = match self.session.deliver_http_responses() {
                    Ok(None) => return Ok(None),
                    Ok(Some(step)) => self.outcome("http-response", Ok(step), None)?,
                    Err(error) => self.outcome("http-response", Err(error), None)?,
                };
                if ended.is_some() {
                    return Ok(ended);
                }
            }
        }

        /// Delivers every owed `error.communication` (cause
        /// `communication-error`).
        fn communication_errors(&mut self) -> Result<Option<End>, Box<dyn std::error::Error>> {
            match self.session.deliver_communication_errors() {
                Ok(None) => Ok(None),
                Ok(Some(step)) => self.outcome("communication-error", Ok(step), None),
                Err(error) => self.outcome("communication-error", Err(error), None),
            }
        }

        fn timer(&mut self) -> Result<Option<End>, Box<dyn std::error::Error>> {
            match self.session.advance() {
                Ok(step) => {
                    self.call_record("timer", &step)?;
                    Ok(self.ended())
                }
                Err(error) => Ok(self.failure(&error, None)?.then_some(End::Failed)),
            }
        }

        fn ended(&self) -> Option<End> {
            match self.session.status() {
                LiveStatus::Finished => Some(End::Finished),
                LiveStatus::Failed => Some(End::Failed),
                LiveStatus::Running => None,
            }
        }

        /// Milliseconds until the next deadline; `None` when nothing is
        /// scheduled.
        fn wait(&mut self) -> Result<Option<u64>, SessionHostError> {
            // The logical clock never moves past an invoked service's request
            // in flight: its response is the next thing that happens.
            if self.clock == ClockMode::Logical && self.session.awaiting_http_responses() {
                return Ok(None);
            }
            let deadline = self.session.next_deadline()?;
            Ok(deadline.map(|deadline| match self.clock {
                ClockMode::Wall => {
                    deadline.saturating_sub(self.session.clock_ms().unwrap_or(deadline))
                }
                ClockMode::Logical => 0,
            }))
        }

        fn close(mut self, end: End) -> Result<(), Box<dyn std::error::Error>> {
            // No message arrives once the session closes.
            if let Some(listener) = self.listener.take() {
                listener.stop();
            }
            let (reason, status) = match end {
                End::Finished => ("done", EXIT_DONE),
                End::Failed => ("failed", EXIT_FAILED),
                End::Closed(reason, status) => (reason, status),
            };
            self.sink
                .log(&Object::record("close").string("reason", reason).finish())?;
            let report = self.session.close();
            let status = match report {
                Ok(report) => {
                    self.sink.outputs(report.outputs)?;
                    let state = match report.status {
                        LiveStatus::Running => "running",
                        LiveStatus::Finished => "finished",
                        LiveStatus::Failed => "failed",
                    };
                    self.sink.emit(
                        &Object::record("closed")
                            .string("reason", reason)
                            .string("status", state)
                            .number(
                                "cancelled_sends",
                                u64::try_from(report.cancelled_sends).unwrap_or(u64::MAX),
                            )
                            .number(
                                "cancelled_children",
                                u64::try_from(report.cancelled_children).unwrap_or(u64::MAX),
                            )
                            .finish(),
                    )?;
                    status
                }
                Err(error) => {
                    self.sink.error(
                        &failure_code(&error.failure),
                        &error.to_string(),
                        Severity::Fatal,
                        None,
                    )?;
                    self.sink.emit(
                        &Object::record("closed")
                            .string("reason", reason)
                            .string("status", "failed")
                            .finish(),
                    )?;
                    EXIT_FAILED
                }
            };
            if status == EXIT_DONE {
                Ok(())
            } else {
                Err(Box::new(Exited(status)))
            }
        }

        /// Waits for the next input until the timer due in `wait` ms (none:
        /// no timer), in [`LISTENER_CHECK`] slices while a listener serves.
        fn receive(&self, wait: Option<u64>) -> Received {
            let receiver = &self.messages.1;
            let listening = if self.listener.is_some() {
                Listening::Serving
            } else {
                Listening::Absent
            };
            let window = match (wait, listening) {
                (Some(0), _) => return Received::TimerDue,
                (Some(ms), _) => receive_window(Duration::from_millis(ms), listening),
                (None, Listening::Serving) => Window::Slice(LISTENER_CHECK),
                (None, Listening::Absent) => {
                    return Received::Message(receiver.recv().unwrap_or(Message::Eof));
                }
            };
            let (Window::UntilDue(duration) | Window::Slice(duration)) = window;
            match receiver.recv_timeout(duration) {
                Ok(message) => Received::Message(message),
                Err(error) => {
                    if error == mpsc::RecvTimeoutError::Disconnected {
                        std::thread::sleep(duration);
                    }
                    match window {
                        Window::UntilDue(_) => Received::TimerDue,
                        Window::Slice(_) => Received::Recheck,
                    }
                }
            }
        }

        /// Fails the run when a `BasicHTTP` listener thread panicked.
        fn listener_failed(&mut self) -> Result<Option<End>, Box<dyn std::error::Error>> {
            if !self.listener.as_ref().is_some_and(ListenerHandle::panicked) {
                return Ok(None);
            }
            self.sink.error(
                "http-listener-failed",
                "a BasicHTTP listener thread panicked",
                Severity::Fatal,
                None,
            )?;
            Ok(Some(End::Failed))
        }

        /// The standard-input loop of a live run.
        fn serve(&mut self, on_eof: OnEof) -> Result<End, Box<dyn std::error::Error>> {
            if let Some(end) = self.ended() {
                return Ok(end);
            }
            let sender = self.messages.0.clone();
            watch_signals(sender.clone())
                .map_err(|error| format!("cannot watch termination signals: {error}"))?;
            std::thread::Builder::new()
                .name("scxml-run-stdin".into())
                .spawn(move || read_stdin(&sender))?;
            let mut eof = false;
            loop {
                let wait = match self.wait() {
                    Ok(wait) => wait,
                    Err(error) => {
                        self.failure(&error, None)?;
                        return Ok(End::Failed);
                    }
                };
                // With nothing scheduled, the end of input closes the session
                // unless a BasicHTTP message may still arrive: the listener
                // serves, or an accepted request may still fail.
                if eof
                    && wait.is_none()
                    && self.listener.is_none()
                    && !self.session.communication_pending()
                    && !self.session.awaiting_http_responses()
                {
                    return Ok(End::Closed("eof", EXIT_CLOSED));
                }
                if let Some(end) = self.listener_failed()? {
                    return Ok(end);
                }
                let message = match self.receive(wait) {
                    Received::Message(message) => Some(message),
                    Received::TimerDue => None,
                    Received::Recheck => continue,
                };
                let ended = match message {
                    None => self.timer()?,
                    Some(Message::Line(number, Err(message))) => {
                        self.sink.error(
                            "input-invalid",
                            &message,
                            Severity::Recoverable,
                            Some(number),
                        )?;
                        None
                    }
                    Some(Message::Line(number, Ok(text))) => match parse_input(&text) {
                        Err(message) => {
                            self.sink.error(
                                "input-invalid",
                                &message,
                                Severity::Recoverable,
                                Some(number),
                            )?;
                            None
                        }
                        Ok(Input::Event(event)) => self.event(&event, Some(number))?,
                        Ok(Input::Snapshot) => self.snapshot(Some(number))?.then_some(End::Failed),
                        Ok(Input::Close) => Some(End::Closed("close", EXIT_CLOSED)),
                    },
                    Some(Message::Eof) => {
                        eof = true;
                        (on_eof == OnEof::Close).then_some(End::Closed("eof", EXIT_CLOSED))
                    }
                    Some(Message::ReadFailed(message)) => {
                        self.sink.error(
                            "input-unreadable",
                            &message,
                            Severity::Recoverable,
                            None,
                        )?;
                        eof = true;
                        (on_eof == OnEof::Close).then_some(End::Closed("eof", EXIT_CLOSED))
                    }
                    Some(Message::Signal(signal)) => {
                        let status = u8::try_from(128 + signal).unwrap_or(u8::MAX);
                        Some(End::Closed("signal", status))
                    }
                    Some(Message::Http(inbound)) => self.http(inbound)?,
                    Some(Message::CommunicationSettled) => self.communication_errors()?,
                    Some(Message::HttpResponse) => self.http_responses()?,
                };
                if let Some(end) = ended {
                    return Ok(end);
                }
            }
        }

        /// Replays `records` through the logical clock, writing the records
        /// the live run wrote: an `idle` the live run made right after
        /// `start`, an event, an inbound message, or a late
        /// `error.communication` delivery (the `--clock logical` drain)
        /// folds into that call's record, as it did live.
        fn replay(
            &mut self,
            start: LiveCallResult,
            records: &[InputRecord],
        ) -> Result<End, Box<dyn std::error::Error>> {
            // Outbound decisions and the clock's start were consumed when the
            // session opened; they sit between the records of the calls.
            let mut records = records
                .iter()
                .filter(|record| {
                    !matches!(
                        record,
                        InputRecord::OutboundHttp { .. } | InputRecord::Start { .. }
                    )
                })
                .peekable();
            let mut start = Some(start);
            if records.peek() == Some(&&InputRecord::Idle) {
                records.next();
                match self.session.apply(&InputRecord::Idle) {
                    Ok(Applied::Call(step)) => start = Some(step),
                    Ok(
                        Applied::Snapshot(_)
                        | Applied::Received(_)
                        | Applied::OutboundDecision
                        | Applied::Started,
                    ) => {}
                    Err(error) => {
                        self.failure(&error, None)?;
                        return Ok(End::Failed);
                    }
                }
            }
            if let Some(start) = start {
                self.call_record("start", &start)?;
            }
            // Every record applies, even after the root finished: the live run
            // recorded exactly what it did (a snapshot of the finished
            // session, a late event it rejected).
            while let Some(record) = records.next() {
                let cause = match record {
                    InputRecord::Event { .. } => "event",
                    InputRecord::Advance { .. } => "timer",
                    InputRecord::Idle => "idle",
                    InputRecord::Snapshot { .. } => "snapshot",
                    InputRecord::Http { .. } => "http",
                    InputRecord::CommunicationErrors { .. } => "communication-error",
                    InputRecord::HttpResponse { .. } | InputRecord::HttpResponsePart { .. } => {
                        "http-response"
                    }
                    InputRecord::OutboundHttp { .. } => "outbound",
                    InputRecord::Start { .. } => "start",
                };
                let settles = matches!(
                    record,
                    InputRecord::Event { .. }
                        | InputRecord::Http { .. }
                        | InputRecord::CommunicationErrors { .. }
                        | InputRecord::HttpResponse { .. }
                        | InputRecord::HttpResponsePart { .. }
                );
                let mut applied = self.session.apply(record);
                if settles && records.peek() == Some(&&InputRecord::Idle) {
                    let stepped = match &applied {
                        Ok(Applied::Call(step) | Applied::Received(Some(step))) => Some(*step),
                        _ => None,
                    };
                    if let Some(step) = stepped {
                        records.next();
                        applied = self
                            .session
                            .apply(&InputRecord::Idle)
                            .map(|idle| match idle {
                                Applied::Call(idle) => Applied::Call(LiveCallResult {
                                    at_ms: step.at_ms,
                                    commands: step.commands.saturating_add(idle.commands),
                                    ..idle
                                }),
                                other => other,
                            });
                    }
                }
                match applied {
                    Ok(Applied::Call(step) | Applied::Received(Some(step))) => {
                        self.call_record(cause, &step)?;
                    }
                    Ok(Applied::Received(None)) => self.flush()?,
                    Ok(Applied::OutboundDecision | Applied::Started) => {}
                    Ok(Applied::Snapshot(bytes)) => {
                        self.flush()?;
                        self.sink.snapshot(&bytes)?;
                    }
                    Err(error) => {
                        if self.failure(&error, None)? {
                            return Ok(End::Failed);
                        }
                    }
                }
            }
            Ok(self.ended().unwrap_or(End::Closed("replayed", EXIT_CLOSED)))
        }
    }

    /// The session's `BasicHTTP` network: the allowlisted client (none
    /// without `--http-allow`) and the bound, not yet serving listener.
    fn network(
        args: &HttpArgs,
        messages: &mpsc::Sender<Message>,
    ) -> Result<(LiveHttp, Option<BasicHttpListener>), Box<dyn std::error::Error>> {
        let mut http = LiveHttp::default();
        if !args.allow.is_empty() {
            let allow = HttpAllowlist::new(&args.allow).map_err(|error| {
                let entry = args.allow.get(error.index).map_or("", String::as_str);
                format!("[http-allow-invalid] --http-allow `{entry}`: {error}")
            })?;
            let client = NetworkClient::new(HttpClientConfig::new(allow))
                .map_err(|error| format!("[http-allow-invalid] {error}"))?;
            http.outbound = Some(Arc::new(client));
            let sender = messages.clone();
            http.on_communication_error = Some(Arc::new(move || {
                let _ = sender.send(Message::CommunicationSettled);
            }));
        }
        let Some(addr) = args.listen else {
            return Ok((http, None));
        };
        let listener = BasicHttpListener::bind(addr, ListenerConfig::default())
            .map_err(|error| format!("[http-listen-failed] cannot listen on {addr}: {error}"))?;
        let public = args
            .public_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", listener.local_addr()));
        http.public_url = Some(PublicUrl::new(&public).map_err(|error| {
            format!("[http-public-url-invalid] --http-public-url `{public}`: {error}")
        })?);
        Ok((http, Some(listener)))
    }

    /// Runs `run` (see the module documentation).
    ///
    /// # Errors
    ///
    /// [`Exited`] with the exit status once the session ran, or a failure
    /// before it.
    #[allow(
        clippy::too_many_lines,
        reason = "one sequence: identity, replay log, host, record log, open, then the loop"
    )]
    pub fn execute(args: &RunArgs) -> Result<(), Box<dyn std::error::Error>> {
        let source = crate::read_source(&args.document)?;
        // Every literal URI of the document tree loads once, before the
        // first macrostep; the `<invoke src>` documents' packages bind with
        // the root's.
        let mut loaded = crate::documents::Documents::prefetch(
            args.namespaces.repository.open()?,
            &args.document,
            &source,
        )?;
        let packages = crate::extensions::Packages::resolve(
            &source,
            loaded.children(),
            &args.namespaces.packages,
            &[],
            &args.namespaces.repository,
        )?;
        args.namespaces.check(&packages)?;
        let services = crate::services::resolve(
            &source,
            loaded.children(),
            &args.services.packages,
            &args.namespaces.repository,
        )?;
        args.services.check(&services.packages)?;
        let data_models = crate::datamodels::resolve(
            &source,
            loaded.children(),
            &args.data_models.packages,
            &args.namespaces.repository,
        )?;
        args.data_models.check(&data_models)?;
        let values = args.namespaces.values(&packages.slot_uses)?;
        crate::mask::install(
            &crate::extensions::HeldValue::pairs(&values),
            &crate::extensions::declared_origins(&services.packages),
        );
        crate::extensions::write_notes(&packages.summary());
        crate::extensions::write_notes(&crate::extensions::env_summary(
            &packages.slot_uses,
            &values,
        ));
        crate::extensions::write_notes(&data_models.summary());
        crate::extensions::write_notes(&loaded.summary());
        let (ir, data_model) = crate::snapshot::host::compile(
            &args.document,
            &packages.all(),
            &services.packages,
            data_models.bound(),
            loaded.script_packages(&source)?,
            loaded.invoked_documents(),
        )?;
        let mut namespaces = Vec::new();
        for package in &packages.explicit {
            let sha256 = args
                .namespaces
                .extension_component_sha256(package)
                .ok_or("every extension package needs --extension-component-sha256")?;
            namespaces.push((package.manifest.namespace_uri.clone(), sha256.to_owned()));
        }
        // A repository namespace's identity is the component digest its
        // release pinned (read from the cache, or from the release's
        // SHA256SUMS on first use); the component itself loads when the
        // session starts.
        if let Some(store) = &packages.store {
            for package in packages
                .repository
                .iter()
                .chain(packages.documents.iter().flat_map(|(_, packages)| packages))
            {
                if let Some(uri) = package.resolved_uri() {
                    namespaces.push((uri.to_owned(), store.component_sha256(uri)?));
                }
            }
        }
        namespaces.sort();
        let mut identity = Identity {
            ir_sha256: format!("{:x}", Sha256::digest(&ir)),
            components_manifest_sha256: args.components.manifest_sha256().to_owned(),
            session_id: args.session_id.clone(),
            max_commands: args.max_commands,
            fuel: args.fuel,
            clock: args.clock.as_str(),
            namespaces,
            services: args.services.identity(&services)?,
            http_public_url: None,
        };
        let replay = args
            .replay
            .as_deref()
            .map(|path| read_log(path, &identity))
            .transpose()?;
        if let Some(log) = &replay {
            identity.session_id.clone_from(&log.session_id);
            identity.max_commands = log.max_commands;
            identity.fuel = log.fuel;
            identity.http_public_url.clone_from(&log.http_public_url);
        }
        // The service components load once the replay log matched the run's
        // identity. An invoked service's HTTP responses are recorded inputs:
        // a run that records performs its requests, and a replay answers
        // them from its log without the network.
        let (service_components, service_imports) =
            args.services
                .components(&services, &values, args.replay.is_none())?;
        crate::extensions::write_notes(&crate::services::summary(
            &services.packages,
            Some(&args.services),
            Some(&service_imports),
        ));
        crate::snapshot::host::bind_documents(&mut loaded, &packages, &args.services, &services)?;
        let host = args.components.session_host(identity.fuel)?;
        let extensions = crate::snapshot::host::extensions(&args.namespaces, &packages)?;
        let host = host
            .with_extension_components(extensions)?
            .with_service_components(service_components)?
            .with_data_model_components(args.data_models.components(&data_models)?)?;
        let messages = mpsc::channel();
        // A replay reproduces the recorded network without touching it: the
        // recorded public URL, no transport, and no listener.
        let (mut http, listener) = match &replay {
            Some(_) => (
                LiveHttp {
                    public_url: identity
                        .http_public_url
                        .as_deref()
                        .map(PublicUrl::new)
                        .transpose()
                        .map_err(|error| format!("[replay-invalid] `http_public_url`: {error}"))?,
                    ..LiveHttp::default()
                },
                None,
            ),
            None => network(&args.http, &messages.0)?,
        };
        if replay.is_none() {
            identity.http_public_url = http.public_url.as_ref().map(|url| url.as_str().to_owned());
            // An invoked service's HTTP response wakes the loop, which
            // delivers it (`deliver_http_responses`).
            let sender = messages.0.clone();
            http.on_http_response = Some(Arc::new(move || {
                let _ = sender.send(Message::HttpResponse);
            }));
        }
        let log = match &args.record {
            None => None,
            Some(path) => {
                let mut file = io::BufWriter::new(fs::File::create(path).map_err(|error| {
                    format!(
                        "[record-unwritable] cannot create `{}`: {error}",
                        path.display()
                    )
                })?);
                file.write_all(identity.header().as_bytes())?;
                file.write_all(b"\n")?;
                file.flush()?;
                Some(file)
            }
        };
        let clock = match (&replay, args.clock) {
            (None, ClockMode::Wall) => LiveClock::Timed(Box::new(WallClock::start())),
            _ => LiveClock::Logical,
        };
        let mut sink = Sink {
            stdout: io::stdout(),
            log,
        };
        let opened = host.open(SessionOpen {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: &identity.session_id,
            embedder: Rc::new(loaded),
            max_commands: identity.max_commands,
            clock,
            record: args.record.is_some(),
            replay: replay.as_ref().map(|log| log.records.as_slice()),
            http,
        });
        let mut session = match opened {
            Ok(session) => session,
            Err(error) => {
                let message = error.to_string();
                sink.error(
                    &failure_code(&error.failure),
                    &message,
                    Severity::Fatal,
                    None,
                )?;
                sink.emit(
                    &Object::record("closed")
                        .string("reason", "failed")
                        .string("status", "failed")
                        .finish(),
                )?;
                return Err(Box::new(Exited(EXIT_FAILED)));
            }
        };
        let io_processors = session.io_processors()?;
        let mut entries = Object::new();
        for (key, location) in &io_processors {
            entries = entries.raw(key, &Object::new().string("location", location).finish());
        }
        sink.emit(
            &Object::record("opened")
                .string("session", &identity.session_id)
                .string(
                    "clock",
                    if replay.is_some() {
                        "logical"
                    } else {
                        args.clock.as_str()
                    },
                )
                .raw("ioprocessors", &entries.finish())
                .finish(),
        )?;
        let start = LiveCallResult {
            at_ms: 0,
            status: session.status(),
            next_deadline: session.next_deadline()?,
            commands: 0,
        };
        let mut driver = Driver {
            session,
            sink,
            clock: if replay.is_some() {
                ClockMode::Logical
            } else {
                args.clock
            },
            snapshot_on: args.snapshot_on,
            options: args.options.clone(),
            snapshot_done: false,
            messages,
            listener: None,
        };
        // Inbound messages flow only once the session is open.
        if let Some(listener) = listener {
            let sender = driver.messages.0.clone();
            let serving = listener
                .serve(move |inbound| {
                    let _ = sender.send(Message::Http(inbound));
                })
                .map_err(|error| format!("[http-listen-failed] cannot serve: {error}"))?;
            driver.listener = Some(serving);
        }
        let end = if let Some(log) = replay {
            driver.replay(start, &log.records)?
        } else {
            let start = if args.clock == ClockMode::Logical && start.status == LiveStatus::Running {
                let idle = driver.session.run_until_idle();
                match idle {
                    Ok(step) => step,
                    Err(error) => {
                        driver.failure(&error, None)?;
                        return driver.close(End::Failed);
                    }
                }
            } else {
                start
            };
            driver.call_record("start", &start)?;
            driver.serve(args.on_eof)?
        };
        driver.close(end)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn record(text: &str) -> Result<Option<InputRecord>, String> {
            let serde_json::Value::Object(fields) =
                serde_json::from_str(text).expect("test records are JSON objects")
            else {
                panic!("test records are JSON objects");
            };
            log_record(&fields)
        }

        /// While a listener serves, a wait never outlasts one
        /// `LISTENER_CHECK` slice, so a panicked listener is reported within
        /// it even with a timer minutes away; without one, the wait lasts
        /// until the timer is due.
        #[test]
        fn a_serving_listener_is_checked_while_a_timer_is_pending() {
            let minute = Duration::from_secs(60);
            assert_eq!(
                receive_window(minute, Listening::Serving),
                Window::Slice(LISTENER_CHECK)
            );
            assert_eq!(
                receive_window(minute, Listening::Absent),
                Window::UntilDue(minute)
            );
            let soon = Duration::from_millis(5);
            assert_eq!(
                receive_window(soon, Listening::Serving),
                Window::UntilDue(soon)
            );
        }

        #[test]
        fn communication_error_records_carry_each_failed_send_id() {
            let parsed = record(
                r#"{"type":"communication-error","at_ms":4,"senders":[{"session":"root","path":["kid"],"sendid":"note"}]}"#,
            )
            .expect("the record parses");
            assert_eq!(
                parsed,
                Some(InputRecord::CommunicationErrors {
                    at_ms: 4,
                    senders: vec![CommunicationSender {
                        session_id: "root".into(),
                        path: vec!["kid".into()],
                        send_id: "note".into(),
                    }],
                })
            );
            for senders in [
                r#"[{"session":"root","path":[]}]"#,
                r#"[{"session":"root","path":[],"sendid":7}]"#,
                r#"[{"session":"root","path":[],"sendid":"not a name"}]"#,
                r#"[{"session":"root","path":[7],"sendid":"note"}]"#,
                r#"[{"session":"root","path":[""],"sendid":"note"}]"#,
                r#"[{"session":"root","path":["not an id"],"sendid":"note"}]"#,
            ] {
                assert!(
                    record(&format!(
                        r#"{{"type":"communication-error","at_ms":4,"senders":{senders}}}"#
                    ))
                    .is_err(),
                    "{senders}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn args(extra: &[&str]) -> Result<RunArgs, String> {
        let mut all = vec![
            "doc.scxml".to_owned(),
            "--components".to_owned(),
            "dir".to_owned(),
            "--components-manifest-sha256".to_owned(),
            DIGEST.to_owned(),
        ];
        all.extend(extra.iter().map(|value| (*value).to_owned()));
        parse(&all)
    }

    #[test]
    fn defaults_are_wall_clock_wait_and_no_snapshots() {
        let parsed = args(&[]).expect("minimal run parses");
        assert_eq!(parsed.clock, ClockMode::Wall);
        assert_eq!(parsed.on_eof, OnEof::Wait);
        assert_eq!(parsed.snapshot_on, SnapshotOn::Never);
        assert_eq!(parsed.session_id, "session");
        assert_eq!(parsed.max_commands, u64::MAX, "no default command budget");
        assert!(parsed.record.is_none() && parsed.replay.is_none());
    }

    #[test]
    fn every_option_parses() {
        let parsed = args(&[
            "--clock",
            "logical",
            "--snapshot-on",
            "call",
            "--on-eof",
            "close",
            "--exclude",
            "queues",
            "--trace-context",
            "preserve",
            "--fuel",
            "5000",
            "--max-commands",
            "9",
            "--session-id",
            "agent",
            "--record",
            "log.ndjson",
        ])
        .expect("every option parses");
        assert_eq!(parsed.clock, ClockMode::Logical);
        assert_eq!(parsed.snapshot_on, SnapshotOn::Call);
        assert_eq!(parsed.on_eof, OnEof::Close);
        assert_eq!(parsed.fuel, 5000);
        assert_eq!(parsed.max_commands, 9);
        assert_eq!(parsed.session_id, "agent");
        assert_eq!(parsed.options.trace_context, TraceContextPolicy::Preserve);
        assert_eq!(parsed.record, Some(PathBuf::from("log.ndjson")));
    }

    #[test]
    fn http_options_parse_and_default_to_loopback() {
        let parsed = args(&[
            "--http-allow",
            "https://api.example",
            "--http-allow",
            "http://127.0.0.1:9000/hooks",
            "--http-listen",
            "8080",
        ])
        .expect("http options parse");
        assert_eq!(
            parsed.http.allow,
            ["https://api.example", "http://127.0.0.1:9000/hooks"]
        );
        assert_eq!(
            parsed.http.listen,
            Some("127.0.0.1:8080".parse().expect("address"))
        );
        assert_eq!(parsed.http.public_url, None);
        let parsed = args(&[
            "--http-listen",
            "0.0.0.0:8080",
            "--http-public-url",
            "https://agent.example/scxml",
        ])
        .expect("an unspecified address with a public URL parses");
        assert!(
            parsed
                .http
                .listen
                .is_some_and(|addr| addr.ip().is_unspecified())
        );
        assert!(!args(&[]).expect("no http").http.any());
        for (extra, expected) in [
            (&["--http-allow", ""][..], "--http-allow requires"),
            (
                &["--http-listen", "nope"][..],
                "--http-listen requires PORT",
            ),
            (&["--http-listen", "[::]:1"][..], "needs --http-public-url"),
            (
                &["--http-public-url", "https://a.example"][..],
                "give --http-listen too",
            ),
            (
                &["--http-listen", "1", "--replay", "log"][..],
                "--http-allow, --http-listen, and --http-public-url cannot be given with it",
            ),
        ] {
            let error = args(extra).expect_err("rejected");
            assert!(error.contains(expected), "{extra:?}: {error}");
        }
    }

    #[test]
    fn invalid_or_conflicting_options_are_rejected() {
        for (extra, expected) in [
            (
                &["--clock", "sundial"][..],
                "--clock requires wall or logical",
            ),
            (&["--snapshot-on", "always"][..], "--snapshot-on requires"),
            (&["--on-eof", "exit"][..], "--on-eof requires wait or close"),
            (&["--fuel", "0"][..], "--fuel requires a positive integer"),
            (&["--max-commands", "x"][..], "--max-commands requires"),
            (&["--session-id", "not valid"][..], "--session-id requires"),
            (
                &["--replay", "a", "--record", "b"][..],
                "--record comes from the log",
            ),
            (
                &["--replay", "a", "--clock", "wall"][..],
                "--clock comes from the log",
            ),
            (&["--evnt", "go"][..], "unknown option"),
        ] {
            let error = args(extra).expect_err("rejected");
            assert!(error.contains(expected), "{extra:?}: {error}");
        }
        let parsed =
            parse(&["doc.scx".to_owned()]).expect("the embedded components are the default");
        assert_eq!(parsed.components, ComponentArgs::Embedded);
        let error = parse(&[
            "doc.scx".to_owned(),
            "--components".to_owned(),
            "dir".to_owned(),
        ])
        .expect_err("one override flag alone");
        assert!(error.contains("together"), "{error}");
    }
}
