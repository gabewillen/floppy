//! The `migration` extension namespace: `<migration:plan>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Alembic's revision graph walk, agnostic of the database (the user's
//! direction: "make it like alembic that's agnostic of the database"): it
//! knows revisions, never an engine, never SQL. The migration ability
//! (`abilities/migration.scx`) reads the current revision through the sql
//! service, plans with this element, and applies the planned statements in
//! one transaction. Documents bind [`NAMESPACE_URI`] (conventionally with
//! the prefix `migration`):
//!
//! ```xml
//! <migration:plan location="plan" revisions="revisions" current="current" target="'head'"/>
//! ```
//!
//! # `<migration:plan location="L" revisions="R" current="C" target="T"/>`
//!
//! `revisions` (required) evaluates to an array of revisions, each an
//! object `{revision, down_revision, upgrade, downgrade}` (Alembic's
//! script: `revision` a non-empty string, `down_revision` the revision it
//! follows or `null` for the first, `upgrade` and `downgrade` arrays of
//! statements, each an object the element passes through untouched; other
//! members are ignored). The history must be linear, as mosfet's is: one
//! first revision, every `down_revision` a known revision, no revision
//! followed by two (Alembic's "multiple heads"), no cycle, and no revision
//! named twice.
//!
//! `current` (required) evaluates to the database's current revision, a
//! string, or `null` (Alembic's `base`: no revision applied).
//!
//! `target` (optional, default `head`) evaluates to Alembic's target
//! syntax: `head` (the last revision), `base` (none), a revision, or a
//! relative step `+N` or `-N` from `current`.
//!
//! `location` (required) receives `{from, to, steps}`: `from` is `current`,
//! `to` the target revision (`null` for `base`), and `steps` the ordered
//! steps `{revision, direction, statements}`: upgrading, each revision
//! after `current` up to `to`, oldest first, with its `upgrade` statements
//! (`direction` `upgrade`); downgrading, each revision from `current` down
//! to the one after `to`, newest first, with its `downgrade` statements
//! (`direction` `downgrade`). At the target already, `steps` is empty.
//!
//! # Failures
//!
//! A malformed element (a missing, empty, unknown or repeated attribute, or
//! any content but white space), an expression that fails or evaluates to a
//! value of the wrong kind, a history that is not linear, a `current` or
//! `target` revision the history does not hold, a relative step past
//! `base` or `head`, or a failed assignment is an `execution` error with a
//! static detail (SCXML `error.execution`). A host failure that is not
//! recoverable is passed through unchanged.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries a revision, a statement, or a
//! location, so the cardinality is the product of the three closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::collections::BTreeMap;

use scxml_types::DataValue as Data;
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "migration";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/migration";

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
pub trait MigrationHost {
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
/// [`Failure::Execution`] for every malformed element, value, history, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: MigrationHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "plan" => plan_element(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// The values of an element's unqualified attributes named in `names`, in
/// that order, each trimmed and non-empty; any other attribute is `refused`,
/// a repeated one `repeated`, an empty one `empty`.
fn attribute_values<'a, E, const N: usize>(
    attributes: &'a [Attribute],
    names: [&str; N],
    refused: &'static str,
    repeated: &'static str,
    empty: &'static str,
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

/// Evaluates `expression`, else `failed`.
fn evaluate<H: MigrationHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: MigrationHost>(
    host: &mut H,
    location: &str,
    value: &Data,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    host.assign(location, value)
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
/// exactly (ECMAScript prints it without a fraction).
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

/// Executes `<migration:plan>`.
fn plan_element<H: MigrationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, revisions, current, target] = attribute_values(
        attributes,
        ["location", "revisions", "current", "target"],
        "migration:plan takes only the location, revisions, current and target attributes",
        "migration:plan repeats an attribute",
        "migration:plan has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution("migration:plan requires a location"))?;
    let revisions = revisions.ok_or(Failure::Execution("migration:plan requires revisions"))?;
    let current = current.ok_or(Failure::Execution("migration:plan requires a current"))?;
    no_content(content, "migration:plan holds no content")?;
    let Some(Json::Array(revisions)) = json_of(&evaluate(
        host,
        revisions,
        "migration:plan revisions did not evaluate",
    )?) else {
        return Err(Failure::Execution(
            "migration:plan revisions is not an array",
        ));
    };
    let current = match evaluate(host, current, "migration:plan current did not evaluate")? {
        Data::Null => None,
        Data::String(current) if !current.is_empty() => Some(current),
        _ => {
            return Err(Failure::Execution(
                "migration:plan current is not a revision or null",
            ));
        }
    };
    let target = match target {
        None => "head".to_owned(),
        Some(target) => match evaluate(host, target, "migration:plan target did not evaluate")? {
            Data::String(target) if !target.is_empty() => target,
            _ => {
                return Err(Failure::Execution(
                    "migration:plan target is not a non-empty string",
                ));
            }
        },
    };
    let planned = plan(&revisions, current.as_deref(), &target).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(planned).to_string()),
        "migration:plan could not assign the plan",
    )?;
    Ok(Executed::Handled)
}

/// One revision of a linear history (Alembic's `Script`).
struct Script<'a> {
    revision: &'a str,
    upgrade: &'a Json,
    downgrade: &'a Json,
}

/// The history oldest first, when it is linear (see the crate
/// documentation).
fn history(revisions: &[Json]) -> Result<Vec<Script<'_>>, &'static str> {
    let mut by_down: BTreeMap<Option<&str>, &Map<String, Json>> = BTreeMap::new();
    let mut names = std::collections::BTreeSet::new();
    for revision in revisions {
        let Json::Object(fields) = revision else {
            return Err("migration:plan revision is not an object");
        };
        let name = match fields.get("revision") {
            Some(Json::String(name)) if !name.is_empty() => name.as_str(),
            _ => return Err("migration:plan revision has no revision id"),
        };
        let down = match fields.get("down_revision") {
            None | Some(Json::Null) => None,
            Some(Json::String(down)) if !down.is_empty() => Some(down.as_str()),
            _ => return Err("migration:plan down_revision is not a revision or null"),
        };
        for direction in ["upgrade", "downgrade"] {
            match fields.get(direction) {
                Some(Json::Array(statements)) if statements.iter().all(Json::is_object) => {}
                _ => {
                    return Err(
                        "migration:plan upgrade and downgrade must be arrays of statements",
                    );
                }
            }
        }
        if !names.insert(name) {
            return Err("migration:plan names a revision twice");
        }
        if by_down.insert(down, fields).is_some() {
            return Err("migration:plan history has multiple heads");
        }
    }
    let mut ordered = Vec::with_capacity(revisions.len());
    let mut previous: Option<&str> = None;
    while let Some(fields) = by_down.get(&previous) {
        let revision = fields
            .get("revision")
            .and_then(Json::as_str)
            .unwrap_or_default();
        ordered.push(Script {
            revision,
            upgrade: &fields["upgrade"],
            downgrade: &fields["downgrade"],
        });
        if ordered.len() > revisions.len() {
            return Err("migration:plan history has a cycle");
        }
        previous = Some(revision);
    }
    if ordered.len() != revisions.len() {
        return Err("migration:plan history is not one line from its first revision");
    }
    Ok(ordered)
}

/// The plan `{from, to, steps}` from `current` to `target` (see the crate
/// documentation).
///
/// # Errors
///
/// A static detail for a history that is not linear, or a `current` or
/// `target` it does not hold.
pub fn plan(
    revisions: &[Json],
    current: Option<&str>,
    target: &str,
) -> Result<Map<String, Json>, &'static str> {
    let history = history(revisions)?;
    // Positions: 0 is base, n is the n-th revision.
    let position = |revision: &str| {
        history
            .iter()
            .position(|known| known.revision == revision)
            .map(|index| index + 1)
    };
    let from = match current {
        None => 0,
        Some(current) => {
            position(current).ok_or("migration:plan current is not a revision of the history")?
        }
    };
    let to = match target {
        "head" => history.len(),
        "base" => 0,
        relative if relative.starts_with('+') || relative.starts_with('-') => {
            let steps: usize = relative[1..]
                .parse()
                .map_err(|_| "migration:plan target is not a revision or a relative step")?;
            if relative.starts_with('+') {
                from.checked_add(steps)
                    .filter(|to| *to <= history.len())
                    .ok_or("migration:plan target steps past head")?
            } else {
                from.checked_sub(steps)
                    .ok_or("migration:plan target steps past base")?
            }
        }
        revision => {
            position(revision).ok_or("migration:plan target is not a revision of the history")?
        }
    };
    let step = |revision: &Script<'_>, direction: &str, statements: &Json| {
        serde_json::json!({
            "revision": revision.revision,
            "direction": direction,
            "statements": statements,
        })
    };
    let steps: Vec<Json> = if to >= from {
        history[from..to]
            .iter()
            .map(|revision| step(revision, "upgrade", revision.upgrade))
            .collect()
    } else {
        history[to..from]
            .iter()
            .rev()
            .map(|revision| step(revision, "downgrade", revision.downgrade))
            .collect()
    };
    let name = |at: usize| match at {
        0 => Json::Null,
        at => Json::String(history[at - 1].revision.to_owned()),
    };
    let mut planned = Map::new();
    planned.insert("from".into(), name(from));
    planned.insert("to".into(), name(to));
    planned.insert("steps".into(), Json::Array(steps));
    Ok(planned)
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

    use super::{Attribute, Content, Data, Executed, Failure, MigrationHost};

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

    impl MigrationHost for Host {
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
