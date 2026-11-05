//! The `device` extension namespace: `<device:snapshot>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! A bot reads each device it holds from the device's live session, as the
//! original reads a device's state machine snapshot (decision D109): the
//! element reads another session of the run through
//! `session-snapshot-host.snapshot` with a target and assigns what the bot
//! needs of it to a data-model location. Executable content is synchronous
//! and quick, and the read is too: every other session of the run is between
//! machine calls while this one executes. Documents bind [`NAMESPACE_URI`]
//! (conventionally with the prefix `device`):
//!
//! ```xml
//! <device:snapshot location="reads.lamp" target="'#_scxml_' + owned_devices.lamp"/>
//! ```
//!
//! # `<device:snapshot location="L" target="T"/>`
//!
//! `target` (required) is an expression whose value, a string, names the
//! session to read as a `<send>` target does (`#_parent`, `#_<invokeid>` of
//! a live invoked session, `#_scxml_<sessionid>` of any running session of
//! the run). `location` (required) receives one JSON object:
//!
//! * `id`: the session's `_sessionid`;
//! * `state`: its active atomic states (its innermost active state ids), in
//!   document order, separated by one space;
//! * `attributes`: its data model, each declared `<data>` id mapped to its
//!   value (text, a number, a boolean, `null`, or a JSON array or object); a
//!   value with no portable form (`undefined`, absent, an opaque or
//!   oversized value, XML) is left out;
//! * `events`: the event names its active transitions accept, in the
//!   snapshot's walk order, each once: a descriptor `*` names no event and
//!   is not listed, and a descriptor `e.*` is listed as `e`.
//!
//! The element takes no other attribute and no content. A target that no
//! running session of the run answers (a session that is not running,
//! finished or failed, or a target that names none) is nothing to read: the
//! element assigns nothing and raises nothing, so `location` keeps its value
//! (decision D109, as the maintainer amended it on 2026-10-16). A malformed
//! element, a target that is not a string, a read the host fails (a bound),
//! a malformed snapshot, or a failed assignment is an `execution` error
//! with a static detail (SCXML `error.execution`); nothing is assigned then.
//!
//! # Bounds
//!
//! The snapshot is bounded by the host (32 MiB, 64 reads per `execute`).
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries a target, a session id, a state, or a
//! location, so the cardinality is the product of the three closed enums.
//! The host adds one `session-snapshot` span per snapshot read with only its
//! closed outcome.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

pub use floppy_guest::element::{Attribute, Content, Executed, Failure};

use scxml_types::DataValue as Data;
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "device";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/device";

/// A section of the session snapshot a read can omit
/// (`session-snapshot-host.section`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Section {
    /// SCXML 5.10 system variables.
    System,
    /// User data-model variables.
    DataModel,
    /// The internal and external event queues.
    Queues,
    /// Pending delayed sends.
    PendingSends,
    /// Transitions an event could trigger.
    Transitions,
}

/// The sections a read omits: it uses only the configuration, the data
/// model and the transitions.
pub const SNAPSHOT_EXCLUDE: [Section; 3] =
    [Section::System, Section::Queues, Section::PendingSends];

/// What a host read of another session gave.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapshotRead {
    /// The session's `scxml.session-snapshot/v2` document.
    Document(String),
    /// No running session of the run answers the target (it is not
    /// running, or the target names none).
    NotRunning,
    /// The read failed (a bound, or the host).
    Failed,
}

/// The host capabilities one `execute` call uses.
pub trait DeviceHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one value expression in the current data model;
    /// `Ok(Err(()))` is a recoverable evaluation failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn read(&mut self, expression: &str) -> Result<Result<Data, ()>, Self::Error>;

    /// Assigns one value to one data-model location; `Ok(Err(()))` is a
    /// recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, Self::Error>;

    /// The `scxml.session-snapshot/v2` document of the session `target`
    /// names, with `exclude` omitted, or why there is none.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn snapshot(&mut self, exclude: &[Section], target: &str) -> Result<SnapshotRead, Self::Error>;
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, target, snapshot, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: DeviceHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "snapshot" => snapshot(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// Executes `<device:snapshot>`.
fn snapshot<H: DeviceHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let mut location = None;
    let mut target = None;
    for attribute in attributes {
        let slot = match (
            attribute.namespace_uri.as_deref(),
            attribute.local_name.as_str(),
        ) {
            (None, "location") => &mut location,
            (None, "target") => &mut target,
            _ => {
                return Err(Failure::Execution(
                    "device:snapshot takes only the location and target attributes",
                ));
            }
        };
        if slot.replace(attribute.value.trim()).is_some() {
            return Err(Failure::Execution("device:snapshot repeats an attribute"));
        }
    }
    let location = location
        .filter(|value| !value.is_empty())
        .ok_or(Failure::Execution("device:snapshot needs a location"))?;
    let target = target
        .filter(|value| !value.is_empty())
        .ok_or(Failure::Execution("device:snapshot needs a target"))?;
    if content
        .iter()
        .any(|item| !matches!(item, Content::Text(text) if text.trim().is_empty()))
    {
        return Err(Failure::Execution("device:snapshot takes no content"));
    }
    let Data::String(target) = host
        .read(target)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("device:snapshot's target is not a string"))?
    else {
        return Err(Failure::Execution(
            "device:snapshot's target is not a string",
        ));
    };
    let document = match host
        .snapshot(&SNAPSHOT_EXCLUDE, &target)
        .map_err(Failure::Host)?
    {
        SnapshotRead::Document(document) => document,
        // Nothing to read (decision D109, as amended): the location is left
        // as it was.
        SnapshotRead::NotRunning => return Ok(Executed::Handled),
        SnapshotRead::Failed => {
            return Err(Failure::Execution(
                "device:snapshot cannot read the target session",
            ));
        }
    };
    let read =
        session_read(&document).ok_or(Failure::Execution("the session snapshot is malformed"))?;
    host.assign(location, &Data::JsonObject(read.to_string()))
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("device:snapshot cannot assign its location"))?;
    Ok(Executed::Handled)
}

/// The read `{id, state, attributes, events}` of one
/// `scxml.session-snapshot/v2` document (see the crate documentation), or
/// `None` when the document is malformed.
#[must_use]
pub fn session_read(document: &str) -> Option<Json> {
    let document: Json = serde_json::from_str(document).ok()?;
    let root = document.pointer("/snapshot/sessions/0")?;
    let id = root.get("session_id")?.as_str()?;
    let state = root
        .pointer("/configuration/atomic")?
        .as_array()?
        .iter()
        .map(Json::as_str)
        .collect::<Option<Vec<_>>>()?
        .join(" ");
    let mut attributes = Map::new();
    // The data model is absent before the session's data model is
    // initialized (session-snapshot.md, "Data-model image").
    if let Some(data_model) = root.get("data_model") {
        for entry in data_model.as_array()? {
            let name = entry.get("name")?.as_str()?;
            if let Some(value) = portable(entry.get("value")?).ok()? {
                attributes.insert(name.to_owned(), value);
            }
        }
    }
    let mut events: Vec<Json> = Vec::new();
    if let Some(transitions) = root.get("transitions") {
        for transition in transitions.as_array()? {
            for descriptor in transition.get("events")?.as_array()? {
                let descriptor = descriptor.as_str()?;
                if descriptor == "*" {
                    continue;
                }
                let event = Json::from(descriptor.strip_suffix(".*").unwrap_or(descriptor));
                if !events.contains(&event) {
                    events.push(event);
                }
            }
        }
    }
    let mut read = Map::new();
    read.insert("id".into(), id.into());
    read.insert("state".into(), state.into());
    read.insert("attributes".into(), Json::Object(attributes));
    read.insert("events".into(), Json::Array(events));
    Some(Json::Object(read))
}

/// The JSON of one snapshot value `{type, value?}`: `Ok(None)` for a
/// value with no portable form, `Err(())` for a malformed one.
fn portable(value: &Json) -> Result<Option<Json>, ()> {
    // An opaque ECMAScript value is shown by its `typeof` and string form.
    if value.get("typeof").is_some() {
        return Ok(None);
    }
    let inner = || value.get("value").ok_or(());
    let text = || inner()?.as_str().ok_or(());
    Ok(Some(
        match value.get("type").and_then(Json::as_str).ok_or(())? {
            "null" => Json::Null,
            "boolean" => match inner()? {
                Json::Bool(value) => Json::Bool(*value),
                Json::String(text) => Json::Bool(text.parse().map_err(|_| ())?),
                _ => return Err(()),
            },
            "number" => {
                let number = match inner()? {
                    Json::Number(number) => number.as_f64().ok_or(())?,
                    Json::String(text) => text.parse().map_err(|_| ())?,
                    _ => return Err(()),
                };
                number_json(number).ok_or(())?
            }
            "text" => Json::String(text()?.to_owned()),
            "json-array" | "json-object" => serde_json::from_str(text()?).map_err(|_| ())?,
            "absent" | "undefined" | "xml" | "oversized" => return Ok(None),
            _ => return Err(()),
        },
    ))
}

/// A finite number as JSON: an integer when it is one that `f64` holds
/// exactly (ECMAScript prints it without a fraction).
fn number_json(value: f64) -> Option<Json> {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && value.abs() < EXACT {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "an integral value under 2^53 in magnitude converts exactly"
        )]
        return Some(Json::from(value as i64));
    }
    serde_json::Number::from_f64(value).map(Json::Number)
}

floppy_guest::extension_guest!(
    host = {
        use super::{DeviceHost, Section, SnapshotRead};
        use stateforward::scxml::session_snapshot_host;

        impl DeviceHost for Host {
            type Error = ComponentError;

            fn read(&mut self, expression: &str) -> Result<Result<Data, ()>, ComponentError> {
                match data_model_host::evaluate(expression) {
                    Ok(DataValue::Text(text)) => Ok(Ok(Data::String(text))),
                    Ok(_) => Ok(Err(())),
                    Err(error) if recoverable(error.code) => Ok(Err(())),
                    Err(error) => Err(error),
                }
            }

            fn assign(
                &mut self,
                location: &str,
                value: &Data,
            ) -> Result<Result<(), ()>, ComponentError> {
                let Data::JsonObject(text) = value else {
                    return Ok(Err(()));
                };
                match data_model_host::assign(location, &DataValue::JsonObject(text.clone())) {
                    Ok(()) => Ok(Ok(())),
                    Err(error) if recoverable(error.code) => Ok(Err(())),
                    Err(error) => Err(error),
                }
            }

            fn snapshot(
                &mut self,
                exclude: &[Section],
                target: &str,
            ) -> Result<SnapshotRead, ComponentError> {
                let exclude: Vec<session_snapshot_host::Section> = exclude
                    .iter()
                    .map(|section| match section {
                        Section::System => session_snapshot_host::Section::System,
                        Section::DataModel => session_snapshot_host::Section::DataModel,
                        Section::Queues => session_snapshot_host::Section::Queues,
                        Section::PendingSends => session_snapshot_host::Section::PendingSends,
                        Section::Transitions => session_snapshot_host::Section::Transitions,
                    })
                    .collect();
                match session_snapshot_host::snapshot(&exclude, Some(target)) {
                    Ok(document) => Ok(SnapshotRead::Document(document)),
                    // No running session of the run answers the target: nothing
                    // to read.
                    Err(error) if error.code == ErrorCode::Unavailable => {
                        Ok(SnapshotRead::NotRunning)
                    }
                    // The call bound or a document bound: the element cannot
                    // read it (`error.execution`).
                    Err(error)
                        if recoverable(error.code)
                            || error.code == ErrorCode::CapacityExhausted =>
                    {
                        Ok(SnapshotRead::Failed)
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
);

#[cfg(test)]
mod tests;
