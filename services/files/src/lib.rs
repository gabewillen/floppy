//! The `files` invoked service: reads and writes the files of one directory
//! the run grants, as a `stateforward:scxml/invoked-service` Wasm component.
//! The builder bot (`bots/builder/builder.scx`) writes each draft through it
//! as three documents beside each other, `<name>.scx`, `<name>.json` and
//! `<name>.js` (decision D121), and reflection reads a stored behavior's
//! back through it to revise it. It is modeled on scxml.rs's files example
//! service (`scxml.rs/components/scxml-service-files`), with its parameter
//! names and its result.
//!
//! Files are I/O, so this is a service, never executable content. Documents
//! name [`SERVICE_URI`]:
//!
//! ```xml
//! <invoke type="https://github.com/stateforward/floppy/services/files">
//!   <param name="name" expr="'GreetOnKnock.scx'"/>
//!   <param name="text" expr="scxml_text"/>
//! </invoke>
//! ```
//!
//! # Parameters (`<param>`)
//!
//! Each optional, as the example's:
//!
//! * `seed`: the name of a file to read.
//! * `name` and `text`, both or neither: the name of a file to write and its
//!   text (a string), written whole (created, or truncated first).
//!
//! A name is one plain path component ([`is_file_name`]): non-empty, at most
//! [`MAX_FILE_NAME_BYTES`] bytes, not `.` or `..`, and free of `/`, `\` and
//! control characters, so it names a file directly in [`DIRECTORY`] and
//! never one outside it. Any other parameter is ignored, as the example
//! ignores it; the last of a parameter given twice wins.
//!
//! The directory is the run's grant:
//! `--dir https://github.com/stateforward/floppy/services/files=HOST::/behaviors`
//! (`scx run` refuses a run that binds the service without it). The service
//! reads and writes only there.
//!
//! # Result
//!
//! As the example: `invoke` reads the seed, then writes, then resolves its
//! future at once with the done data `{"seed": TEXT-OR-NULL, "wrote":
//! BYTES-OR-NULL}` (the invoking session receives it as `done.invoke.<id>`),
//! and returns no event. An invalid parameter, or a failed filesystem call
//! (a missing seed, a seed that is not UTF-8),
//! rejects the start: the `<invoke>` raises `error.execution`.
//!
//! # Bounds
//!
//! The name's bytes ([`MAX_FILE_NAME_BYTES`], the example's bound) are
//! bounded on wasm builds only ([`scxml_types::wasm_bound`]); the seed has
//! no byte bound (the done data that carries it has none).
//!
//! # Telemetry
//!
//! `invoke` records one observation through scxml.rs's `telemetry` WIT
//! facade (operation `start`: its closed outcome, `ok`, `rejected` or
//! `failed`, and one closed signal event). No attribute carries a name, a
//! text or a path, so the cardinality is the product of the closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::fmt::Write as _;

use scxml_types::{exceeds_wasm_bound, wasm_bound};

/// The service's name: the last path segment of its repository service URI
/// before the tag.
pub const SERVICE_NAME: &str = "files";

/// The exact service type URI documents name: tagless, so it names the
/// repository's latest release (decision D19); the manifest names it too.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/files";

/// Where the granted directory appears inside the component.
pub const DIRECTORY: &str = "/behaviors";

/// Upper bound on a file name's bytes (wasm builds only).
pub const MAX_FILE_NAME_BYTES: usize = wasm_bound(255);

/// Why the service rejected its start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesError {
    /// A file name parameter is not text naming one plain path component.
    InvalidName,
    /// `text` is not text, or `name` and `text` are not both given.
    InvalidText,
    /// A filesystem call failed, or the seed is not UTF-8.
    Filesystem,
}

impl FilesError {
    /// The rejection's static detail (never a name or a text).
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::InvalidName => "a file name is invalid",
            Self::InvalidText => "`name` and `text` go together",
            Self::Filesystem => "a filesystem call failed",
        }
    }
}

/// One invocation parameter's value as the service reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Parameter<'a> {
    /// A text value.
    Text(&'a str),
    /// Any other value.
    Other,
}

/// What one start does.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilesRequest {
    /// The file to read, in [`DIRECTORY`].
    pub seed: Option<String>,
    /// The file to write, in [`DIRECTORY`], and its text.
    pub write: Option<(String, String)>,
}

/// Whether `name` is one plain path component: non-empty, at most
/// [`MAX_FILE_NAME_BYTES`] bytes, not `.` or `..`, and free of `/`, `\` and
/// control characters, so it names a file directly in [`DIRECTORY`].
#[must_use]
pub fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && !exceeds_wasm_bound(name.len(), MAX_FILE_NAME_BYTES)
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|character| matches!(character, '/' | '\\') || character.is_control())
}

impl FilesRequest {
    /// The request the invocation's parameters (in document order; the last
    /// of each name wins) describe.
    ///
    /// # Errors
    ///
    /// [`FilesError::InvalidName`] for a file name that is not text or not
    /// [`is_file_name`], and [`FilesError::InvalidText`] for a `text` that is
    /// not text or a `name` without a `text` (or the reverse).
    pub fn from_parameters<'a>(
        parameters: impl IntoIterator<Item = (&'a str, Parameter<'a>)>,
    ) -> Result<Self, FilesError> {
        let mut seed = None;
        let mut name = None;
        let mut text = None;
        for (parameter, value) in parameters {
            match (parameter, value) {
                ("seed", value) => seed = Some(file_name(value)?),
                ("name", value) => name = Some(file_name(value)?),
                ("text", Parameter::Text(value)) => text = Some(value.to_owned()),
                ("text", Parameter::Other) => return Err(FilesError::InvalidText),
                _ => {}
            }
        }
        let write = match (name, text) {
            (Some(name), Some(text)) => Some((name, text)),
            (None, None) => None,
            _ => return Err(FilesError::InvalidText),
        };
        Ok(Self { seed, write })
    }
}

fn file_name(value: Parameter<'_>) -> Result<String, FilesError> {
    match value {
        Parameter::Text(name) if is_file_name(name) => Ok(name.to_owned()),
        _ => Err(FilesError::InvalidName),
    }
}

/// The path of the file `name` (an [`is_file_name`]) names.
#[must_use]
pub fn path(name: &str) -> String {
    format!("{DIRECTORY}/{name}")
}

/// `text` as a JSON string literal (quotes, backslashes and control
/// characters escaped).
#[must_use]
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The done data as JSON object text, the example's:
/// `{"seed":TEXT-OR-NULL,"wrote":BYTES-OR-NULL}`.
#[must_use]
pub fn completion(seed: Option<&str>, wrote: Option<usize>) -> String {
    let seed = seed.map_or_else(|| "null".to_owned(), json_string);
    let wrote = wrote.map_or_else(|| "null".to_owned(), |bytes| bytes.to_string());
    format!("{{\"seed\":{seed},\"wrote\":{wrote}}}")
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

    use std::io::{Read as _, Write as _};

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, InvokeRequest,
    };

    use super::{FilesError, FilesRequest, Parameter};

    /// The invocation's result: `ok` with its done data, or its error.
    type Done = Result<Option<DataValue>, ComponentError>;

    struct Component;

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its task is spawned"
    )]
    impl Guest for Component {
        async fn invoke(
            request: InvokeRequest,
            events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            // The service reads no event.
            drop(events);
            let result = run(&request);
            let outcome = match &result {
                Ok(_) => Outcome::Ok,
                Err(failure) if failure.code == ErrorCode::InvalidInput => Outcome::Rejected,
                Err(_) => Outcome::Failed,
            };
            telemetry::observe(Operation::Start, outcome, None, None);
            telemetry::event(
                Operation::Start,
                if result.is_ok() {
                    SignalEvent::TransformationCompleted
                } else {
                    SignalEvent::InputRejected
                },
                None,
            );
            let data = result?;
            // No event; the future resolves at once with the done data.
            let (returned, returned_reader) = wit_stream::new::<Event>();
            drop(returned);
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            wit_bindgen::spawn(async move {
                let _ = done.write(Ok(Some(DataValue::JsonObject(data)))).await;
            });
            Ok((returned_reader, done_reader))
        }
    }

    /// Reads and writes as the request asks; the done data.
    fn run(request: &InvokeRequest) -> Result<String, ComponentError> {
        if request.type_uri != super::SERVICE_URI {
            return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
        }
        let files = FilesRequest::from_parameters(request.parameters.iter().map(|entry| {
            let value = match &entry.value {
                DataValue::Text(text) => Parameter::Text(text),
                _ => Parameter::Other,
            };
            (entry.name.as_str(), value)
        }))
        .map_err(files_error)?;
        let seed = files.seed.as_deref().map(read).transpose()?;
        let wrote = match &files.write {
            Some((name, text)) => Some(write(name, text)?),
            None => None,
        };
        Ok(super::completion(seed.as_deref(), wrote))
    }

    fn read(name: &str) -> Result<String, ComponentError> {
        let mut file = std::fs::File::open(super::path(name)).map_err(|_| filesystem())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|_| filesystem())?;
        String::from_utf8(bytes).map_err(|_| filesystem())
    }

    fn write(name: &str, text: &str) -> Result<usize, ComponentError> {
        let mut file = std::fs::File::create(super::path(name)).map_err(|_| filesystem())?;
        file.write_all(text.as_bytes()).map_err(|_| filesystem())?;
        Ok(text.len())
    }

    fn filesystem() -> ComponentError {
        files_error(FilesError::Filesystem)
    }

    fn files_error(failure: FilesError) -> ComponentError {
        let code = match failure {
            FilesError::InvalidName | FilesError::InvalidText => ErrorCode::InvalidInput,
            FilesError::Filesystem => ErrorCode::Unavailable,
        };
        error(code, failure.detail())
    }

    fn error(code: ErrorCode, detail: &str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: Some(detail.to_owned()),
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests;
