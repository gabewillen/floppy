//! The `decision` extension namespace: `<decision:menu>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Executable content is synchronous and quick, so this namespace holds only
//! the half of a decision that is: reading the menu. Asking a backend to
//! choose is slow I/O and belongs to the `decision` invoked service
//! (`services/decision`), which takes the menu this element writes as its
//! `menu` parameter. Documents bind [`NAMESPACE_URI`] (conventionally with
//! the prefix `decision`):
//!
//! ```xml
//! <onentry><decision:menu location="menu"/></onentry>
//! <invoke type="https://github.com/stateforward/nemobot/services/decision@v0.1.0" ...>
//!   <param name="menu" expr="menu"/>
//!   <param name="state" expr="..."/>
//! </invoke>
//! ```
//!
//! # `<decision:menu location="L"/>`
//!
//! `location` (required) receives the canonical menu: a JSON array of
//! objects with `event` (an event name), `schema` (the transition's resolved
//! `event:schema`), and `description` (the schema's `description`, else its
//! `title`, first line; omitted when it has neither), canonical JSON with
//! members sorted by key, in the session snapshot's walk order. Every other
//! attribute (unqualified or namespaced), any child element, and any
//! non-white-space text is refused.
//!
//! * **The menu.** The transitions the session snapshot lists for the
//!   current configuration (`session-snapshot-host`, `transitions` section,
//!   in its walk order) that carry an `event:schema`: each of their event
//!   descriptors that names an event (`*` is skipped; `e.*` names `e`),
//!   unless a transition listed earlier in the same active atomic state's
//!   chain has a descriptor that matches the same event (that one would be
//!   selected for it); each event is offered once. The walk lists a shared
//!   ancestor's transitions once, in the first chain that reaches it, and a
//!   leaf without eventful transitions of its own opens no chain, so in
//!   those two cases the menu can leave out an event SCXML would also
//!   select, never offer one it would not. `event:schemaexpr` transitions
//!   are not offered (a snapshot carries the expression, not the schema).
//! * **Guards.** A transition with a `cond` is offered only when its
//!   condition, evaluated in the current data model (with the current
//!   `_event`, not the event being offered), is exactly `true`; one whose
//!   condition is exactly `false` is skipped and takes no event from a later
//!   transition; one whose condition fails or is not a boolean is not offered
//!   but still takes its events from later transitions. So an event on the
//!   menu is never one only a disabled transition would take, but a guard
//!   that reads `_event` of the offered event is judged against the current
//!   event (author such guards without `event:schema`, or expect them not
//!   to be offered). Conditions run once per element, so they must be free
//!   of side effects, as SCXML conditions should be anyway.
//!
//! The menu's schemas are the contracts of the events the decision service
//! may emit: it validates the chosen event's data against them before it
//! emits it (the service package cannot declare document events).
//!
//! # Failures
//!
//! A malformed element, a missing or malformed snapshot, a schema that does
//! not compile or match its digest, or a failed assignment is an `execution`
//! error with a static detail (SCXML `error.execution`); nothing is assigned
//! then.
//!
//! # Bounds
//!
//! Event names are bounded by [`MAX_EVENT_NAME_BYTES`] on wasm builds only
//! ([`scxml_types::wasm_bound`]); schemas are bounded by
//! `scxml-event-schema`, and the snapshot by the host.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries an event name, a schema, or a
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

use scxml_event_schema::{CompiledSchema, canonical_json, sha256_hex};
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "decision";

/// The namespace's repository namespace URI without its `@<tag>` suffix.
pub const NAMESPACE_URI_BASE: &str = "https://github.com/stateforward/nemobot/namespaces/decision";

/// The release tag the namespace URI names (the placeholder of nemobot's
/// first release, as for every nemobot namespace).
pub const NAMESPACE_TAG: &str = "v0.1.0";

/// The exact namespace URI documents bind: `{NAMESPACE_URI_BASE}@{NAMESPACE_TAG}`.
pub const NAMESPACE_URI: &str =
    "https://github.com/stateforward/nemobot/namespaces/decision@v0.1.0";

/// Upper bound on a menu event name's bytes (wasm builds only).
pub const MAX_EVENT_NAME_BYTES: usize = wasm_bound(256);

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

/// The snapshot read for the menu: only its transitions are used.
pub const MENU_SNAPSHOT_EXCLUDE: [Section; 4] = [
    Section::System,
    Section::DataModel,
    Section::Queues,
    Section::PendingSends,
];

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

/// What a guard evaluated to: `Some(b)` for a boolean, `None` for a failed
/// or non-boolean evaluation.
pub type GuardValue = Option<bool>;

/// The host capabilities one `execute` call uses.
pub trait MenuHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one transition's `cond` in the current data model.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn guard(&mut self, cond: &str) -> Result<GuardValue, Self::Error>;

    /// Assigns the menu (a JSON array) to one data-model location;
    /// `Ok(Err(()))` is a recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, Self::Error>;

    /// The executing session's `scxml.session-snapshot/v2` document with
    /// `exclude` omitted; `Ok(Err(()))` when the host offers none.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, Self::Error>;
}

/// One event on the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuEntry {
    /// The event name.
    pub event: String,
    /// The transition's resolved schema.
    pub schema: Json,
    /// What the event means, when its schema says.
    pub description: Option<String>,
}

/// The canonical menu document of `entries`: a JSON array of
/// `{"description"?, "event", "schema"}` objects in order, canonical JSON.
#[must_use]
pub fn menu_json(entries: &[MenuEntry]) -> String {
    canonical_json(&Json::Array(
        entries
            .iter()
            .map(|entry| {
                let mut object = Map::new();
                object.insert("event".into(), entry.event.clone().into());
                object.insert("schema".into(), entry.schema.clone());
                if let Some(description) = &entry.description {
                    object.insert("description".into(), description.clone().into());
                }
                Json::Object(object)
            })
            .collect(),
    ))
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, snapshot, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: MenuHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    if local_name != "menu" {
        return Ok(Executed::NotHandled);
    }
    let mut location = None;
    for attribute in attributes {
        if attribute.namespace_uri.is_some() || attribute.local_name != "location" {
            return Err(Failure::Execution(
                "decision:menu takes only the location attribute",
            ));
        }
        if location.replace(attribute.value.trim()).is_some() {
            return Err(Failure::Execution("decision:menu repeats an attribute"));
        }
    }
    let location = location
        .filter(|location| !location.is_empty())
        .ok_or(Failure::Execution(
            "decision:menu requires a non-empty location",
        ))?;
    if content
        .iter()
        .any(|item| !matches!(item, Content::Text(text) if text.trim().is_empty()))
    {
        return Err(Failure::Execution("decision:menu holds no content"));
    }
    let document = host
        .snapshot(&MENU_SNAPSHOT_EXCLUDE)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("the host offers no session snapshot"))?;
    let menu = snapshot_menu(&document, &mut |cond| host.guard(cond))?;
    host.assign(location, &Data::JsonArray(menu_json(&menu)))
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("decision:menu could not assign the menu to location"))?;
    Ok(Executed::Handled)
}

/// The first line of the schema's `description`, else of its `title`.
fn description_of(schema: &Json) -> Option<String> {
    ["description", "title"].iter().find_map(|key| {
        schema
            .get(key)
            .and_then(Json::as_str)
            .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
            .map(str::to_owned)
    })
}

/// Whether one event descriptor matches `event` (SCXML 3.12.1: a token
/// prefix match; `*` matches every event; a trailing `.*` is ignored).
fn matches(descriptor: &str, event: &str) -> bool {
    if descriptor == "*" {
        return true;
    }
    let descriptor = descriptor.strip_suffix(".*").unwrap_or(descriptor);
    event == descriptor
        || event
            .strip_prefix(descriptor)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Whether `name` is an event name a candidate may carry: non-empty, no
/// white space, no `*`, no empty `.`-separated token, bounded.
fn valid_event_name(name: &str) -> bool {
    !name.is_empty()
        && !exceeds_wasm_bound(name.len(), MAX_EVENT_NAME_BYTES)
        && !name.contains('*')
        && !name.chars().any(scxml_types::is_xml_space)
        && name.split('.').all(|token| !token.is_empty())
}

/// What one transition's guard evaluated to (see [`snapshot_menu`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Guard {
    /// No `cond`, or a `cond` that evaluated to `true`.
    Enabled,
    /// A `cond` that evaluated to `false`.
    Disabled,
    /// A `cond` whose evaluation failed or was not a boolean.
    Unknown,
}

/// The menu of a `scxml.session-snapshot/v2` document: see the crate
/// documentation ("The menu"). `guard` evaluates one transition's `cond`
/// text: `Ok(Some(b))` for a boolean, `Ok(None)` for a failed or
/// non-boolean evaluation.
///
/// # Errors
///
/// [`Failure::Execution`] with a static detail when the document is
/// malformed or a schema does not compile or match its digest;
/// [`Failure::Host`] when `guard` fails non-recoverably.
pub fn snapshot_menu<E>(
    document: &str,
    guard: &mut dyn FnMut(&str) -> Result<Option<bool>, E>,
) -> Result<Vec<MenuEntry>, Failure<E>> {
    let malformed = || Failure::Execution("the session snapshot is malformed");
    let document: Json = serde_json::from_str(document).map_err(|_| malformed())?;
    let Some(transitions) = document.pointer("/snapshot/root/transitions") else {
        return Ok(Vec::new());
    };
    let transitions = transitions.as_array().ok_or_else(malformed)?;
    let atomic: Vec<&str> = document
        .pointer("/snapshot/root/configuration/atomic")
        .and_then(Json::as_array)
        .ok_or_else(malformed)?
        .iter()
        .map(|state| state.as_str().ok_or_else(malformed))
        .collect::<Result<_, _>>()?;
    // SCXML 3.13 selects per active atomic state: the first enabled
    // matching transition of that state's chain (the state, then its
    // ancestors). The walk lists each leaf's chain in turn, so a transition
    // only preempts later ones of the same chain: `earlier` restarts where
    // a listed transition's source is another active atomic state.
    let mut leaf: Option<&str> = None;
    let mut earlier: Vec<&str> = Vec::new();
    let mut menu: Vec<MenuEntry> = Vec::new();
    for transition in transitions {
        let source = transition
            .get("source")
            .and_then(Json::as_str)
            .ok_or_else(malformed)?;
        if leaf != Some(source) && atomic.contains(&source) {
            leaf = Some(source);
            earlier.clear();
        }
        let descriptors: Vec<&str> = transition
            .get("events")
            .and_then(Json::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|descriptor| descriptor.as_str().ok_or_else(malformed))
            .collect::<Result<_, _>>()?;
        let state = match (
            transition.get("cond"),
            transition.get("has_cond").and_then(Json::as_bool),
        ) {
            (None, Some(false) | None) => Guard::Enabled,
            (Some(Json::String(cond)), Some(true) | None) => {
                match guard(cond).map_err(Failure::Host)? {
                    Some(true) => Guard::Enabled,
                    Some(false) => Guard::Disabled,
                    None => Guard::Unknown,
                }
            }
            _ => return Err(malformed()),
        };
        if state == Guard::Disabled {
            continue;
        }
        if state == Guard::Enabled
            && let Some(event_schema) = transition.get("event_schema")
            && event_schema.get("kind").and_then(Json::as_str) == Some("schema")
        {
            let text = event_schema
                .get("schema")
                .and_then(Json::as_str)
                .ok_or_else(malformed)?;
            if event_schema.get("sha256").and_then(Json::as_str)
                != Some(sha256_hex(text.as_bytes()).as_str())
            {
                return Err(malformed());
            }
            let schema: Json = serde_json::from_str(text).map_err(|_| malformed())?;
            // Fail closed on a schema the decision service could not
            // compile, before it is offered.
            CompiledSchema::new(text)
                .map_err(|_| Failure::Execution("a transition's event schema does not compile"))?;
            for descriptor in &descriptors {
                if *descriptor == "*" {
                    continue;
                }
                let event = descriptor.strip_suffix(".*").unwrap_or(descriptor);
                if !valid_event_name(event)
                    || earlier.iter().any(|before| matches(before, event))
                    || menu.iter().any(|entry| entry.event == event)
                {
                    continue;
                }
                menu.push(MenuEntry {
                    event: event.to_owned(),
                    description: description_of(&schema),
                    schema: schema.clone(),
                });
            }
        }
        earlier.extend(descriptors);
    }
    Ok(menu)
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
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, ExecutableContentExtension,
        ExtensionContent, ExtensionOutcome, SlotReference, TraceContext,
    };
    use stateforward::scxml::{data_model_host, session_snapshot_host};

    use super::{Attribute, Content, Data, Executed, Failure, GuardValue, MenuHost, Section};

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

    impl MenuHost for Host {
        type Error = ComponentError;

        fn guard(&mut self, cond: &str) -> Result<GuardValue, ComponentError> {
            match data_model_host::evaluate(cond) {
                Ok(DataValue::Boolean(value)) => Ok(Some(value)),
                Ok(_) => Ok(None),
                Err(error) if recoverable(error.code) => Ok(None),
                Err(error) => Err(error),
            }
        }

        fn assign(
            &mut self,
            location: &str,
            value: &Data,
        ) -> Result<Result<(), ()>, ComponentError> {
            let Data::JsonArray(text) = value else {
                return Ok(Err(()));
            };
            match data_model_host::assign(location, &DataValue::JsonArray(text.clone())) {
                Ok(()) => Ok(Ok(())),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, ComponentError> {
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
            match session_snapshot_host::snapshot(&exclude) {
                Ok(document) => Ok(Ok(document)),
                // No image for this call, the call bound, or a document
                // bound: the element cannot read the menu (`error.execution`).
                Err(error)
                    if recoverable(error.code) || error.code == ErrorCode::CapacityExhausted =>
                {
                    Ok(Err(()))
                }
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
            _references: Vec<SlotReference>,
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
