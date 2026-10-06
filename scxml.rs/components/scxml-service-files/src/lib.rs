//! Example invoked-service component (SCXML 6.4) that imports
//! `wasi:filesystem`.
//!
//! It proves the WASI resource path end to end: its component imports
//! `wasi:filesystem`, so the session host links it only when the run grants
//! a preopened directory (`--dir URI=HOST::/workspace`), which the component
//! sees at `/workspace`.
//!
//! * `invoke` reads the text parameters `seed` (a file name to read) and
//!   `name` and `text` (a file name to write and its text), each optional; it
//!   reads `/workspace/<seed>`, writes `text` to `/workspace/<name>`, and
//!   resolves its future at once with the JSON object
//!   `{"seed":TEXT-OR-NULL,"wrote":BYTES-OR-NULL}`, returning no event. An
//!   invalid parameter or a failed filesystem call rejects the start (the
//!   `<invoke>` fails with `error.execution`).
//!
//! It uses `std::fs`. The standard library also imports `wasi:cli` and
//! `wasi:clocks/monotonic-clock@0.2`, which `scripts/verify-wasm-capabilities.sh`
//! satisfies with a composed deny-all provider (`wasi-cli-deny.wat`; a
//! `wasi-virt` adapter would virtualize the filesystem too, read-only), so the
//! released component imports exactly its world's interfaces and the
//! filesystem's: `wasi:filesystem/types`, `wasi:filesystem/preopens`, the
//! `wasi:io` interfaces they use, and `wasi:clocks/wall-clock` for its
//! `datetime` type.
//!
//! The decisions are the portable [`FilesRequest`] and [`completion`],
//! tested natively.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

/// The exact service type URI this component implements.
pub const FILES_SERVICE_URI: &str = "https://stateforward.org/scxml/services/files";
/// Where the granted `workspace` mount appears inside the component.
pub const WORKSPACE: &str = "/workspace";
/// Maximum bytes of a file name parameter.
pub const MAX_FILE_NAME_BYTES: usize = 255;
/// Maximum bytes the service reads from its seed file.
pub const MAX_SEED_BYTES: u64 = 64 * 1024;

/// Why the service rejected its start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesError {
    /// A file name parameter is not text naming one plain path component.
    InvalidName,
    /// `text` is not text, or `name` and `text` are not both given.
    InvalidText,
    /// A filesystem call failed, or the seed is not UTF-8 within
    /// [`MAX_SEED_BYTES`].
    Filesystem,
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
    /// The file to read, relative to [`WORKSPACE`].
    pub seed: Option<String>,
    /// The file to write and its text.
    pub write: Option<(String, String)>,
}

/// Whether `name` is one plain path component: non-empty, at most
/// [`MAX_FILE_NAME_BYTES`] bytes, not `.` or `..`, and free of `/`, `\`, and
/// control characters.
#[must_use]
pub fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FILE_NAME_BYTES
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

/// `text` as a JSON string literal (quotes, backslashes, and control
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
            control if control.is_control() && u32::from(control) < 0x20 => {
                let code = u32::from(control);
                out.push_str("\\u00");
                for digit in [code >> 4, code & 0xf] {
                    out.push(char::from_digit(digit, 16).unwrap_or('0'));
                }
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The completion data as JSON object text:
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
        path: "../../crates/scxml-wit/wit/scxml",
        // The world's `wasi:clocks` interfaces too.
        generate_all,
    });

    use std::io::{Read as _, Write as _};

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::telemetry::{self, Operation, Outcome};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, InvokeRequest,
    };

    use super::{FilesError, FilesRequest, Parameter};

    /// The service's result: `ok` with its done data, or its error.
    type Done = Result<Option<DataValue>, ComponentError>;

    struct Component;

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its tasks are spawned"
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
            let result = run(&request);
            telemetry::observe(
                Operation::Start,
                match &result {
                    Ok(_) => Outcome::Ok,
                    Err(failure) if failure.code == ErrorCode::InvalidInput => Outcome::Rejected,
                    Err(_) => Outcome::Failed,
                },
                None,
                None,
            );
            let data = result?;
            // No event; the future resolves at once with the result.
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

    /// Reads and writes as the request asks; the completion data.
    fn run(request: &InvokeRequest) -> Result<String, ComponentError> {
        if request.type_uri != super::FILES_SERVICE_URI {
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

    fn filesystem() -> ComponentError {
        files_error(FilesError::Filesystem)
    }

    fn path(name: &str) -> String {
        format!("{}/{name}", super::WORKSPACE)
    }

    fn read(name: &str) -> Result<String, ComponentError> {
        let file = std::fs::File::open(path(name)).map_err(|_| filesystem())?;
        let mut text = String::new();
        file.take(super::MAX_SEED_BYTES)
            .read_to_string(&mut text)
            .map_err(|_| filesystem())?;
        Ok(text)
    }

    fn write(name: &str, text: &str) -> Result<usize, ComponentError> {
        let mut file = std::fs::File::create(path(name)).map_err(|_| filesystem())?;
        file.write_all(text.as_bytes()).map_err(|_| filesystem())?;
        Ok(text.len())
    }

    fn files_error(failure: FilesError) -> ComponentError {
        match failure {
            FilesError::InvalidName => error(ErrorCode::InvalidInput, "a file name is invalid"),
            FilesError::InvalidText => {
                error(ErrorCode::InvalidInput, "`name` and `text` go together")
            }
            FilesError::Filesystem => error(ErrorCode::Unavailable, "a filesystem call failed"),
        }
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
mod tests {
    use super::{FilesError, FilesRequest, Parameter, completion, is_file_name, json_string};

    #[test]
    fn parameters_describe_one_read_and_one_write() {
        let request = FilesRequest::from_parameters([
            ("seed", Parameter::Text("seed.txt")),
            ("name", Parameter::Text("out.txt")),
            ("text", Parameter::Text("hello")),
            ("ignored", Parameter::Other),
        ])
        .expect("valid parameters");
        assert_eq!(request.seed.as_deref(), Some("seed.txt"));
        assert_eq!(
            request.write,
            Some(("out.txt".to_owned(), "hello".to_owned()))
        );
        assert_eq!(
            FilesRequest::from_parameters([]).expect("no parameters"),
            FilesRequest::default()
        );
    }

    #[test]
    fn names_are_single_plain_components() {
        for bad in ["", ".", "..", "a/b", "../x", "a\\b", "a\nb"] {
            assert!(!is_file_name(bad), "{bad:?}");
            assert_eq!(
                FilesRequest::from_parameters([("seed", Parameter::Text(bad))]),
                Err(FilesError::InvalidName)
            );
        }
        assert_eq!(
            FilesRequest::from_parameters([("seed", Parameter::Other)]),
            Err(FilesError::InvalidName)
        );
        assert!(is_file_name("notes.txt"));
        assert!(!is_file_name(&"a".repeat(super::MAX_FILE_NAME_BYTES + 1)));
    }

    #[test]
    fn a_write_needs_both_name_and_text() {
        assert_eq!(
            FilesRequest::from_parameters([("name", Parameter::Text("out.txt"))]),
            Err(FilesError::InvalidText)
        );
        assert_eq!(
            FilesRequest::from_parameters([
                ("name", Parameter::Text("out.txt")),
                ("text", Parameter::Other)
            ]),
            Err(FilesError::InvalidText)
        );
    }

    #[test]
    fn the_completion_is_a_json_object() {
        let text = completion(Some("a\"b\\c\n\u{1}é"), Some(5));
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["seed"], "a\"b\\c\n\u{1}é");
        assert_eq!(value["wrote"], 5);
        assert_eq!(completion(None, None), r#"{"seed":null,"wrote":null}"#);
        assert_eq!(json_string("x"), "\"x\"");
    }
}
