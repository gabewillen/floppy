//! The `autonomy` extension namespace: `<autonomy:behavior_input_payload>`
//! as a `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The input a learned behavior runs with: the data of the turn's stimulus
//! with the turn's focus added. Autonomy and the reflection (which tells
//! the builder what a behavior will run with) build it alike. Documents
//! bind [`NAMESPACE_URI`] (conventionally with the prefix `autonomy`):
//!
//! ```xml
//! <autonomy:behavior_input_payload location="behavior_input" stimulus="stimulus"
//!     focus="input.focus" focus_candidates="input.focus_candidates"/>
//! ```
//!
//! Every attribute but `location` is an expression of the document's data
//! model. The element writes to its location and never sends.
//!
//! # `<autonomy:behavior_input_payload location="L" stimulus="S" focus="F" focus_candidates="C" environment="E"? owned_devices="O"? targets="G"?/>`
//!
//! The input a learned behavior runs with, built from a cognition input.
//! `stimulus` (required) evaluates to the turn's stimulus envelope (an
//! object; its `data` is the stimulus data), `focus` (required) to the
//! turn's focus and `focus_candidates` (required) to its focus candidates.
//! `environment`, `owned_devices` and `targets` (optional) evaluate to
//! read-only snapshots the turn carries: the environment the bot perceived
//! itself in, the devices it owns as it read them this turn, and the
//! sessions the turn's choices offer (each offered event's name naming the
//! session id that receives it, which the invoking stage resolves a
//! selection's recipient against at send time). Each is copied into the
//! payload as the member of its name when the expression evaluates to a
//! value, and left out when it evaluates to `undefined` — a caller passes
//! what its turn has, and one without a snapshot passes nothing.
//! `location` (required) receives the stimulus data's members when the
//! data is an object (not an array), else `{event: data}` (a missing data
//! is left out), with `focus`, `focus_candidates`, `environment`,
//! `owned_devices` and `targets` set to the values given, whatever they
//! are (one that evaluates to `undefined` is left out, as ECMAScript's
//! `JSON` leaves it out).
//!
//! # Failures
//!
//! A malformed element (a missing, empty, unknown or repeated attribute,
//! or any content but white space), an expression that fails or evaluates
//! to a value of the wrong kind, or a failed assignment is an `execution`
//! error with a static detail (SCXML `error.execution`); nothing is
//! assigned then. A host failure that is not recoverable is passed through
//! unchanged.
//!
//! # Bounds
//!
//! The data model bounds the values it hands this component.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one
//! closed signal event. No attribute carries a stimulus, a focus or a
//! location, so the cardinality is the product of the three closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use scxml_types::DataValue as Data;
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "autonomy";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/autonomy";

pub use floppy_guest::element::{Attribute, Content, Executed, Failure, Host};
use floppy_guest::element::{assign_to, attribute_values, data_of, json_of, no_content};

/// Assigns a JSON value to `location`, else `failed`.
fn assign_json<H: Host>(
    host: &mut H,
    location: &str,
    value: &Json,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    let value = data_of(value).ok_or(Failure::Execution(failed))?;
    assign_to(host, location, &value, failed)
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, value, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: Host>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "behavior_input_payload" => payload_element(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// Evaluates `expression` to JSON, else `failed` (it does not evaluate) or
/// `malformed` (it evaluates to no JSON value).
fn evaluate_json<H: Host>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Json, Failure<H::Error>> {
    let value = host
        .read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))?;
    json_of(&value).ok_or(Failure::Execution(malformed))
}

/// Evaluates `expression` to JSON, `None` for `undefined` (a missing
/// member), else `failed` (it does not evaluate) or `malformed` (XML or a
/// JSON text that does not parse).
fn evaluate_optional<H: Host>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Option<Json>, Failure<H::Error>> {
    let value = host
        .read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))?;
    match value {
        Data::Absent | Data::Undefined => Ok(None),
        value => json_of(&value)
            .map(Some)
            .ok_or(Failure::Execution(malformed)),
    }
}

/// Executes `<autonomy:behavior_input_payload>`.
fn payload_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [
        location,
        stimulus,
        focus,
        focus_candidates,
        environment,
        owned_devices,
        targets,
    ] = attribute_values(
        attributes,
        [
            "location",
            "stimulus",
            "focus",
            "focus_candidates",
            "environment",
            "owned_devices",
            "targets",
        ],
        [
            "autonomy:behavior_input_payload takes only the location, stimulus, focus, focus_candidates, environment, owned_devices and targets attributes",
            "autonomy:behavior_input_payload repeats an attribute",
            "autonomy:behavior_input_payload has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires a location",
    ))?;
    let stimulus = stimulus.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires a stimulus",
    ))?;
    let focus = focus.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires a focus",
    ))?;
    let focus_candidates = focus_candidates.ok_or(Failure::Execution(
        "autonomy:behavior_input_payload requires focus_candidates",
    ))?;
    no_content(content, "autonomy:behavior_input_payload holds no content")?;
    let Json::Object(stimulus) = evaluate_json(
        host,
        stimulus,
        "autonomy:behavior_input_payload stimulus did not evaluate",
        "autonomy:behavior_input_payload stimulus is not an object",
    )?
    else {
        return Err(Failure::Execution(
            "autonomy:behavior_input_payload stimulus is not an object",
        ));
    };
    let focus = evaluate_optional(
        host,
        focus,
        "autonomy:behavior_input_payload focus did not evaluate",
        "autonomy:behavior_input_payload focus is not data",
    )?;
    let focus_candidates = evaluate_optional(
        host,
        focus_candidates,
        "autonomy:behavior_input_payload focus_candidates did not evaluate",
        "autonomy:behavior_input_payload focus_candidates is not data",
    )?;
    let environment = evaluate_snapshot(
        host,
        environment,
        "autonomy:behavior_input_payload environment did not evaluate",
        "autonomy:behavior_input_payload environment is not data",
    )?;
    let owned_devices = evaluate_snapshot(
        host,
        owned_devices,
        "autonomy:behavior_input_payload owned_devices did not evaluate",
        "autonomy:behavior_input_payload owned_devices is not data",
    )?;
    let targets = evaluate_snapshot(
        host,
        targets,
        "autonomy:behavior_input_payload targets did not evaluate",
        "autonomy:behavior_input_payload targets is not data",
    )?;
    let payload = behavior_input_payload(
        stimulus.get("data"),
        focus,
        focus_candidates,
        environment,
        owned_devices,
        targets,
    );
    assign_json(
        host,
        location,
        &Json::Object(payload),
        "autonomy:behavior_input_payload could not assign the payload",
    )?;
    Ok(Executed::Handled)
}

/// One optional snapshot attribute: `None` when the element does not carry
/// it or its expression evaluates to `undefined` (both are left out of the
/// payload), and the value when it evaluates to one.
fn evaluate_snapshot<H: Host>(
    host: &mut H,
    expression: Option<&str>,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Option<Json>, Failure<H::Error>> {
    match expression {
        None => Ok(None),
        Some(expression) => evaluate_optional(host, expression, failed, malformed),
    }
}

/// The input a learned behavior runs with (see the crate documentation):
/// the stimulus data's members, or `{event: data}`, with the focus and the
/// read-only snapshots set. Each of the five is `None` (ECMAScript's
/// `undefined`, or an attribute the caller did not pass) when it is left
/// out, and `Some` when it is set (a `null` value included).
#[must_use]
pub fn behavior_input_payload(
    data: Option<&Json>,
    focus: Option<Json>,
    focus_candidates: Option<Json>,
    environment: Option<Json>,
    owned_devices: Option<Json>,
    targets: Option<Json>,
) -> Map<String, Json> {
    let mut payload = match data {
        Some(Json::Object(members)) => members.clone(),
        Some(other) => {
            let mut payload = Map::new();
            payload.insert("event".into(), other.clone());
            payload
        }
        None => Map::new(),
    };
    for (name, value) in [
        ("focus", focus),
        ("focus_candidates", focus_candidates),
        ("environment", environment),
        ("owned_devices", owned_devices),
        ("targets", targets),
    ] {
        match value {
            Some(value) => payload.insert(name.into(), value),
            None => payload.remove(name),
        };
    }
    payload
}

floppy_guest::extension_guest!();

#[cfg(test)]
mod tests;
