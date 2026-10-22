//! The `behavior` extension namespace:
//! `<behavior:instances_from_behavior_results>`,
//! `<behavior:replace_behavior_clauses>`, `<behavior:lifetime_errors>`,
//! `<behavior:verify_tick>` and `<behavior:verify_apply>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The original's behavior package (`behavior/storage.py`,
//! `behavior/instance.py`, `behavior/verify.py`): the inventory rows a
//! memory select returns read as behavior instances, the statements that
//! replace one behavior's row and triggers, the schedule bounds a
//! behavior's lifetime implies, the check of a persistent routine's
//! forced tick, and the check of a turn behavior's run on its live turn.
//! The memory ability applies the statements through the sql service and
//! the builder simulates the routine; these elements only build and read
//! values, so they are synchronous executable content.
//! Documents bind [`NAMESPACE_URI`] (conventionally with the prefix
//! `behavior`):
//!
//! ```xml
//! <behavior:instances_from_behavior_results location="behaviors"
//!     behaviors="_event.data.results[1]" triggers="_event.data.results[2]"/>
//! <behavior:replace_behavior_clauses location="statements" instance="accepted"/>
//! <behavior:lifetime_errors location="lifetime_errors" scxml="scxml_text"
//!     min_every="min_every_ms" turn_after_bound="turn_after_bound_ms"
//!     lifetimelocation="lifetime" scheduleslocation="schedules"/>
//! <behavior:verify_tick location="apply" ticks="ticks"/>
//! <behavior:verify_apply location="apply" done="child_data" turn="testTurn()"
//!     input="testInput()" targets="testTargets()" error="applyError()"/>
//! ```
//!
//! Every attribute but the locations is an expression of the document's
//! data model. Each element writes to its locations and never sends.
//!
//! # `<behavior:instances_from_behavior_results location="L" behaviors="B" triggers="T"/>`
//!
//! The original's `storage.instances_from_behavior_results`. `behaviors`
//! and `triggers` (required) each evaluate to one statement's result of a
//! `bot.ability.memory.output` (an object whose `rows` is an array of rows
//! with `columns` and `values`): the behavior rows, then the trigger rows
//! (`behavior_name`, `trigger`). `location` (required) receives one
//! instance per behavior row, in row order: every column of the row as a
//! member, `examples` parsed from its JSON text, `used_count` and
//! `failed_count` as numbers (ECMAScript's `Number` of their text: blank
//! is `0`, anything else that is not a number `null`), and `triggers` the
//! second value of each trigger row whose first value is the instance's
//! `name`, in row order.
//!
//! # `<behavior:replace_behavior_clauses location="L" instance="I"/>`
//!
//! The original's `storage.replace_behavior_clauses`: the statements that
//! delete a behavior's triggers and row and insert its replacement row and
//! each of its distinct triggers, in that order, as
//! `bot.ability.memory.input` statements `{sql, parameters}`. `instance`
//! (required) evaluates to an object named as the original's `Instance`:
//! `name` (a non-empty string), `source` (a string), and optionally
//! `description` (a string; otherwise empty), `examples` (any value,
//! written as `JSON.stringify` writes it; absent is `[]`), `lifetime` (a
//! string; absent is `turn`), `status` (a string; absent is `ACTIVE`),
//! `used_count` and `failed_count` (absent is `0`, written as `String`
//! writes them), `last_used_at` and `last_failed_at` (absent is empty)
//! and `triggers` (an array of strings; absent is none). The statements'
//! SQL is [`DELETE_TRIGGERS`], [`DELETE_BEHAVIOR`], [`INSERT_BEHAVIOR`]
//! and [`INSERT_TRIGGER`], portable to both memory engines.
//!
//! # `<behavior:lifetime_errors location="L" scxml="S" min_every="M"? turn_after_bound="A"? lifetimelocation="F"? scheduleslocation="C"?/>`
//!
//! The original's `instance._lifetime_errors` over an SCXML document, read
//! with an XML parser. `scxml` (required) evaluates to the document's text.
//! A transition that contains a `<send event="bot.behavior.tick">` ticks:
//! each delayed `<send>` (one with a `delay` or a `delayexpr`) of an event
//! a ticking transition takes is a schedule (the original's `every` and
//! `at`), any other an `after`. The document is `persistent` when it sends
//! `bot.behavior.tick` anywhere, else `turn` (the persistent routine
//! contract). `min_every` and `turn_after_bound` (optional) evaluate to the
//! original check's bounds in milliseconds, or `null`; with neither given
//! nothing is checked. `location` (required) receives at most 20
//! diagnostics `{kind: "lifetime", code: "E0009", message}`: a schedule
//! whose literal delay is shorter than `min_every`, a turn behavior's
//! `after` whose literal delay is longer than `turn_after_bound`, and a
//! persistent routine with no schedule. `lifetimelocation` (optional)
//! receives the lifetime, and `scheduleslocation` (optional) the events of
//! the schedules in document order, each once. A text that does not parse
//! as XML has no sends: its lifetime is `turn`, and it has no diagnostics
//! and no schedules.
//!
//! # `<behavior:verify_tick location="L" ticks="T"/>`
//!
//! The original's `verify._verify_tick` with `_selection_binding_errors`
//! on a persistent routine's forced tick. `ticks` (required) evaluates to
//! the `bot.behavior.tick` data the routine sent, in order. `location`
//! (required) receives the original's `E0008` messages, none when the
//! first tick carries `name`, `trigger` (`every` or `at`), `scheduled_at`
//! and `fired_at` and its `output` selects at least one event whose data
//! holds the tick's own values for any of those fields.
//!
//! # `<behavior:verify_apply location="L" done="D" turn="T" input="I" targets="G" error="E"?/>`
//!
//! The original's `verify_apply` on a turn behavior (`verify._apply_once`
//! with `_live_binding_values` and `_selection_binding_errors`): the
//! behavior ran once, invoked with the params `turn` and `input` of the
//! turn it is learned from. `done` (required) evaluates to the run's
//! donedata, `null` when it reached no top-level final state; `turn`
//! (required) to that turn's number; `input` (required) to that turn's
//! behavior input, an object; `targets` (required) to the sessions that
//! turn's output names, an array of strings; and `error` (optional) to the
//! name of the error event the run ended on, or `null`. `location`
//! (required) receives `E0008` messages, at most 20 (the original's for
//! nothing selected, its binding message adapted to `input`, and floppy's
//! for the run, the donedata contract and the targets): none when the
//! donedata is `{turn, handled, selected}` with the live turn,
//! `handled` true and at least one selection, each with an event, a target
//! among `targets` (floppy's check, decision D104: the candidates runner
//! sends each selection to its target, so an empty `targets` refuses every
//! selection) and data whose members agree with the string, number and
//! boolean members of `input` of the same names (JSON equality, so `true`
//! is not `1`); else why, an error or no donedata first.
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
//! closed signal event. No attribute carries a behavior, a row, a document
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

use std::collections::BTreeSet;

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use scxml_types::DataValue as Data;
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "behavior";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/behavior";

/// The SCXML namespace, whose `<send>` and `<transition>` the lifetime
/// check reads.
const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";

/// The event a persistent routine sends on every tick.
const TICK_EVENT: &str = "bot.behavior.tick";

/// The original `delete_behavior_clauses`' first clause: a behavior's
/// triggers (first, for engines without `ON DELETE CASCADE`).
pub const DELETE_TRIGGERS: &str = "DELETE FROM bot_behavior_trigger WHERE behavior_name = ?";

/// The original `delete_behavior_clauses`' second clause: a behavior's row.
pub const DELETE_BEHAVIOR: &str = "DELETE FROM bot_behavior WHERE name = ?";

/// The original `insert_behavior_clauses`' row: name, source, description,
/// examples JSON, lifetime, status, usage and failure counters; the status
/// reason empty and the status stamped now.
pub const INSERT_BEHAVIOR: &str = "INSERT INTO bot_behavior (name, source, description, examples_json, lifetime, status, status_reason, status_updated_at, used_count, last_used_at, failed_count, last_failed_at) VALUES (?, ?, ?, ?, ?, ?, '', replace(substr(CAST(CURRENT_TIMESTAMP AS TEXT), 1, 19), ' ', 'T') || 'Z', ?, ?, ?, ?)";

/// The original `insert_behavior_clauses`' trigger row.
pub const INSERT_TRIGGER: &str =
    "INSERT INTO bot_behavior_trigger (behavior_name, \"trigger\") VALUES (?, ?)";

/// The original's diagnostic help for E0009.
const LIFETIME_HELP: &str = " Help: A turn behavior lives for one turn and is silenced about a second after it starts: keep its delayed sends within that bound, or make it a persistent routine that sends bot.behavior.tick on every / at schedules. A persistent routine's every interval must be at least the minimum the check names.";

/// The most diagnostics the lifetime check writes.
const MAX_LIFETIME_ERRORS: usize = 20;

/// One attribute of an element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    /// Its namespace URI, when it has one.
    pub namespace_uri: Option<String>,
    /// Local name.
    pub local_name: String,
    /// Value exactly as authored.
    pub value: String,
}

/// One item of an element's flattened content, in document order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Content {
    /// Character data.
    Text(String),
    /// The start tag of a nested element.
    Start,
    /// The end tag of the innermost open nested element.
    End,
}

/// What `execute` did with the element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Executed {
    /// The element was executed.
    Handled,
    /// The local name is not one of this namespace's elements.
    NotHandled,
}

/// Why `execute` failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure<E> {
    /// SCXML `error.execution`, with a static detail.
    Execution(&'static str),
    /// A non-recoverable host failure, passed through unchanged.
    Host(E),
}

/// The host capabilities one `execute` call uses.
pub trait BehaviorHost {
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
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, value, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: BehaviorHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "instances_from_behavior_results" => instances_element(attributes, content, host),
        "replace_behavior_clauses" => replace_element(attributes, content, host),
        "lifetime_errors" => lifetime_element(attributes, content, host),
        "verify_tick" => verify_tick_element(attributes, content, host),
        "verify_apply" => verify_apply_element(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// The values of an element's unqualified attributes named in `names`, in
/// that order, each trimmed and non-empty; any other attribute is `refused`,
/// a repeated one `repeated`, an empty one `empty`.
fn attribute_values<'a, E, const N: usize>(
    attributes: &'a [Attribute],
    names: [&str; N],
    [refused, repeated, empty]: [&'static str; 3],
) -> Result<[Option<&'a str>; N], Failure<E>> {
    let mut values = [None; N];
    for attribute in attributes {
        let index = match attribute.namespace_uri {
            None => names.iter().position(|name| *name == attribute.local_name),
            Some(_) => None,
        }
        .ok_or(Failure::Execution(refused))?;
        let value = attribute.value.trim();
        if value.is_empty() {
            return Err(Failure::Execution(empty));
        }
        if values[index].replace(value).is_some() {
            return Err(Failure::Execution(repeated));
        }
    }
    Ok(values)
}

/// Refuses any content but white space with `detail`.
fn no_content<E>(content: &[Content], detail: &'static str) -> Result<(), Failure<E>> {
    if content
        .iter()
        .any(|item| !matches!(item, Content::Text(text) if text.trim().is_empty()))
    {
        return Err(Failure::Execution(detail));
    }
    Ok(())
}

/// Evaluates `expression` to JSON, else `failed` (it does not evaluate) or
/// `malformed` (it evaluates to no JSON value).
fn evaluate<H: BehaviorHost>(
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

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: BehaviorHost>(
    host: &mut H,
    location: &str,
    value: &Json,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    let value = data_of(value).ok_or(Failure::Execution(failed))?;
    host.assign(location, &value)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// A data value as JSON; `None` for `undefined`, an absent value, XML, or
/// a JSON text that does not parse.
fn json_of(value: &Data) -> Option<Json> {
    match value {
        Data::Null => Some(Json::Null),
        Data::Bool(value) => Some(Json::Bool(*value)),
        Data::Number(value) => Some(number(value.get())),
        Data::String(text) => Some(Json::String(text.clone())),
        Data::JsonArray(text) | Data::JsonObject(text) => serde_json::from_str(text).ok(),
        Data::Absent | Data::Undefined | Data::Xml(_) => None,
    }
}

/// A finite number as JSON: an integer when it is one that `f64` holds
/// exactly (ECMAScript prints it without a fraction), `null` when it is
/// not finite (ECMAScript's `JSON.stringify` of `NaN`).
fn number(value: f64) -> Json {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && value.abs() < EXACT {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "an integral value under 2^53 in magnitude converts exactly"
        )]
        return Json::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Json::Null, Json::Number)
}

/// The data value of a JSON value (canonical JSON text for arrays and
/// objects: `serde_json` sorts object members).
fn data_of(value: &Json) -> Option<Data> {
    Some(match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(value) => Data::Number(scxml_types::FiniteNumber::new(value.as_f64()?).ok()?),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(value.to_string()),
        Json::Object(_) => Data::JsonObject(value.to_string()),
    })
}

/// Executes `<behavior:instances_from_behavior_results>`.
fn instances_element<H: BehaviorHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, behaviors, triggers] = attribute_values(
        attributes,
        ["location", "behaviors", "triggers"],
        [
            "behavior:instances_from_behavior_results takes only the location, behaviors and triggers attributes",
            "behavior:instances_from_behavior_results repeats an attribute",
            "behavior:instances_from_behavior_results has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "behavior:instances_from_behavior_results requires a location",
    ))?;
    let behaviors = behaviors.ok_or(Failure::Execution(
        "behavior:instances_from_behavior_results requires behaviors",
    ))?;
    let triggers = triggers.ok_or(Failure::Execution(
        "behavior:instances_from_behavior_results requires triggers",
    ))?;
    no_content(
        content,
        "behavior:instances_from_behavior_results holds no content",
    )?;
    let behaviors = evaluate(
        host,
        behaviors,
        "behavior:instances_from_behavior_results behaviors did not evaluate",
        "behavior:instances_from_behavior_results behaviors is not a result",
    )?;
    let triggers = evaluate(
        host,
        triggers,
        "behavior:instances_from_behavior_results triggers did not evaluate",
        "behavior:instances_from_behavior_results triggers is not a result",
    )?;
    let instances =
        instances_from_behavior_results(&behaviors, &triggers).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Json::Array(instances),
        "behavior:instances_from_behavior_results could not assign the instances",
    )?;
    Ok(Executed::Handled)
}

/// One row of a statement's result: its columns and its values.
type Row<'a> = (&'a [Json], &'a [Json]);

/// The rows of one statement's result: each row's columns and values.
fn result_rows<'a>(
    result: &'a Json,
    malformed: &'static str,
) -> Result<Vec<Row<'a>>, &'static str> {
    let rows = result
        .get("rows")
        .and_then(Json::as_array)
        .ok_or(malformed)?;
    rows.iter()
        .map(|row| {
            let columns = row.get("columns").and_then(Json::as_array);
            let values = row.get("values").and_then(Json::as_array);
            match (columns, values) {
                (Some(columns), Some(values)) => Ok((columns.as_slice(), values.as_slice())),
                _ => Err(malformed),
            }
        })
        .collect()
}

/// The original's `instances_from_behavior_results` (see the crate
/// documentation, `<behavior:instances_from_behavior_results>`).
///
/// # Errors
///
/// A static detail when a result is not one or an `examples` text does not
/// parse.
pub fn instances_from_behavior_results(
    behaviors: &Json,
    triggers: &Json,
) -> Result<Vec<Json>, &'static str> {
    let behavior_rows = result_rows(
        behaviors,
        "behavior:instances_from_behavior_results behaviors is not a result with rows",
    )?;
    let trigger_rows = result_rows(
        triggers,
        "behavior:instances_from_behavior_results triggers is not a result with rows",
    )?;
    behavior_rows
        .into_iter()
        .map(|(columns, values)| {
            let mut instance = Map::new();
            for (index, column) in columns.iter().enumerate() {
                let Json::String(column) = column else {
                    return Err(
                        "behavior:instances_from_behavior_results behaviors has a column that is not a name",
                    );
                };
                instance.insert(
                    column.clone(),
                    values.get(index).cloned().unwrap_or(Json::Null),
                );
            }
            let examples = match instance.get("examples") {
                Some(Json::String(text)) => serde_json::from_str(text).map_err(|_| {
                    "behavior:instances_from_behavior_results has examples that are not JSON"
                })?,
                Some(other) => other.clone(),
                None => {
                    return Err(
                        "behavior:instances_from_behavior_results has a behavior row without examples",
                    );
                }
            };
            instance.insert("examples".into(), examples);
            for counter in ["used_count", "failed_count"] {
                let value = ecmascript_to_number(instance.get(counter));
                instance.insert(counter.into(), value);
            }
            let name = instance.get("name").cloned().unwrap_or(Json::Null);
            let triggers: Vec<Json> = trigger_rows
                .iter()
                .filter(|(_, values)| values.first() == Some(&name))
                .map(|(_, values)| values.get(1).cloned().unwrap_or(Json::Null))
                .collect();
            instance.insert("triggers".into(), Json::Array(triggers));
            Ok(Json::Object(instance))
        })
        .collect()
}

/// ECMAScript's `Number(value)` of a column value, as JSON (`NaN` is
/// `null`).
fn ecmascript_to_number(value: Option<&Json>) -> Json {
    match value {
        None | Some(Json::Array(_) | Json::Object(_)) => Json::Null,
        Some(Json::Null) => Json::from(0),
        Some(Json::Bool(value)) => Json::from(i64::from(*value)),
        Some(Json::Number(value)) => value.as_f64().map_or(Json::Null, number),
        Some(Json::String(text)) => {
            let text = text.trim();
            if text.is_empty() {
                return Json::from(0);
            }
            match text.parse::<f64>() {
                Ok(value) if value.is_finite() && is_decimal_literal(text) => number(value),
                _ => Json::Null,
            }
        }
    }
}

/// Whether `text` is an ECMAScript decimal literal with an optional sign
/// (what `Number` accepts but for hexadecimal, octal, binary and
/// `Infinity`).
fn is_decimal_literal(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (body, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    mantissa_ok && exponent_ok
}

/// Executes `<behavior:replace_behavior_clauses>`.
fn replace_element<H: BehaviorHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, instance] = attribute_values(
        attributes,
        ["location", "instance"],
        [
            "behavior:replace_behavior_clauses takes only the location and instance attributes",
            "behavior:replace_behavior_clauses repeats an attribute",
            "behavior:replace_behavior_clauses has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "behavior:replace_behavior_clauses requires a location",
    ))?;
    let instance = instance.ok_or(Failure::Execution(
        "behavior:replace_behavior_clauses requires an instance",
    ))?;
    no_content(
        content,
        "behavior:replace_behavior_clauses holds no content",
    )?;
    let instance = evaluate(
        host,
        instance,
        "behavior:replace_behavior_clauses instance did not evaluate",
        "behavior:replace_behavior_clauses instance is not an object",
    )?;
    let statements = replace_behavior_clauses(&instance).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Json::Array(statements),
        "behavior:replace_behavior_clauses could not assign the statements",
    )?;
    Ok(Executed::Handled)
}

/// One `bot.ability.memory.input` statement.
fn statement(sql: &str, parameters: Vec<Json>) -> Json {
    let mut statement = Map::new();
    statement.insert("sql".into(), Json::String(sql.into()));
    statement.insert("parameters".into(), Json::Array(parameters));
    Json::Object(statement)
}

/// The original's `replace_behavior_clauses` (see the crate documentation,
/// `<behavior:replace_behavior_clauses>`).
///
/// # Errors
///
/// A static detail when the instance is malformed.
pub fn replace_behavior_clauses(instance: &Json) -> Result<Vec<Json>, &'static str> {
    let Json::Object(instance) = instance else {
        return Err("behavior:replace_behavior_clauses instance is not an object");
    };
    let name = match instance.get("name") {
        Some(Json::String(name)) if !name.is_empty() => name.clone(),
        _ => return Err("behavior:replace_behavior_clauses instance has no name"),
    };
    let source = match instance.get("source") {
        Some(Json::String(source)) => source.clone(),
        _ => return Err("behavior:replace_behavior_clauses instance source is not a string"),
    };
    let text = |field: &str, absent: &str, malformed: &'static str| match instance.get(field) {
        None | Some(Json::Null) => Ok(absent.to_owned()),
        Some(Json::String(text)) => Ok(text.clone()),
        Some(_) => Err(malformed),
    };
    let description = text(
        "description",
        "",
        "behavior:replace_behavior_clauses instance description is not a string",
    )?;
    let lifetime = text(
        "lifetime",
        "turn",
        "behavior:replace_behavior_clauses instance lifetime is not a string",
    )?;
    let status = text(
        "status",
        "ACTIVE",
        "behavior:replace_behavior_clauses instance status is not a string",
    )?;
    let examples = js_stringify(instance.get("examples").unwrap_or(&Json::Array(Vec::new())));
    let counter = |field: &str| -> Result<Json, &'static str> {
        match instance.get(field) {
            None => Ok(Json::String("0".into())),
            Some(value) => ecmascript_string(value).map(Json::String).ok_or(
                "behavior:replace_behavior_clauses instance has a counter that is not a scalar",
            ),
        }
    };
    let stamp = |field: &str| -> Result<Json, &'static str> {
        match instance.get(field) {
            None => Ok(Json::String(String::new())),
            Some(value @ (Json::Null | Json::String(_))) => Ok(value.clone()),
            Some(_) => Err(
                "behavior:replace_behavior_clauses instance has a timestamp that is not a string",
            ),
        }
    };
    let triggers = match instance.get("triggers") {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Array(triggers)) if triggers.iter().all(Json::is_string) => triggers.clone(),
        Some(_) => {
            return Err(
                "behavior:replace_behavior_clauses instance triggers is not an array of strings",
            );
        }
    };
    let mut statements = vec![
        statement(DELETE_TRIGGERS, vec![Json::String(name.clone())]),
        statement(DELETE_BEHAVIOR, vec![Json::String(name.clone())]),
        statement(
            INSERT_BEHAVIOR,
            vec![
                Json::String(name.clone()),
                Json::String(source),
                Json::String(description),
                Json::String(examples),
                Json::String(lifetime),
                Json::String(status),
                counter("used_count")?,
                stamp("last_used_at")?,
                counter("failed_count")?,
                stamp("last_failed_at")?,
            ],
        ),
    ];
    let mut seen = BTreeSet::new();
    for trigger in triggers {
        if seen.insert(trigger.to_string()) {
            statements.push(statement(
                INSERT_TRIGGER,
                vec![Json::String(name.clone()), trigger],
            ));
        }
    }
    Ok(statements)
}

/// ECMAScript's `String(value)` of a scalar; `None` for an array or an
/// object.
fn ecmascript_string(value: &Json) -> Option<String> {
    match value {
        Json::Null => Some("null".into()),
        Json::Bool(value) => Some(if *value { "true" } else { "false" }.into()),
        Json::Number(value) => value.as_f64().map(ecmascript_number),
        Json::String(text) => Some(text.clone()),
        Json::Array(_) | Json::Object(_) => None,
    }
}

/// ECMAScript's `JSON.stringify` of a JSON value: compact, members in the
/// order given (the data model hands an element sorted members), numbers
/// and strings as ECMAScript writes them.
fn js_stringify(value: &Json) -> String {
    match value {
        Json::Null => "null".into(),
        Json::Bool(value) => if *value { "true" } else { "false" }.into(),
        Json::Number(value) => value
            .as_f64()
            .map_or_else(|| "null".into(), ecmascript_number),
        Json::String(text) => js_string(text),
        Json::Array(items) => {
            let items: Vec<String> = items.iter().map(js_stringify).collect();
            format!("[{}]", items.join(","))
        }
        Json::Object(members) => {
            let members: Vec<String> = members
                .iter()
                .map(|(key, value)| format!("{}:{}", js_string(key), js_stringify(value)))
                .collect();
            format!("{{{}}}", members.join(","))
        }
    }
}

/// A string as ECMAScript's `JSON.stringify` writes it: quote, backslash
/// and `\b \f \n \r \t` escaped short, the other control characters as
/// lower-case `\u00xx`, everything else as it is.
fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{0}'..='\u{1f}' => {
                out.push_str("\\u00");
                let code = u32::from(character);
                for shift in [4, 0] {
                    out.push(char::from_digit((code >> shift) & 0xf, 16).unwrap_or('0'));
                }
            }
            _ => out.push(character),
        }
    }
    out.push('"');
    out
}

/// A number as ECMAScript's `String` and `JSON.stringify` write it.
fn ecmascript_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return whole(value);
    }
    if (1e-6..1e21).contains(&value.abs()) {
        return value.to_string();
    }
    let (mantissa, exponent) = exponent_parts(value);
    match exponent.strip_prefix('-') {
        Some(_) => format!("{mantissa}e{exponent}"),
        None => format!("{mantissa}e+{exponent}"),
    }
}

/// A whole number's digits (`-0` is `0`).
fn whole(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    format!("{value:.0}")
}

/// The shortest exponent form's mantissa and exponent digits.
fn exponent_parts(value: f64) -> (String, String) {
    let text = format!("{value:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) => (mantissa.to_owned(), exponent.to_owned()),
        None => (text, "0".to_owned()),
    }
}

/// Executes `<behavior:lifetime_errors>`.
fn lifetime_element<H: BehaviorHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [
        location,
        scxml,
        min_every,
        turn_after_bound,
        lifetime_location,
        schedules_location,
    ] = attribute_values(
        attributes,
        [
            "location",
            "scxml",
            "min_every",
            "turn_after_bound",
            "lifetimelocation",
            "scheduleslocation",
        ],
        [
            "behavior:lifetime_errors takes only the location, scxml, min_every, turn_after_bound, lifetimelocation and scheduleslocation attributes",
            "behavior:lifetime_errors repeats an attribute",
            "behavior:lifetime_errors has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "behavior:lifetime_errors requires a location",
    ))?;
    let scxml = scxml.ok_or(Failure::Execution(
        "behavior:lifetime_errors requires an scxml",
    ))?;
    no_content(content, "behavior:lifetime_errors holds no content")?;
    let Json::String(text) = evaluate(
        host,
        scxml,
        "behavior:lifetime_errors scxml did not evaluate",
        "behavior:lifetime_errors scxml is not a string",
    )?
    else {
        return Err(Failure::Execution(
            "behavior:lifetime_errors scxml is not a string",
        ));
    };
    let mut bound = |expression: Option<&str>| -> Result<Option<f64>, Failure<H::Error>> {
        let Some(expression) = expression else {
            return Ok(None);
        };
        match evaluate(
            host,
            expression,
            "behavior:lifetime_errors a bound did not evaluate",
            "behavior:lifetime_errors a bound is not a number or null",
        )? {
            Json::Null => Ok(None),
            Json::Number(value) => Ok(value.as_f64()),
            _ => Err(Failure::Execution(
                "behavior:lifetime_errors a bound is not a number or null",
            )),
        }
    };
    let min_every = bound(min_every)?;
    let turn_after_bound = bound(turn_after_bound)?;
    let checked = lifetime_errors(&text, min_every, turn_after_bound);
    assign_to(
        host,
        location,
        &Json::Array(checked.errors),
        "behavior:lifetime_errors could not assign the diagnostics",
    )?;
    if let Some(lifetime_location) = lifetime_location {
        assign_to(
            host,
            lifetime_location,
            &Json::String(checked.lifetime.into()),
            "behavior:lifetime_errors could not assign the lifetime",
        )?;
    }
    if let Some(schedules_location) = schedules_location {
        assign_to(
            host,
            schedules_location,
            &Json::Array(checked.schedules.into_iter().map(Json::String).collect()),
            "behavior:lifetime_errors could not assign the schedules",
        )?;
    }
    Ok(Executed::Handled)
}

/// What the lifetime check finds in one document.
#[derive(Clone, Debug, PartialEq)]
pub struct Lifetime {
    /// `persistent` or `turn`.
    pub lifetime: &'static str,
    /// The E0009 diagnostics, at most 20.
    pub errors: Vec<Json>,
    /// The schedules' events in document order, each once.
    pub schedules: Vec<String>,
}

/// One delayed `<send>` of a document.
struct DelayedSend {
    /// Its literal `event`, when it has one.
    event: Option<String>,
    /// Its literal `delay` in milliseconds, when it has one that parses.
    delay: Option<f64>,
}

/// The sends of a document as the lifetime check reads them.
#[derive(Default)]
struct Sends {
    /// Whether any `<send>` sends `bot.behavior.tick`.
    ticks: bool,
    /// The events the ticking transitions take.
    ticking: BTreeSet<String>,
    /// Every delayed send, in document order.
    delayed: Vec<DelayedSend>,
}

/// One open element while the document is read: a transition carries its
/// event descriptors and whether it sends `bot.behavior.tick`.
enum Open {
    Transition { events: Vec<String>, ticks: bool },
    Other,
}

/// An unprefixed attribute's value, normalized as XML 1.0 normalizes it
/// (references resolved, white space characters as spaces); a
/// malformed attribute is an error.
fn attribute(start: &BytesStart<'_>, name: &str) -> Result<Option<String>, ()> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|_| ())?;
        if attribute.key.prefix().is_none()
            && attribute.key.local_name().as_ref() == name.as_bytes()
        {
            return attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|_| ());
        }
    }
    Ok(None)
}

/// Reads the SCXML sends and transitions of `text`; `None` when it does not
/// parse as XML.
fn read_sends(text: &str) -> Option<Sends> {
    let mut reader = NsReader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut sends = Sends::default();
    let mut open: Vec<Open> = Vec::new();
    loop {
        let (namespace, event) = reader.read_resolved_event().ok()?;
        let scxml = matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == SCXML_NAMESPACE.as_bytes());
        match event {
            Event::Start(start) => {
                let element = start_element(scxml, &start, &mut sends, &mut open)?;
                open.push(element);
            }
            // An empty element is never open: an empty transition sends
            // nothing.
            Event::Empty(start) => {
                start_element(scxml, &start, &mut sends, &mut open)?;
            }
            Event::End(_) => {
                if let Some(Open::Transition {
                    events,
                    ticks: true,
                }) = open.pop()
                {
                    sends.ticking.extend(events);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Some(sends)
}

/// Reads one start tag: a `<send>` is recorded (and marks the open
/// transitions when it sends `bot.behavior.tick`); a `<transition>` opens
/// with its event descriptors.
fn start_element(
    scxml: bool,
    start: &BytesStart<'_>,
    sends: &mut Sends,
    open: &mut [Open],
) -> Option<Open> {
    if !scxml {
        return Some(Open::Other);
    }
    match start.local_name().as_ref() {
        b"send" => {
            let event = attribute(start, "event").ok()?;
            if event.as_deref().map(str::trim) == Some(TICK_EVENT) {
                sends.ticks = true;
                for element in open.iter_mut() {
                    if let Open::Transition { ticks, .. } = element {
                        *ticks = true;
                    }
                }
            }
            let delay = attribute(start, "delay").ok()?;
            let delayexpr = attribute(start, "delayexpr").ok()?;
            if delay.is_some() || delayexpr.is_some() {
                sends.delayed.push(DelayedSend {
                    event,
                    delay: delay.as_deref().and_then(delay_ms),
                });
            }
            Some(Open::Other)
        }
        b"transition" => {
            let events = attribute(start, "event")
                .ok()?
                .map(|events| events.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default();
            Some(Open::Transition {
                events,
                ticks: false,
            })
        }
        _ => Some(Open::Other),
    }
}

/// A CSS2 time (an SCXML `delay`) in milliseconds: a decimal number then
/// `ms` or `s`, with optional white space around; `None` otherwise.
fn delay_ms(value: &str) -> Option<f64> {
    let value = value.trim();
    let (number, scale) = match value.strip_suffix("ms") {
        Some(number) => (number, 1.0),
        None => (value.strip_suffix('s')?, 1000.0),
    };
    let number = number.trim_end();
    let (whole, fraction) = match number.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (number, None),
    };
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let valid = match fraction {
        Some(fraction) => digits(whole) && !fraction.is_empty() && digits(fraction),
        None => !whole.is_empty() && digits(whole),
    };
    if !valid {
        return None;
    }
    number.parse::<f64>().ok().map(|number| number * scale)
}

/// ECMAScript's text of `ms` milliseconds in seconds: `(ms / 1000) + 's'`.
fn seconds(ms: f64) -> String {
    format!("{}s", ecmascript_number(ms / 1000.0))
}

/// One E0009 diagnostic.
fn lifetime_error(message: String) -> Json {
    let mut error = Map::new();
    error.insert("kind".into(), Json::String("lifetime".into()));
    error.insert("code".into(), Json::String("E0009".into()));
    error.insert("message".into(), Json::String(message));
    Json::Object(error)
}

/// The original's `_lifetime_errors` over `text` (see the crate
/// documentation, `<behavior:lifetime_errors>`).
#[must_use]
pub fn lifetime_errors(
    text: &str,
    min_every: Option<f64>,
    turn_after_bound: Option<f64>,
) -> Lifetime {
    let Some(sends) = read_sends(text) else {
        return Lifetime {
            lifetime: "turn",
            errors: Vec::new(),
            schedules: Vec::new(),
        };
    };
    let lifetime = if sends.ticks { "persistent" } else { "turn" };
    let delayed: Vec<(String, Option<f64>, bool)> = sends
        .delayed
        .iter()
        .map(|send| {
            let schedule = send
                .event
                .as_ref()
                .is_some_and(|event| sends.ticking.contains(event));
            (
                send.event.clone().unwrap_or_else(|| "undefined".into()),
                send.delay,
                schedule,
            )
        })
        .collect();
    let mut schedules: Vec<String> = Vec::new();
    for (event, _, schedule) in &delayed {
        if *schedule && !schedules.contains(event) {
            schedules.push(event.clone());
        }
    }
    let mut errors = Vec::new();
    if min_every.is_some() || turn_after_bound.is_some() {
        for (event, delay, schedule) in &delayed {
            let Some(delay) = *delay else { continue };
            if let Some(min_every) = min_every.filter(|min_every| *schedule && *min_every > delay) {
                errors.push(lifetime_error(format!(
                    "every (the {event} delay of {}) is shorter than the {}-second minimum routine interval.{LIFETIME_HELP}",
                    seconds(delay),
                    ecmascript_number(min_every / 1000.0)
                )));
            }
            if let Some(bound) =
                turn_after_bound.filter(|bound| !*schedule && lifetime == "turn" && delay > *bound)
            {
                errors.push(lifetime_error(format!(
                    "after (the {event} delay of {}) never fires in a turn behavior, which is silenced {} after it starts; shorten it or make the behavior a persistent routine.{LIFETIME_HELP}",
                    seconds(delay),
                    seconds(bound)
                )));
            }
        }
        if lifetime == "persistent" && !delayed.iter().any(|(_, _, schedule)| *schedule) {
            errors.push(lifetime_error(format!(
                "a persistent routine must schedule at least one transition with every or at: a delayed send of an event whose transition sends bot.behavior.tick.{LIFETIME_HELP}"
            )));
        }
        errors.truncate(MAX_LIFETIME_ERRORS);
    }
    Lifetime {
        lifetime,
        errors,
        schedules,
    }
}

/// Executes `<behavior:verify_tick>`.
fn verify_tick_element<H: BehaviorHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, ticks] = attribute_values(
        attributes,
        ["location", "ticks"],
        [
            "behavior:verify_tick takes only the location and ticks attributes",
            "behavior:verify_tick repeats an attribute",
            "behavior:verify_tick has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "behavior:verify_tick requires a location",
    ))?;
    let ticks = ticks.ok_or(Failure::Execution("behavior:verify_tick requires ticks"))?;
    no_content(content, "behavior:verify_tick holds no content")?;
    let Json::Array(ticks) = evaluate(
        host,
        ticks,
        "behavior:verify_tick ticks did not evaluate",
        "behavior:verify_tick ticks is not an array",
    )?
    else {
        return Err(Failure::Execution(
            "behavior:verify_tick ticks is not an array",
        ));
    };
    let messages = verify_tick(&ticks).into_iter().map(Json::String).collect();
    assign_to(
        host,
        location,
        &Json::Array(messages),
        "behavior:verify_tick could not assign the messages",
    )?;
    Ok(Executed::Handled)
}

/// The tick fields a selection's data must not contradict.
const LIVE_FIELDS: [&str; 4] = ["name", "trigger", "scheduled_at", "fired_at"];

/// The original's `_verify_tick` and `_selection_binding_errors` on the
/// first of `ticks` (see the crate documentation, `<behavior:verify_tick>`).
#[must_use]
pub fn verify_tick(ticks: &[Json]) -> Vec<String> {
    let Some(tick) = ticks.first() else {
        return vec!["persistent routine did not emit a tick for its first schedule.".into()];
    };
    let field = |name: &str| tick.get(name).and_then(Json::as_str);
    let carries = field("name").is_some_and(|name| !name.is_empty())
        && field("trigger").is_some_and(|trigger| ["every", "at"].contains(&trigger))
        && field("scheduled_at").is_some()
        && field("fired_at").is_some();
    if !tick.is_object() || !carries {
        return vec![
            "persistent routine tick must carry name, trigger (every or at), scheduled_at and fired_at."
                .into(),
        ];
    }
    let selections = match tick.get("output") {
        Some(Json::Array(output))
            if output
                .iter()
                .all(|selection| selection.get("event").is_some_and(Json::is_string)) =>
        {
            output
        }
        _ => {
            return vec![
                "behavior output must be a cognition event selection object (or list of them)."
                    .into(),
            ];
        }
    };
    if selections.is_empty() {
        return vec![NO_SELECTION.into()];
    }
    let live: Map<String, Json> = LIVE_FIELDS
        .iter()
        .filter_map(|field| {
            tick.get(*field)
                .map(|value| ((*field).to_owned(), value.clone()))
        })
        .collect();
    selection_binding_errors(selections, &live, "the tick")
}

/// The original's `_selection_binding_errors` after its shape checks: one
/// message for each selection data member whose live value differs, in
/// order; `source` names where the behavior reads the live values.
fn selection_binding_errors(
    selections: &[Json],
    live: &Map<String, Json>,
    source: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    for selection in selections {
        let Some(Json::Object(data)) = selection.get("data") else {
            continue;
        };
        for (key, value) in data {
            let Some(live) = live.get(key) else {
                continue;
            };
            if value != live {
                errors.push(format!(
                    "selection data[{}]={} does not match live event value {}; read identifiers from {source} at runtime.",
                    js_string(key),
                    js_stringify(value),
                    js_stringify(live)
                ));
            }
        }
    }
    errors
}

/// The most messages `<behavior:verify_apply>` writes.
const MAX_APPLY_ERRORS: usize = 20;

/// The original's message for an output that selects nothing (verify.py).
const NO_SELECTION: &str =
    "behavior output must select at least one event for the observed pattern.";

/// A turn behavior that did not finish on the live turn.
const NO_FINAL: &str = "turn behavior reached no top-level final state on the live turn's input within the turn's bound: it receives no events, so it decides with eventless transitions and ends at once.";

/// A turn behavior whose donedata is not the candidates runner's contract.
const NOT_THE_CONTRACT: &str = "turn behavior donedata must be the object {turn, handled, selected}: handled a boolean and selected a list of selections.";

/// `<behavior:verify_apply>`'s targets that are not strings.
const NOT_STRINGS: &str = "behavior:verify_apply targets is not an array of strings";

/// `<behavior:verify_apply>`'s error that is not a name.
const NOT_AN_ERROR: &str = "behavior:verify_apply error is not a string or null";

/// Executes `<behavior:verify_apply>`.
fn verify_apply_element<H: BehaviorHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, done, turn, input, targets, error] = attribute_values(
        attributes,
        ["location", "done", "turn", "input", "targets", "error"],
        [
            "behavior:verify_apply takes only the location, done, turn, input, targets and error attributes",
            "behavior:verify_apply repeats an attribute",
            "behavior:verify_apply has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "behavior:verify_apply requires a location",
    ))?;
    let done = done.ok_or(Failure::Execution("behavior:verify_apply requires done"))?;
    let turn = turn.ok_or(Failure::Execution("behavior:verify_apply requires a turn"))?;
    let input = input.ok_or(Failure::Execution(
        "behavior:verify_apply requires an input",
    ))?;
    let targets = targets.ok_or(Failure::Execution("behavior:verify_apply requires targets"))?;
    no_content(content, "behavior:verify_apply holds no content")?;
    let done = evaluate(
        host,
        done,
        "behavior:verify_apply done did not evaluate",
        "behavior:verify_apply done did not evaluate",
    )?;
    let turn = evaluate(
        host,
        turn,
        "behavior:verify_apply turn is not a number",
        "behavior:verify_apply turn is not a number",
    )?;
    if !turn.is_number() {
        return Err(Failure::Execution(
            "behavior:verify_apply turn is not a number",
        ));
    }
    let Json::Object(input) = evaluate(
        host,
        input,
        "behavior:verify_apply input is not an object",
        "behavior:verify_apply input is not an object",
    )?
    else {
        return Err(Failure::Execution(
            "behavior:verify_apply input is not an object",
        ));
    };
    let Json::Array(targets) = evaluate(host, targets, NOT_STRINGS, NOT_STRINGS)? else {
        return Err(Failure::Execution(NOT_STRINGS));
    };
    let targets = targets
        .into_iter()
        .map(|target| match target {
            Json::String(target) => Ok(target),
            _ => Err(Failure::Execution(NOT_STRINGS)),
        })
        .collect::<Result<Vec<String>, _>>()?;
    let error = match error {
        None => None,
        Some(error) => match evaluate(host, error, NOT_AN_ERROR, NOT_AN_ERROR)? {
            Json::Null => None,
            Json::String(error) => Some(error),
            _ => return Err(Failure::Execution(NOT_AN_ERROR)),
        },
    };
    let done = (!done.is_null()).then_some(done);
    let messages = verify_apply(done.as_ref(), &turn, &input, &targets, error.as_deref())
        .into_iter()
        .map(Json::String)
        .collect();
    assign_to(
        host,
        location,
        &Json::Array(messages),
        "behavior:verify_apply could not assign the messages",
    )?;
    Ok(Executed::Handled)
}

/// The original's `verify_apply` on a turn behavior with floppy's donedata
/// contract (the candidates runner's) and target check (decision D104); see
/// the crate documentation, `<behavior:verify_apply>`.
#[must_use]
pub fn verify_apply(
    done: Option<&Json>,
    turn: &Json,
    input: &Map<String, Json>,
    targets: &[String],
    error: Option<&str>,
) -> Vec<String> {
    if let Some(error) = error {
        return vec![format!(
            "turn behavior failed on the live turn's input ({error}) before it reached a top-level final state."
        )];
    }
    let Some(done) = done else {
        return vec![NO_FINAL.into()];
    };
    let (Some(done_turn), Some(Json::Bool(handled)), Some(Json::Array(selected))) =
        (done.get("turn"), done.get("handled"), done.get("selected"))
    else {
        return vec![NOT_THE_CONTRACT.into()];
    };
    if done_turn != turn {
        return vec![format!(
            "turn behavior donedata turn {} is not the live turn {}: return the turn param as it is.",
            js_stringify(done_turn),
            js_stringify(turn)
        )];
    }
    if !*handled || selected.is_empty() {
        return vec![NO_SELECTION.into()];
    }
    let offered = js_stringify(&Json::Array(
        targets.iter().cloned().map(Json::String).collect(),
    ));
    let mut errors = Vec::new();
    for (index, selection) in selected.iter().enumerate() {
        let Some(event) = selection
            .get("event")
            .and_then(Json::as_str)
            .filter(|event| !event.is_empty())
        else {
            errors.push(format!(
                "selected[{index}] has no event: each selection is an object {{event, target, data?, reason?}}."
            ));
            continue;
        };
        match selection.get("target") {
            Some(Json::String(target)) if !target.is_empty() => {
                if !targets.contains(target) {
                    errors.push(format!(
                        "selected[{index}] ({event}) names the target {}, which is not one of this turn's targets {offered}: name the session exactly as this turn's cognition_output names it.",
                        js_string(target)
                    ));
                }
            }
            _ => errors.push(format!(
                "selected[{index}] ({event}) names no target: name the session that receives its event, one of {offered}."
            )),
        }
    }
    errors.extend(selection_binding_errors(
        selected,
        &live_binding_values(input),
        "input",
    ));
    errors.truncate(MAX_APPLY_ERRORS);
    errors
}

/// The original's `_live_binding_values` of a data object: its string,
/// number and boolean members (not `null`, arrays or objects).
fn live_binding_values(data: &Map<String, Json>) -> Map<String, Json> {
    data.iter()
        .filter(|(_, value)| matches!(value, Json::String(_) | Json::Number(_) | Json::Bool(_)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "executable-content-extension",
        path: "../../scxml.rs/crates/scxml-wit/wit/scxml",
    });

    use std::cell::Cell;

    use exports::stateforward::scxml::executable_content_extension_api::Guest;
    use stateforward::scxml::data_model_host;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, ExecutableContentExtension,
        ExtensionContent, ExtensionOutcome, TraceContext,
    };

    use super::{Attribute, BehaviorHost, Content, Data, Executed, Failure};

    /// The lifecycle of this instance.
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Lifecycle {
        Created,
        Live,
        ShutDown,
    }

    thread_local! {
        static STATE: Cell<Lifecycle> = const { Cell::new(Lifecycle::Created) };
    }

    struct Component;

    /// The host capabilities of one `execute` call.
    struct Host;

    /// Evaluation and assignment failures (and a session with no data
    /// model) are document errors.
    fn recoverable(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::InvalidInput
                | ErrorCode::Execution
                | ErrorCode::Unavailable
                | ErrorCode::PayloadTooLarge
        )
    }

    impl BehaviorHost for Host {
        type Error = ComponentError;

        fn read(&mut self, expression: &str) -> Result<Result<Data, ()>, ComponentError> {
            match data_model_host::evaluate(expression) {
                Ok(value) => Ok(match value {
                    DataValue::Absent => Ok(Data::Absent),
                    DataValue::Undefined => Ok(Data::Undefined),
                    DataValue::Null => Ok(Data::Null),
                    DataValue::Boolean(value) => Ok(Data::Bool(value)),
                    DataValue::Number(value) => scxml_types::FiniteNumber::new(value)
                        .map(Data::Number)
                        .map_err(|_| ()),
                    DataValue::Text(text) => Ok(Data::String(text)),
                    DataValue::JsonArray(text) => Ok(Data::JsonArray(text)),
                    DataValue::JsonObject(text) => Ok(Data::JsonObject(text)),
                    DataValue::Xml(text) => Ok(Data::Xml(text)),
                }),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn assign(
            &mut self,
            location: &str,
            value: &Data,
        ) -> Result<Result<(), ()>, ComponentError> {
            let value = match value {
                Data::Null => DataValue::Null,
                Data::Bool(value) => DataValue::Boolean(*value),
                Data::Number(value) => DataValue::Number(value.get()),
                Data::String(text) => DataValue::Text(text.clone()),
                Data::JsonArray(text) => DataValue::JsonArray(text.clone()),
                Data::JsonObject(text) => DataValue::JsonObject(text.clone()),
                Data::Absent | Data::Undefined | Data::Xml(_) => return Ok(Err(())),
            };
            match data_model_host::assign(location, &value) {
                Ok(()) => Ok(Ok(())),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }
    }

    fn content(items: Vec<ExtensionContent>) -> Vec<Content> {
        items
            .into_iter()
            .map(|item| match item {
                ExtensionContent::Text(text) => Content::Text(text),
                ExtensionContent::StartElement(_) => Content::Start,
                ExtensionContent::EndElement(_) => Content::End,
            })
            .collect()
    }

    impl Guest for Component {
        fn namespace_uri() -> String {
            super::NAMESPACE_URI.into()
        }

        fn initialize() -> Result<(), ComponentError> {
            observed(Operation::Initialize, None, || {
                if STATE.get() != Lifecycle::Created {
                    return Err(error(ErrorCode::Protocol, "initialize was already called"));
                }
                STATE.set(Lifecycle::Live);
                Ok(())
            })
        }

        fn execute(
            element: ExecutableContentExtension,
            context: Option<TraceContext>,
        ) -> Result<ExtensionOutcome, ComponentError> {
            observed(Operation::Execute, context.as_ref(), || {
                if element.name.namespace_uri.as_deref() != Some(super::NAMESPACE_URI) {
                    return Err(error(
                        ErrorCode::Protocol,
                        "element namespace does not match extension component identity",
                    ));
                }
                if STATE.get() != Lifecycle::Live {
                    return Err(error(
                        ErrorCode::Protocol,
                        "execute outside the initialized lifecycle",
                    ));
                }
                let attributes: Vec<Attribute> = element
                    .attributes
                    .into_iter()
                    .map(|attribute| Attribute {
                        namespace_uri: attribute.name.namespace_uri,
                        local_name: attribute.name.local_name,
                        value: attribute.value,
                    })
                    .collect();
                let flattened = content(element.content);
                match super::execute(&element.name.local_name, &attributes, &flattened, &mut Host) {
                    Ok(Executed::Handled) => Ok(ExtensionOutcome::Handled),
                    Ok(Executed::NotHandled) => Ok(ExtensionOutcome::NotHandled),
                    Err(Failure::Execution(detail)) => Err(error(ErrorCode::Execution, detail)),
                    Err(Failure::Host(host)) => Err(host),
                }
            })
        }

        fn shutdown() -> Result<(), ComponentError> {
            observed(Operation::Shutdown, None, || {
                STATE.set(Lifecycle::ShutDown);
                Ok(())
            })
        }
    }

    /// Records one closed-label observation per entry point: the operation,
    /// its outcome, and one signal event (no attribute carries input).
    fn observed<T>(
        operation: Operation,
        context: Option<&TraceContext>,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(error) if error.code == ErrorCode::Execution => Outcome::Failed,
            Err(_) => Outcome::Rejected,
        };
        telemetry::observe(operation, outcome, None, context);
        telemetry::event(
            operation,
            if result.is_ok() {
                SignalEvent::TransformationCompleted
            } else {
                SignalEvent::InputRejected
            },
            context,
        );
        result
    }

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests;
