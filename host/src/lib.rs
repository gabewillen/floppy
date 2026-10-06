//! nemobot's embedder of the scxml.rs Wasmtime session host.
//!
//! A bot is one SCXML document run as a live root session of
//! `scxml_host_wasmtime::SessionHost`. This crate is the composition root
//! around it, and stays a thin host (scxml.rs `AGENTS.md`): it never selects
//! transitions, runs macrosteps, or interprets events; the engine does, inside
//! the `scxml-machine` component.
//!
//! * [`config`]: the nemobot config file (`stateforward.nemobot.config/v1`):
//!   the release components, the env file and the variables the run grants,
//!   the extension and service packages to bind, and their WASI grants.
//! * [`env_file`]: Docker `--env-file` values. Values reach components only as
//!   opaque `env`-slot references the scxml.rs host substitutes into admitted
//!   requests; this crate never logs, prints, or formats a value
//!   ([`env_file::Values`] masks them in `Debug`).
//! * [`packages`]: reads and verifies every package manifest (scxml.rs's
//!   `scxml-validator-core` parsers), authenticates each component by its
//!   configured SHA-256, and lists what binding consents to
//!   ([`packages::Packages::consent`]) so the embedding CLI can show it before
//!   anything runs.
//! * [`compile`]: validates and compiles one document to Machine IR with the
//!   bound packages, exactly as `scxml-cli run` does.
//! * [`runtime`]: one live session on its own thread. `LiveSession` is
//!   `!Send`, so the thread builds the host and the session and owns them;
//!   callers talk to it through an ingress channel ([`runtime::Ingress`]) and
//!   read its outputs from an egress channel ([`runtime::Egress`]).
//!
//! Telemetry: OpenTelemetry API spans at every boundary above
//! (`telemetry` module docs carry the cardinality analysis).

#![forbid(unsafe_code)]

pub mod compile;
pub mod config;
pub mod env_file;
pub mod packages;
pub mod runtime;
mod telemetry;

/// The closed category of a [`HostFailure`] (also its telemetry label).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    /// The config file is unreadable or invalid.
    Config,
    /// The env file is unreadable or invalid, or lacks a granted variable.
    EnvFile,
    /// A package manifest, schema, or component is unreadable, invalid, or
    /// does not authenticate, or the host refuses its binding.
    Package,
    /// A WASI grant names nothing a package declares, or a declared
    /// permission is not granted.
    Grant,
    /// The document does not validate or compile.
    Compile,
    /// The session host could not link, open, run, or close the session.
    Session,
    /// The session thread is gone or could not start.
    Channel,
}

impl FailureKind {
    /// The stable label.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::EnvFile => "env-file",
            Self::Package => "package",
            Self::Grant => "grant",
            Self::Compile => "compile",
            Self::Session => "session",
            Self::Channel => "channel",
        }
    }
}

/// A failure of the embedder: its kind and a message that names the
/// offending file, field, package, or variable, never a value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostFailure {
    kind: FailureKind,
    message: String,
}

impl HostFailure {
    /// A failure of `kind`.
    #[must_use]
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Its category.
    #[must_use]
    pub const fn kind(&self) -> FailureKind {
        self.kind
    }

    /// Its message (never a value).
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for HostFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "[{}] {}", self.kind.name(), self.message)
    }
}

impl std::error::Error for HostFailure {}

/// Reads a regular file of at most `limit` bytes.
///
/// # Errors
///
/// A message naming `path` for a missing, non-regular, unreadable, or
/// oversized file.
pub(crate) fn read_bounded(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;

    let shown = path.display();
    // Opening a FIFO or a device can block, so only a regular file is opened;
    // the handle is checked again in case the path changed between.
    let metadata = std::fs::metadata(path).map_err(|error| format!("`{shown}`: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("`{shown}` is not a regular file"));
    }
    let file = std::fs::File::open(path).map_err(|error| format!("`{shown}`: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("`{shown}`: {error}"))?;
    if !opened.is_file() {
        return Err(format!("`{shown}` is not a regular file"));
    }
    if opened.len() > limit {
        return Err(format!("`{shown}` is larger than {limit} bytes"));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("`{shown}`: {error}"))?;
    if u64::try_from(bytes.len()).map_or(true, |read| read > limit) {
        return Err(format!("`{shown}` is larger than {limit} bytes"));
    }
    Ok(bytes)
}
