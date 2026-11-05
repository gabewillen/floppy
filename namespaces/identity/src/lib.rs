//! The `identity` extension namespace: `<identity:hears_name>` and
//! `<identity:sorted_identities>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Whether a text names the bot (`hears_name`, the text recognizer), and
//! the canonical order of a collection of participant identities
//! (`sorted_identities`). Both are synchronous and quick, so they are
//! executable content. Documents bind [`NAMESPACE_URI`] (conventionally with the prefix
//! `identity`):
//!
//! ```xml
//! <identity:hears_name location="addressed" name="name" text="heard"/>
//! <identity:sorted_identities location="turn_sources" ids="input.source_ids"/>
//! ```
//!
//! Every attribute but `location` is an expression of the document's data
//! model. Each element writes to its location and never sends.
//!
//! # Identities
//!
//! An identity is a name (a string) or an embedding (an array of numbers);
//! floppy's conversation carries voice embeddings as identities.
//!
//! # `<identity:hears_name location="L" name="N" text="T"/>`
//!
//! Whether `text` names the bot (the text recognizer). `name` and `text`
//! (required) each evaluate to a
//! string, or to `null` (also `undefined`) when there is none. Each is
//! split on white space into tokens kept to their letters and digits and
//! lowercased, the empty ones dropped; the name is heard when its tokens
//! appear as a whole run among the text's (so "Al" is not heard in
//! "always"). A name with no tokens, or a missing name or text, is never
//! heard. `location` (required) receives `true` or `false`.
//!
//! # `<identity:sorted_identities location="L" ids="I"/>`
//!
//! `ids` (required) evaluates to an array of identities; `location`
//! (required) receives them in identity order ([`sort_key`]): names first,
//! by their text in code point order, then embeddings by their JSON text
//! in code point order (each component the JSON number the data model
//! hands over). The sort is stable and keeps repeated identities.
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
//! closed signal event. No attribute carries a name, a text, an identity,
//! or a location, so the cardinality is the product of the three closed
//! enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use scxml_types::DataValue as Data;
use serde_json::Value as Json;

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "identity";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/identity";

pub use floppy_guest::element::{Attribute, Content, Executed, Failure, Host};
use floppy_guest::element::{assign_to, attribute_values, evaluate, no_content};

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
        "hears_name" => hears_name_element(attributes, content, host),
        "sorted_identities" => sorted_identities_element(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// Executes `<identity:hears_name>`.
fn hears_name_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, name, text] = attribute_values(
        attributes,
        ["location", "name", "text"],
        [
            "identity:hears_name takes only the location, name and text attributes",
            "identity:hears_name repeats an attribute",
            "identity:hears_name has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "identity:hears_name requires a location",
    ))?;
    let name = name.ok_or(Failure::Execution("identity:hears_name requires a name"))?;
    let text = text.ok_or(Failure::Execution("identity:hears_name requires a text"))?;
    no_content(content, "identity:hears_name holds no content")?;
    let mut string_or_null = |expression: &str,
                              failed: &'static str,
                              malformed: &'static str|
     -> Result<Option<String>, Failure<H::Error>> {
        match evaluate(host, expression, failed)? {
            Data::String(value) => Ok(Some(value)),
            Data::Null | Data::Undefined | Data::Absent => Ok(None),
            _ => Err(Failure::Execution(malformed)),
        }
    };
    let name = string_or_null(
        name,
        "identity:hears_name name did not evaluate",
        "identity:hears_name name is not a string or null",
    )?;
    let text = string_or_null(
        text,
        "identity:hears_name text did not evaluate",
        "identity:hears_name text is not a string or null",
    )?;
    let addressed = match (name, text) {
        (Some(name), Some(text)) => hears_name(&text, &name),
        _ => false,
    };
    assign_to(
        host,
        location,
        &Data::Bool(addressed),
        "identity:hears_name could not assign whether the name was heard",
    )?;
    Ok(Executed::Handled)
}

/// The white-space separated words of `text`, each kept to its letters and digits and lowercased, the empty
/// ones dropped.
fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

/// Whether `name`'s tokens appear
/// as a whole run among `text`'s; a name with no tokens is never heard.
#[must_use]
pub fn hears_name(text: &str, name: &str) -> bool {
    let wanted = tokens(name);
    !wanted.is_empty()
        && tokens(text)
            .windows(wanted.len())
            .any(|run| run == wanted.as_slice())
}

/// Executes `<identity:sorted_identities>`.
fn sorted_identities_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, ids] = attribute_values(
        attributes,
        ["location", "ids"],
        [
            "identity:sorted_identities takes only the location and ids attributes",
            "identity:sorted_identities repeats an attribute",
            "identity:sorted_identities has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "identity:sorted_identities requires a location",
    ))?;
    let ids = ids.ok_or(Failure::Execution(
        "identity:sorted_identities requires ids",
    ))?;
    no_content(content, "identity:sorted_identities holds no content")?;
    let ids = match evaluate(host, ids, "identity:sorted_identities ids did not evaluate")? {
        Data::JsonArray(text) => match serde_json::from_str::<Json>(&text) {
            Ok(Json::Array(ids)) if ids.iter().all(is_identity) => ids,
            _ => {
                return Err(Failure::Execution(
                    "identity:sorted_identities ids is not an array of identities",
                ));
            }
        },
        _ => {
            return Err(Failure::Execution(
                "identity:sorted_identities ids is not an array of identities",
            ));
        }
    };
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(sorted_identities(&ids)).to_string()),
        "identity:sorted_identities could not assign the identities",
    )?;
    Ok(Executed::Handled)
}

/// An identity: a string, or an array of numbers (an embedding).
#[must_use]
pub fn is_identity(value: &Json) -> bool {
    match value {
        Json::String(_) => true,
        Json::Array(values) => values.iter().all(Json::is_number),
        _ => false,
    }
}

/// `ids` in identity order
/// ([`sort_key`]), stable, repeats kept.
#[must_use]
pub fn sorted_identities(ids: &[Json]) -> Vec<Json> {
    let mut keyed: Vec<(String, &Json)> = ids.iter().map(|id| (sort_key(id), id)).collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.into_iter().map(|(_, id)| id.clone()).collect()
}

/// An identity's sort key, compared in code point order: `0` and a
/// name's text, or `1` and an embedding's JSON text (so every name sorts
/// before every embedding).
#[must_use]
pub fn sort_key(id: &Json) -> String {
    match id {
        Json::String(name) => format!("0{name}"),
        other @ Json::Array(_) => format!("1{other}"),
        other => format!("2{other}"),
    }
}

floppy_guest::extension_guest!();

#[cfg(test)]
mod tests;
