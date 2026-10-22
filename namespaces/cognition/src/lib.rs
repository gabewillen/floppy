//! The `cognition` extension namespace: `<cognition:attention_bias_for_turn>`
//! and `<cognition:model_facing_content>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The original's cognition module owns the turn's attention policy
//! (`abilities/cognition/types.py` `attention_bias_for_turn`, with the
//! body's `_interrupt_reference` and `_target_device_reference`) and the
//! projection of a turn for a model (`processing.py`
//! `InputData.model_facing_content` over `abilities/cognition/event.py`
//! `model_facing_content`, fitted as a provider fits it to its
//! `ContextFit`). Both are synchronous and quick, so they are executable content
//! here, under the original's names. Documents bind [`NAMESPACE_URI`]
//! (conventionally with the prefix `cognition`):
//!
//! ```xml
//! <cognition:attention_bias_for_turn location="focus_candidates" focuslocation="focus"
//!                                    stimulus="stimulus" devices="devices" focus="focused_device"/>
//! <cognition:model_facing_content location="prompt" failurelocation="prompt_failure"
//!     systemlocation="system" fitlocation="fit" stimulus="stimulus" self="self" recent="recent"
//!     environment="environment" acted="acted" prior_episodes="prior_episodes"
//!     standing_directives="standing_directives" feedback="feedback" instructions="instructions"
//!     schema="schema" budget="budget"/>
//! ```
//!
//! Every attribute but `location`, `focuslocation`, `failurelocation`,
//! `systemlocation` and `fitlocation` is an expression of the document's data model. Each element writes to its
//! locations and never sends.
//!
//! # `<cognition:attention_bias_for_turn location="L" focuslocation="F" stimulus="X" devices="D" focus="G"/>`
//!
//! The turn's attention bias from device identity only (the original's
//! `attention_bias_for_turn`). `focus` is the currently focused device (any
//! value; `null` when none), `devices` the configured devices (an array),
//! and `stimulus` the turn's stimulus envelope (an object, `{event, data,
//! source, target?, id?}`).
//!
//! * The focus: `focus` when it is not `null` and is one of `devices`, else
//!   `null`. It is the first focus candidate when it is not `null`.
//! * The stimulus's device: for a `bot.input` stimulus, its data's
//!   `target_device` when the data is an object that has one (the
//!   original's `_target_device_reference`, which its `_interrupt_reference`
//!   prefers: the environment stamps a device's input with it, decision
//!   D105), else `null`. It is appended to the candidates when it is not
//!   `null`, is one of `devices`, and is not a candidate already.
//!
//! `focuslocation` (optional) receives the focus, then `location`
//! (required) receives the candidates, a JSON array. Values compare as JSON
//! values (ECMAScript strict equality on the strings device ids are).
//!
//! # `<cognition:model_facing_content location="L" stimulus="S" …/>`
//!
//! The original's `InputData.model_facing_content` over floppy's JSON:
//! `location` (required) receives the user content a model reads for a
//! turn, its blocks joined by newlines, most stable first (R-20 C5), each
//! left out when absent:
//!
//! 1. the stable part of what the bot has in mind (the original's
//!    `RecalledData` with only its `__model_facing_stable_fields__`):
//!    `<cognition:recalled>` holding `recent` (optional, an array of
//!    short-term memory's turns, oldest first), uncounted (its
//!    `__model_facing_uncounted_fields__`), each turn one `<memory:turn>`
//!    line (`stm.Turn.__model_facing_element__`: `at` as `%H:%M:%SZ`,
//!    `direction`, `who` the speakers' `ref`s when it has any, `cut="true"`
//!    when its words were cut, holding its `text`);
//! 2. the turn's situation: the environment block (decision D106) of
//!    `environment` (optional; an object, anything else renders nothing),
//!    the original's `Environment.model_snapshot` layout (`<environment
//!    id>` holding `<self state>` with the bot's own attributes as children
//!    and `<owned_devices>` with one `<device ref id state>` per held
//!    device, its attributes as children), then what earlier stages already
//!    did on the turn (`acted`, optional: the selections they dispatched,
//!    an array, or the original's `ActedData` `{percepts, acts}`) as the
//!    original's `memory_content` of its `ActedData`: `<cognition:acted>`
//!    with the `percepts` (`[0]` for an array: floppy's turn has one) and
//!    the `acts` as `cognition:event`s, or `<cognition:acted
//!    omitted="size" />` over its 16384 bytes;
//! 3. the rest of what the bot has in mind: `<cognition:recalled>` holding
//!    `prior_episodes` (an array of the original's `CognitiveEpisode`s:
//!    `focus`, `focus_candidates`, `stimulus_name`, `output` as
//!    `cognition:event`s and `behavior` as a `behavior:create`,
//!    `behavior:change` or `behavior:break`) and `standing_directives` (an
//!    array of the original's `Directive`s: `text`, `kind`, `instruction`
//!    by default), each optional and with its `count`: floppy's recalled
//!    memory until associative memory exists (decision D97);
//! 4. the stimulus (`stimulus`, required) as the root element of its own
//!    block, tagged as the original's `_payload_tag` tags a payload that is
//!    not a model (`dict`, `list`, `str`, `int`, `float`, `bool`, or
//!    `none_type` for no data or a `null` stimulus), a string as its text and
//!    anything else as its `content` field, stamped `stimulus:event`, then
//!    `stimulus:id`, `stimulus:source` and `stimulus:target` when each is a
//!    non-empty string; a `bot.ability.conversation.output`'s messages
//!    leave out `provenance`, `source_ids` and `target_ids` (the original's
//!    `conversation.Message.__model_facing_excluded_fields__`);
//! 5. the repair `feedback` (optional, a string; `null` is none).
//!
//! `systemlocation` (optional) receives the system channel: the
//! `instructions` (optional, a string; `null` is none), trimmed, then a
//! blank line and the self block of `self` (optional; the cognition's
//! `self` datum `{name?, traits?, description?}`), the original's
//! `SelfData.__model_repr__`: `<persona>` holding `<name taken_by="you">`
//! for a `name` string or `<no_name_yet/>` for `null` (nothing without
//! one), then, with traits, `<traits given_by="owner">` of `<trait>`s and
//! `<description written_by="you">` (empty without one), every value
//! escaped; `null` when both are empty.
//!
//! A field renders as the original's `_fill_field` renders a value that is
//! not a model: `null` is skipped, a scalar is an attribute (a number as
//! Python's `str` writes it), an object is an element of `<entry key>`
//! elements each holding its `value`, and an array an element with its
//! `count` of `<item>` elements, a scalar item as its text and an object or
//! array item filled as `item` (decision D99); an empty array or object
//! renders nothing unless it is an entry's value or an item. A snapshot
//! attribute renders as a child element named after its key (each character
//! other than a letter, a digit, `_`, `.` or `-` replaced by `_`, and `_`
//! put first when the tag is empty or starts with a digit, `.` or `-`): a
//! string as its text, a boolean or number as its JSON text, an object as
//! nested elements, an array as a list. Object members render in the data
//! model's canonical order (sorted keys). Each block is
//! `ElementTree.indent(space="  ")` then `ElementTree.tostring`: attributes
//! in order, an empty element as `<name />`.
//!
//! The original's budgets hold: 262144 encoded bytes, 4096 nodes, 4096
//! collection items and 65536 characters per scalar for each block, and
//! 65536 characters for the system channel, the situation and the
//! feedback. The system channel and the tool's schema (`schema`, optional,
//! its compact JSON) are reserved first; the situation and the feedback are
//! never trimmed, and the stimulus renders into the bytes they leave of the
//! prompt's 262144.
//!
//! What the bot has in mind never makes the projection fail (the original's
//! `b1654d6`): its stable part, then its rest, each render into the bytes
//! everything else leaves, their items newest (last) first; an item that
//! would exceed any budget is left out, the kept items keep their order, a
//! counted field's `count` is how many were kept, and `omitted="<n>"`
//! follows when any was left out. A part whose frame alone does not fit is
//! left out. Sizes are measured as rendered and indented, so the text always
//! fits.
//!
//! `budget` (optional, a number of bytes; `null` is none) is the fitted size
//! of the whole request (the original's `ContextFit.budget_bytes` less the
//! reserved completion); with it the projection is fitted as a provider
//! fits it: the content renders whole, and when it is over the budget less
//! the system channel and the schema it renders again with what the bot has
//! in mind trimmed to that, and `fitlocation` (optional) receives `fit`,
//! `cut` (the trimmed content fits) or `overflow` (it is still over: the
//! stimulus, situation and feedback are never cut, and the request goes as
//! it is). Without a budget `fitlocation` receives `null`.
//!
//! Over a budget otherwise (the stimulus alone overflows, or the system
//! channel, the situation or the feedback is over the scalar budget),
//! `failurelocation` (optional) receives the original's `ValueError`
//! message (for example `prompt projection collection item budget
//! exceeded`) and `location`, `systemlocation` and `fitlocation` receive
//! `null`; without `failurelocation` it is `error.execution`. Within them,
//! `failurelocation` receives `null`. The locations are assigned in the
//! order `failurelocation`, `systemlocation`, `fitlocation`, `location`.
//!
//! The original also counts the left-out items in its
//! `bot.processing.prompt.recalled.omitted` metric and each fit in
//! `bot.processing.context.fit.count`; this component's telemetry is the
//! closed per-entry-point observation below, so the count is only the
//! `omitted` attribute and the fit is `fitlocation`.
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
//! The projection is bounded by the original's budgets above; the data
//! model bounds the values it hands this component.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one
//! closed signal event. No attribute carries a device, a stimulus, a
//! prompt, or a location, so the cardinality is the product of the three
//! closed enums.

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
pub const NAMESPACE_NAME: &str = "cognition";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/cognition";

/// The projection's encoded byte budget, and the prompt's (the original's
/// `_MAX_PROMPT_PROJECTION_BYTES`, `_MAX_MODEL_PROMPT_BYTES`).
pub const MAX_PROJECTION_BYTES: usize = 262_144;

/// The projection's node budget (the original's
/// `_MAX_PROMPT_PROJECTION_NODES`).
pub const MAX_PROJECTION_NODES: usize = 4_096;

/// The projection's collection item budget (the original's
/// `_MAX_PROMPT_PROJECTION_ITEMS`).
pub const MAX_PROJECTION_ITEMS: usize = 4_096;

/// The characters one scalar may have (the original's
/// `_MAX_PROMPT_PROJECTION_SCALAR_CHARACTERS`).
pub const MAX_SCALAR_CHARACTERS: usize = 65_536;

/// The original's message for each exceeded budget.
const NODE_BUDGET: &str = "prompt projection node budget exceeded";
const BYTE_BUDGET: &str = "prompt projection encoded byte budget exceeded";
const ITEM_BUDGET: &str = "prompt projection collection item budget exceeded";
const SCALAR_BUDGET: &str = "prompt projection scalar length budget exceeded";

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
pub trait CognitionHost {
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
pub fn execute<H: CognitionHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "attention_bias_for_turn" => attention_bias_for_turn(attributes, content, host),
        "model_facing_content" => model_facing_content(attributes, content, host),
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

/// Evaluates `expression`, else `failed`.
fn evaluate<H: CognitionHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: CognitionHost>(
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

/// Executes `<cognition:attention_bias_for_turn>`.
fn attention_bias_for_turn<H: CognitionHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, focus_location, stimulus, devices, focus] = attribute_values(
        attributes,
        ["location", "focuslocation", "stimulus", "devices", "focus"],
        [
            "cognition:attention_bias_for_turn takes only the location, focuslocation, stimulus, devices and focus attributes",
            "cognition:attention_bias_for_turn repeats an attribute",
            "cognition:attention_bias_for_turn has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "cognition:attention_bias_for_turn requires a location",
    ))?;
    let stimulus = stimulus.ok_or(Failure::Execution(
        "cognition:attention_bias_for_turn requires a stimulus",
    ))?;
    let devices = devices.ok_or(Failure::Execution(
        "cognition:attention_bias_for_turn requires devices",
    ))?;
    let focus = focus.ok_or(Failure::Execution(
        "cognition:attention_bias_for_turn requires a focus",
    ))?;
    no_content(
        content,
        "cognition:attention_bias_for_turn holds no content",
    )?;
    let Some(Json::Object(stimulus)) = json_of(&evaluate(
        host,
        stimulus,
        "cognition:attention_bias_for_turn stimulus did not evaluate",
    )?) else {
        return Err(Failure::Execution(
            "cognition:attention_bias_for_turn stimulus is not an object",
        ));
    };
    let Some(Json::Array(devices)) = json_of(&evaluate(
        host,
        devices,
        "cognition:attention_bias_for_turn devices did not evaluate",
    )?) else {
        return Err(Failure::Execution(
            "cognition:attention_bias_for_turn devices is not an array",
        ));
    };
    let focus = json_of(&evaluate(
        host,
        focus,
        "cognition:attention_bias_for_turn focus did not evaluate",
    )?)
    .ok_or(Failure::Execution(
        "cognition:attention_bias_for_turn focus is not a JSON value",
    ))?;
    let (focus, candidates) = attention_bias(&stimulus, &devices, &focus);
    if let Some(focus_location) = focus_location {
        let focus = data_of(&focus).ok_or(Failure::Execution(
            "cognition:attention_bias_for_turn focus is not finite",
        ))?;
        assign_to(
            host,
            focus_location,
            &focus,
            "cognition:attention_bias_for_turn could not assign focuslocation",
        )?;
    }
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(candidates).to_string()),
        "cognition:attention_bias_for_turn could not assign the focus candidates",
    )?;
    Ok(Executed::Handled)
}

/// The original's `attention_bias_for_turn`: the turn's focus and focus
/// candidates (see the crate documentation).
#[must_use]
pub fn attention_bias(
    stimulus: &Map<String, Json>,
    devices: &[Json],
    focus: &Json,
) -> (Json, Vec<Json>) {
    let mut candidates = Vec::new();
    let focus = if !focus.is_null() && devices.contains(focus) {
        candidates.push(focus.clone());
        focus.clone()
    } else {
        Json::Null
    };
    let mut reference = Json::Null;
    if stimulus.get("event").and_then(Json::as_str) == Some("bot.input")
        && let Some(Json::Object(data)) = stimulus.get("data")
    {
        reference = data.get("target_device").cloned().unwrap_or(Json::Null);
    }
    if !reference.is_null() && devices.contains(&reference) && !candidates.contains(&reference) {
        candidates.push(reference);
    }
    (focus, candidates)
}

/// Executes `<cognition:model_facing_content>`.
fn model_facing_content<H: CognitionHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [
        location,
        failure_location,
        system_location,
        fit_location,
        stimulus,
        known,
        recent,
        environment,
        acted,
        prior_episodes,
        standing_directives,
        feedback,
        instructions,
        schema,
        budget,
    ] = attribute_values(
        attributes,
        [
            "location",
            "failurelocation",
            "systemlocation",
            "fitlocation",
            "stimulus",
            "self",
            "recent",
            "environment",
            "acted",
            "prior_episodes",
            "standing_directives",
            "feedback",
            "instructions",
            "schema",
            "budget",
        ],
        [
            "cognition:model_facing_content takes only the location, failurelocation, systemlocation, fitlocation, stimulus, self, recent, environment, acted, prior_episodes, standing_directives, feedback, instructions, schema and budget attributes",
            "cognition:model_facing_content repeats an attribute",
            "cognition:model_facing_content has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "cognition:model_facing_content requires a location",
    ))?;
    let stimulus = stimulus.ok_or(Failure::Execution(
        "cognition:model_facing_content requires a stimulus",
    ))?;
    no_content(content, "cognition:model_facing_content holds no content")?;
    let stimulus = read_stimulus(host, stimulus)?;
    let known = read_json(host, known, "self")?.unwrap_or(Json::Null);
    let recent = read_list(host, recent, "recent")?;
    let prior_episodes = read_list(host, prior_episodes, "prior_episodes")?;
    let standing_directives = read_list(host, standing_directives, "standing_directives")?;
    let environment = read_json(host, environment, "environment")?.unwrap_or(Json::Null);
    let acted = read_json(host, acted, "acted")?.unwrap_or(Json::Null);
    let feedback = read_text(host, feedback, "feedback")?;
    let instructions = read_text(host, instructions, "instructions")?;
    let tool_bytes =
        read_json(host, schema, "schema")?.map_or(0, |schema| schema.to_string().len());
    let budget = read_budget(host, budget)?;
    let input = TurnInput {
        stimulus: stimulus.as_ref(),
        recent: &recent,
        environment: &environment,
        acted: &acted,
        prior_episodes: &prior_episodes,
        standing_directives: &standing_directives,
        feedback: &feedback,
    };
    let system = system(&instructions, &known);
    let projected = project(&input, &system, tool_bytes, budget);
    assign_projection(
        host,
        projected,
        system,
        [
            failure_location,
            system_location,
            fit_location,
            Some(location),
        ],
    )?;
    Ok(Executed::Handled)
}

/// Assigns a projection to `[failurelocation, systemlocation,
/// fitlocation, location]`, each that is given: on success `null`, the
/// system channel (`null` when empty), the fit (`null` without a budget)
/// and the content; on failure the message and `null`s (without
/// `failurelocation`, the failure is `error.execution`).
fn assign_projection<H: CognitionHost>(
    host: &mut H,
    projected: Result<Projected, &'static str>,
    system: String,
    locations: [Option<&str>; 4],
) -> Result<(), Failure<H::Error>> {
    let values = match projected {
        Ok(projected) => [
            Data::Null,
            if system.is_empty() {
                Data::Null
            } else {
                Data::String(system)
            },
            projected
                .fit
                .map_or(Data::Null, |fit| Data::String(fit.as_str().into())),
            Data::String(projected.content),
        ],
        Err(message) if locations[0].is_some() => [
            Data::String(message.into()),
            Data::Null,
            Data::Null,
            Data::Null,
        ],
        Err(message) => return Err(Failure::Execution(message)),
    };
    let failures = [
        "cognition:model_facing_content could not assign failurelocation",
        "cognition:model_facing_content could not assign systemlocation",
        "cognition:model_facing_content could not assign fitlocation",
        "cognition:model_facing_content could not assign the content",
    ];
    for ((location, value), failed) in locations.into_iter().zip(&values).zip(failures) {
        if let Some(location) = location {
            assign_to(host, location, value, failed)?;
        }
    }
    Ok(())
}

/// The stimulus envelope `expression` evaluates to: an object with a
/// string `event`, or `None` for `null`.
fn read_stimulus<H: CognitionHost>(
    host: &mut H,
    expression: &str,
) -> Result<Option<Map<String, Json>>, Failure<H::Error>> {
    let stimulus = match json_of(&evaluate(
        host,
        expression,
        "cognition:model_facing_content stimulus did not evaluate",
    )?) {
        Some(Json::Null) => None,
        Some(Json::Object(stimulus)) => Some(stimulus),
        _ => {
            return Err(Failure::Execution(
                "cognition:model_facing_content stimulus is not an object or null",
            ));
        }
    };
    if let Some(stimulus) = &stimulus
        && !stimulus.get("event").is_some_and(Json::is_string)
    {
        return Err(Failure::Execution(
            "cognition:model_facing_content stimulus has no event",
        ));
    }
    Ok(stimulus)
}

/// An optional input attribute's failure details: it did not evaluate,
/// or it evaluated to a value of the wrong kind.
fn input_failures(name: &str) -> (&'static str, &'static str) {
    match name {
        "self" => (
            "cognition:model_facing_content self did not evaluate",
            "cognition:model_facing_content self is not a JSON value",
        ),
        "recent" => (
            "cognition:model_facing_content recent did not evaluate",
            "cognition:model_facing_content recent is not an array",
        ),
        "prior_episodes" => (
            "cognition:model_facing_content prior_episodes did not evaluate",
            "cognition:model_facing_content prior_episodes is not an array",
        ),
        "standing_directives" => (
            "cognition:model_facing_content standing_directives did not evaluate",
            "cognition:model_facing_content standing_directives is not an array",
        ),
        "environment" => (
            "cognition:model_facing_content environment did not evaluate",
            "cognition:model_facing_content environment is not a JSON value",
        ),
        "acted" => (
            "cognition:model_facing_content acted did not evaluate",
            "cognition:model_facing_content acted is not a JSON value",
        ),
        "feedback" => (
            "cognition:model_facing_content feedback did not evaluate",
            "cognition:model_facing_content feedback is not a string",
        ),
        "instructions" => (
            "cognition:model_facing_content instructions did not evaluate",
            "cognition:model_facing_content instructions is not a string",
        ),
        _ => (
            "cognition:model_facing_content schema did not evaluate",
            "cognition:model_facing_content schema is not a JSON value",
        ),
    }
}

/// The JSON value the optional input attribute `name` evaluates to
/// (`None` without the attribute).
fn read_json<H: CognitionHost>(
    host: &mut H,
    expression: Option<&str>,
    name: &str,
) -> Result<Option<Json>, Failure<H::Error>> {
    let (failed, malformed) = input_failures(name);
    expression
        .map(|expression| {
            json_of(&evaluate(host, expression, failed)?).ok_or(Failure::Execution(malformed))
        })
        .transpose()
}

/// The array the optional list attribute `name` evaluates to; `null` or no
/// attribute is none.
fn read_list<H: CognitionHost>(
    host: &mut H,
    expression: Option<&str>,
    name: &str,
) -> Result<Vec<Json>, Failure<H::Error>> {
    match read_json(host, expression, name)? {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => Ok(items),
        Some(_) => Err(Failure::Execution(input_failures(name).1)),
    }
}

/// The string the optional text attribute `name` evaluates to; `null` or
/// no attribute is empty.
fn read_text<H: CognitionHost>(
    host: &mut H,
    expression: Option<&str>,
    name: &str,
) -> Result<String, Failure<H::Error>> {
    let Some(expression) = expression else {
        return Ok(String::new());
    };
    let (failed, malformed) = input_failures(name);
    match evaluate(host, expression, failed)? {
        Data::String(text) => Ok(text),
        Data::Null => Ok(String::new()),
        _ => Err(Failure::Execution(malformed)),
    }
}

/// The fitted budget an optional `expression` evaluates to: a number of
/// bytes, floored at zero; `null` or no attribute fits nothing.
fn read_budget<H: CognitionHost>(
    host: &mut H,
    expression: Option<&str>,
) -> Result<Option<usize>, Failure<H::Error>> {
    let Some(expression) = expression else {
        return Ok(None);
    };
    match evaluate(
        host,
        expression,
        "cognition:model_facing_content budget did not evaluate",
    )? {
        Data::Null => Ok(None),
        Data::Number(bytes) => {
            let bytes = bytes.get().floor();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a finite byte count, floored at zero and saturated at usize::MAX"
            )]
            Ok(Some(if bytes <= 0.0 { 0 } else { bytes as usize }))
        }
        _ => Err(Failure::Execution(
            "cognition:model_facing_content budget is not a number",
        )),
    }
}

/// What `<cognition:model_facing_content>` projects: the original's
/// processing `InputData`, its `situation` (the environment block, then the
/// `acted` fact) and `recalled` (`RecalledData`) as floppy's JSON.
#[derive(Clone, Copy, Debug)]
pub struct TurnInput<'a> {
    /// The stimulus envelope, `None` for none.
    pub stimulus: Option<&'a Map<String, Json>>,
    /// The recent turns short-term memory holds, oldest first (the
    /// original's `RecalledData.recent`, `stm.Turn`s).
    pub recent: &'a [Json],
    /// The bot's environment, `null` for none.
    pub environment: &'a Json,
    /// What earlier stages already did on the turn: the selections they
    /// dispatched (an array), or the original's `ActedData` (`{percepts,
    /// acts}`); `null` or an empty array for nothing.
    pub acted: &'a Json,
    /// The prior cognitive episodes (floppy's recalled memory until
    /// associative memory exists, decision D97).
    pub prior_episodes: &'a [Json],
    /// The standing directives (decision D97, as `prior_episodes`).
    pub standing_directives: &'a [Json],
    /// The repair feedback, empty for none.
    pub feedback: &'a str,
}

/// How a projection was fitted to a fitted budget (the original's
/// `ContextFitOutcome`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fit {
    /// Sent whole.
    Fit,
    /// Recalled memory was cut or left out to fit.
    Cut,
    /// Still over at its smallest, sent anyway.
    Overflow,
}

impl Fit {
    /// The original's outcome name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fit => "fit",
            Self::Cut => "cut",
            Self::Overflow => "overflow",
        }
    }
}

/// A turn's projected user content and, with a fitted budget, how it fit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Projected {
    /// The user content.
    pub content: String,
    /// How it fit the fitted budget; `None` without one.
    pub fit: Option<Fit>,
}

/// The system channel: the static `instructions` then the self block of
/// `known` (the original's `SelfData.__model_repr__`), joined by a blank
/// line as `Processing._input_for_processor` stamps them; empty when both
/// are.
#[must_use]
pub fn system(instructions: &str, known: &Json) -> String {
    let instructions = instructions.trim();
    let block = self_block(known);
    match (instructions.is_empty(), block.is_empty()) {
        (true, _) => block,
        (false, true) => instructions.to_owned(),
        (false, false) => format!("{instructions}\n\n{block}"),
    }
}

/// The original's `SelfData.__model_repr__` of the cognition's `self`
/// datum (`{name?, traits?, description?}`, what identity and persona told
/// it): `<persona>` holding the name (`identity.NameData`'s `<name
/// taken_by="you">` for a string, `<no_name_yet/>` for `null`, nothing when
/// absent) and, with traits (the persona's `OutputData` has at least one),
/// the owner's `<traits given_by="owner">`, then the self-written
/// `<description written_by="you">` (empty without one); empty when `known`
/// is not an object.
fn self_block(known: &Json) -> String {
    let Some(known) = known.as_object() else {
        return String::new();
    };
    let mut block = String::from("<persona>");
    match known.get("name") {
        Some(Json::String(name)) => {
            block.push_str("<name taken_by=\"you\">");
            block.push_str(&xml_escape(name));
            block.push_str("</name>");
        }
        Some(Json::Null) => block.push_str("<no_name_yet/>"),
        _ => {}
    }
    let traits: Vec<&str> = known
        .get("traits")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    if !traits.is_empty() {
        block.push_str("<traits given_by=\"owner\">");
        for trait_text in traits {
            block.push_str("<trait>");
            block.push_str(&xml_escape(trait_text));
            block.push_str("</trait>");
        }
        block.push_str("</traits>");
        match known.get("description").and_then(Json::as_str) {
            Some(text) => {
                block.push_str("<description written_by=\"you\">");
                block.push_str(&xml_escape(text));
                block.push_str("</description>");
            }
            None => block.push_str("<description written_by=\"you\"/>"),
        }
    }
    block.push_str("</persona>");
    block
}

/// `xml.sax.saxutils.escape`: `&`, `<` and `>`.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The original's `InputData.model_facing_content` fitted as a provider
/// fits it to a `ContextFit` budget: the user content of `input`, with the
/// system channel `system` and a tool schema of `tool_bytes` bytes reserved
/// first (see the crate documentation).
///
/// # Errors
///
/// The original's message for the first budget exceeded: the system
/// channel, the situation or the feedback over the scalar budget, or the
/// stimulus over the bytes they and the tool leave (or any other budget).
/// Recalled memory never fails the projection.
pub fn project(
    input: &TurnInput<'_>,
    system: &str,
    tool_bytes: usize,
    budget: Option<usize>,
) -> Result<Projected, &'static str> {
    let whole = content(input, system, tool_bytes, None)?;
    let Some(budget) = budget else {
        return Ok(Projected {
            content: whole,
            fit: None,
        });
    };
    let budget = budget
        .saturating_sub(system.len())
        .saturating_sub(tool_bytes);
    if whole.len() <= budget {
        return Ok(Projected {
            content: whole,
            fit: Some(Fit::Fit),
        });
    }
    // Over: render again within the budget, which trims or leaves out only
    // the recalled memory.
    let content = content(input, system, tool_bytes, Some(budget))?;
    let fit = if content.len() <= budget {
        Fit::Cut
    } else {
        Fit::Overflow
    };
    Ok(Projected {
        content,
        fit: Some(fit),
    })
}

/// The original's `InputData.model_facing_content` with its
/// `budget_bytes`: the stable recalled part, the situation, the varying
/// recalled part, the stimulus, the feedback, joined by newlines, each
/// left out when absent.
fn content(
    input: &TurnInput<'_>,
    system: &str,
    tool_bytes: usize,
    budget_bytes: Option<usize>,
) -> Result<String, &'static str> {
    let situation = situation(input.environment, input.acted)?;
    let around: Vec<&str> = [situation.as_str(), input.feedback]
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect();
    if [system]
        .iter()
        .chain(&around)
        .any(|block| block.chars().count() > MAX_SCALAR_CHARACTERS)
    {
        return Err(SCALAR_BUDGET);
    }
    let reserved = system.len().saturating_add(tool_bytes);
    // The situation and the feedback each join the stimulus with a newline.
    let beside: usize = around.iter().map(|block| block.len() + 1).sum();
    let limit = MAX_PROJECTION_BYTES
        .checked_sub(reserved)
        .and_then(|limit| limit.checked_sub(beside))
        .ok_or(BYTE_BUDGET)?;
    let mut stimulus_budget = Budget::default();
    let mut stimulus = stimulus_element(input.stimulus, &mut stimulus_budget)?.text(0);
    if stimulus.len() > limit {
        return Err(BYTE_BUDGET);
    }
    let mut stable = None;
    let mut varying = None;
    if !(input.recent.is_empty()
        && input.prior_episodes.is_empty()
        && input.standing_directives.is_empty())
    {
        let available = budget_bytes.map_or(MAX_PROJECTION_BYTES - reserved, |budget| {
            budget.min(MAX_PROJECTION_BYTES - reserved)
        });
        let mut remaining = available.checked_sub(stimulus.len() + beside);
        // The stable fields render before the situation and claim the
        // budget first; the rest after it.
        for (rendered, fields) in [
            (
                &mut stable,
                vec![Field {
                    name: "recent",
                    items: input.recent,
                    render: turn,
                    counted: false,
                }],
            ),
            (
                &mut varying,
                vec![
                    Field {
                        name: "prior_episodes",
                        items: input.prior_episodes,
                        render: episode,
                        counted: true,
                    },
                    Field {
                        name: "standing_directives",
                        items: input.standing_directives,
                        render: directive,
                        counted: true,
                    },
                ],
            ),
        ] {
            if fields.iter().all(|field| field.items.is_empty()) {
                continue;
            }
            // A newline joins each recalled element to what follows it.
            remaining = remaining.and_then(|remaining| remaining.checked_sub(1));
            if let Some(limit) = remaining
                && let Some(text) = recalled(&fields, limit)
            {
                remaining = Some(limit - text.len());
                *rendered = Some(text);
            }
        }
    }
    let before: Vec<&str> = [
        stable.as_deref(),
        Some(situation.as_str()),
        varying.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|block| !block.is_empty())
    .collect();
    if !before.is_empty() {
        stimulus = format!("{}\n{stimulus}", before.join("\n"));
    }
    if !input.feedback.is_empty() {
        stimulus.push('\n');
        stimulus.push_str(input.feedback);
    }
    Ok(stimulus)
}

/// The turn's situation: the environment block, then what earlier stages
/// already did on it (the original's `_build_processing_input`: the world
/// block and the `acted` fact joined by a newline); empty for neither.
fn situation(environment: &Json, acted: &Json) -> Result<String, &'static str> {
    let mut budget = Budget::default();
    let world = environment_element(environment, &mut budget)?.map(|element| element.text(0));
    let fact = acted_fact(acted);
    Ok([world, fact]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n"))
}

/// What the original keeps of a turn's `ActedData` in its situation
/// (`memory_content` within `_MAX_ACTED_BYTES`): `<cognition:acted>` with
/// the `percepts` acted on and the `acts` dispatched, or the bare element
/// with `omitted="size"` when it does not fit; `None` when nothing was
/// done.
fn acted_fact(acted: &Json) -> Option<String> {
    let (percepts, acts) = match acted {
        Json::Array(acts) => (vec![Json::from(0)], acts.as_slice()),
        Json::Object(acted) => (
            acted
                .get("percepts")
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_else(|| vec![Json::from(0)]),
            acted
                .get("acts")
                .and_then(Json::as_array)
                .map_or(&[][..], Vec::as_slice),
        ),
        _ => return None,
    };
    if acts.is_empty() {
        return None;
    }
    let rendered = (|| {
        let mut budget = Budget::default();
        let mut element = budget.element("cognition:acted")?;
        fill_field(
            &mut element,
            "percepts",
            Some(&Json::Array(percepts)),
            &mut budget,
        )?;
        element
            .children
            .push(models("acts", acts, selection, &mut budget)?);
        let text = element.text(0);
        if text.len() > MAX_ACTED_BYTES {
            return Err(BYTE_BUDGET);
        }
        Ok::<_, &'static str>(text)
    })();
    Some(rendered.unwrap_or_else(|_| "<cognition:acted omitted=\"size\" />".to_owned()))
}

/// The bytes the acted fact may take (the original's `_MAX_ACTED_BYTES`:
/// a sixteenth of an episode's 262144).
const MAX_ACTED_BYTES: usize = 16_384;

/// One budgeted field of a recalled part: its items render after the rest
/// of the part, newest (last) first.
#[derive(Clone, Copy)]
struct Field<'a> {
    name: &'static str,
    items: &'a [Json],
    render: Render,
    counted: bool,
}

/// One part of the original's `RecalledData` rendered within `limit`
/// bytes: `<cognition:recalled>` holding each field with items, the rest at
/// their defaults rendering nothing; `None` when its frame does not fit.
fn recalled(fields: &[Field<'_>], limit: usize) -> Option<String> {
    let mut budget = Budget::default();
    let mut root = budget.element("cognition:recalled").ok()?;
    let mut deferred = Vec::new();
    for field in fields.iter().filter(|field| !field.items.is_empty()) {
        let mut list = budget.element(field.name).ok()?;
        // Upper bounds for the final count and omission, settled once the
        // items render.
        if field.counted {
            budget
                .attribute(&mut list, "count", field.items.len().to_string())
                .ok()?;
        }
        budget
            .attribute(&mut list, "omitted", field.items.len().to_string())
            .ok()?;
        deferred.push((root.children.len(), *field));
        root.children.push(list);
    }
    render_budgeted(&mut root, &deferred, limit, &mut budget).ok()?;
    let text = root.text(0);
    (text.len() <= limit).then_some(text)
}

/// The original's `_render_budgeted`: each deferred field's items, newest
/// first, into the bytes the rest of `root` leaves of `limit`. An item that
/// would exceed any budget is left out, and its field's `omitted` counts
/// them (the attribute is dropped when none is); kept items keep their
/// order. Sizes are measured as rendered and indented, as the original's.
///
/// # Errors
///
/// [`BYTE_BUDGET`] when `root` without any recalled item is already over
/// `limit`: the frame does not fit.
fn render_budgeted(
    root: &mut Element,
    deferred: &[(usize, Field<'_>)],
    limit: usize,
    budget: &mut Budget,
) -> Result<(), &'static str> {
    if deferred.is_empty() {
        return Ok(());
    }
    let mut used = root.text(0).len();
    if used > limit {
        return Err(BYTE_BUDGET);
    }
    // Every budgeted field is a child of the root, so its items are at
    // level 2.
    let level = 2;
    for (index, recalled) in deferred {
        let Some(field) = root.children.get_mut(*index) else {
            continue;
        };
        // Each item rides on its own indented line; the first one also turns
        // the empty `<field … />` into `<field …>` + indentation +
        // `</field>` (the original's arithmetic, byte for byte).
        let line = 1 + INDENT.len() * level;
        let opening = format!("</{}>", field.name).len() + line - 2;
        let mut kept: Vec<(usize, Element)> = Vec::new();
        for (position, item) in recalled.items.iter().enumerate().rev() {
            let mark = budget.mark();
            let Ok(element) = budget
                .collection(1)
                .and_then(|()| (recalled.render)(item.as_object(), budget))
            else {
                budget.rewind(mark);
                continue;
            };
            let cost = element.text(level).len() + line + if kept.is_empty() { opening } else { 0 };
            if used + cost > limit || budget.bytes > MAX_PROJECTION_BYTES {
                budget.rewind(mark);
                continue;
            }
            used += cost;
            kept.push((position, element));
        }
        kept.reverse();
        let omitted = recalled.items.len() - kept.len();
        field
            .children
            .extend(kept.into_iter().map(|(_, element)| element));
        let count = field.children.len().to_string();
        field
            .attributes
            .retain_mut(|(name, value)| match name.as_str() {
                "count" => {
                    value.clone_from(&count);
                    true
                }
                "omitted" => {
                    *value = omitted.to_string();
                    omitted > 0
                }
                _ => true,
            });
    }
    Ok(())
}

/// The indentation of one level (`ElementTree.indent(space="  ")`).
const INDENT: &str = "  ";

/// The original's `_ProjectionBudget`: encoded bytes, nodes and collection
/// items so far.
#[derive(Clone, Copy, Debug, Default)]
struct Budget {
    bytes: usize,
    nodes: usize,
    items: usize,
}

impl Budget {
    /// The original's `_ProjectionBudget.mark`.
    fn mark(&self) -> Self {
        *self
    }

    /// The original's `_ProjectionBudget.rewind`.
    fn rewind(&mut self, mark: Self) {
        *self = mark;
    }

    fn markup(&mut self, text: &str) -> Result<(), &'static str> {
        self.bytes = self.bytes.saturating_add(text.len());
        if self.bytes > MAX_PROJECTION_BYTES {
            return Err(BYTE_BUDGET);
        }
        Ok(())
    }

    fn collection(&mut self, count: usize) -> Result<(), &'static str> {
        self.items = self.items.saturating_add(count);
        if self.items > MAX_PROJECTION_ITEMS {
            return Err(ITEM_BUDGET);
        }
        Ok(())
    }

    fn scalar(&mut self, text: &str) -> Result<(), &'static str> {
        if text.chars().count() > MAX_SCALAR_CHARACTERS {
            return Err(SCALAR_BUDGET);
        }
        self.markup(&html_escape(text))
    }

    /// The original's `_element`: one element, counted.
    fn element(&mut self, name: &str) -> Result<Element, &'static str> {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > MAX_PROJECTION_NODES {
            return Err(NODE_BUDGET);
        }
        self.markup(name)?;
        Ok(Element {
            name: name.to_owned(),
            attributes: Vec::new(),
            children: Vec::new(),
            text: None,
        })
    }

    /// The original's `_set_attribute`.
    fn attribute(
        &mut self,
        element: &mut Element,
        name: &str,
        value: String,
    ) -> Result<(), &'static str> {
        self.markup(name)?;
        self.scalar(&value)?;
        element.attributes.push((name.to_owned(), value));
        Ok(())
    }

    /// One element holding `text`, the scalar counted first.
    fn text_element(&mut self, name: &str, text: String) -> Result<Element, &'static str> {
        self.scalar(&text)?;
        let mut element = self.element(name)?;
        element.text = Some(text);
        Ok(element)
    }
}

/// Python's `html.escape(value, quote=True)`.
fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// One element of the projection.
#[derive(Clone, Debug)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
    text: Option<String>,
}

impl Element {
    /// `ElementTree.indent(space="  ")` then `ElementTree.tostring`.
    fn text(&self, level: usize) -> String {
        let mut out = String::new();
        out.push('<');
        out.push_str(&self.name);
        for (name, value) in &self.attributes {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            for character in value.chars() {
                match character {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\r' => out.push_str("&#13;"),
                    '\n' => out.push_str("&#10;"),
                    '\t' => out.push_str("&#09;"),
                    other => out.push(other),
                }
            }
            out.push('"');
        }
        if self.children.is_empty() {
            let text = self.text.as_deref().unwrap_or_default();
            if text.is_empty() {
                out.push_str(" />");
            } else {
                out.push('>');
                for character in text.chars() {
                    match character {
                        '&' => out.push_str("&amp;"),
                        '<' => out.push_str("&lt;"),
                        '>' => out.push_str("&gt;"),
                        other => out.push(other),
                    }
                }
                out.push_str("</");
                out.push_str(&self.name);
                out.push('>');
            }
            return out;
        }
        let inner = format!("\n{}", "  ".repeat(level + 1));
        out.push('>');
        for child in &self.children {
            out.push_str(&inner);
            out.push_str(&child.text(level + 1));
        }
        out.push('\n');
        out.push_str(&"  ".repeat(level));
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
        out
    }
}

/// A number as Python's `str` writes it: a whole number as an integer,
/// else the shortest repr, in exponent form below 1e-4 or from 1e16.
fn python_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return whole(value);
    }
    let magnitude = value.abs();
    if magnitude != 0.0 && !(1e-4..1e16).contains(&magnitude) {
        let (mantissa, exponent) = exponent_parts(value);
        let (sign, digits) = match exponent.strip_prefix('-') {
            Some(digits) => ('-', digits),
            None => ('+', exponent.as_str()),
        };
        let padding = if digits.len() < 2 { "0" } else { "" };
        return format!("{mantissa}e{sign}{padding}{digits}");
    }
    value.to_string()
}

/// A number as ECMAScript's `JSON.stringify` writes it.
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

/// A scalar's text: a boolean as `true`/`false`, a number as Python writes
/// it, a string as itself; `None` for anything else.
fn scalar_text(value: &Json) -> Option<String> {
    match value {
        Json::Bool(value) => Some(if *value { "true" } else { "false" }.to_owned()),
        Json::Number(value) => value.as_f64().map(python_number),
        Json::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// ECMAScript's `String(value)` of a member that may be missing.
fn ecmascript_string(value: Option<&Json>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Json::Null) => "null".to_owned(),
        Some(Json::Bool(value)) => if *value { "true" } else { "false" }.to_owned(),
        Some(Json::Number(value)) => value.as_f64().map(ecmascript_number).unwrap_or_default(),
        Some(Json::String(text)) => text.clone(),
        Some(Json::Array(items)) => items
            .iter()
            .map(|item| match item {
                Json::Null => String::new(),
                item => ecmascript_string(Some(item)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Json::Object(_)) => "[object Object]".to_owned(),
    }
}

/// The members of a value read as an object (none for anything else).
fn members(value: Option<&Json>) -> Option<&Map<String, Json>> {
    value.and_then(Json::as_object)
}

/// The original's `_fill_field` over a JSON value (see the crate
/// documentation): an empty array or object renders nothing, as it tells
/// the model nothing its absence does not.
fn fill_field(
    element: &mut Element,
    name: &str,
    value: Option<&Json>,
    budget: &mut Budget,
) -> Result<(), &'static str> {
    match value {
        Some(Json::Array(items)) if items.is_empty() => Ok(()),
        Some(Json::Object(object)) if object.is_empty() => Ok(()),
        value => fill_item(element, name, value, budget),
    }
}

/// The original's `_fill_field` with `as_item`: a value placed as an entry
/// of an enclosing collection, which renders even when it is an empty
/// array or object, keeping that collection's count true.
fn fill_item(
    element: &mut Element,
    name: &str,
    value: Option<&Json>,
    budget: &mut Budget,
) -> Result<(), &'static str> {
    let Some(value) = value else {
        return Ok(());
    };
    match value {
        Json::Null => Ok(()),
        Json::Bool(_) | Json::Number(_) | Json::String(_) => {
            let text = scalar_text(value).unwrap_or_default();
            budget.attribute(element, name, text)
        }
        Json::Array(items) => {
            budget.collection(items.len())?;
            let mut list = budget.element(name)?;
            budget.attribute(&mut list, "count", items.len().to_string())?;
            for item in items {
                match scalar_text(item) {
                    Some(text) => list.children.push(budget.text_element("item", text)?),
                    None => fill_item(&mut list, "item", Some(item), budget)?,
                }
            }
            element.children.push(list);
            Ok(())
        }
        Json::Object(object) => {
            let mut mapping = budget.element(name)?;
            budget.collection(object.len())?;
            for (key, value) in object {
                let mut entry = budget.element("entry")?;
                budget.attribute(&mut entry, "key", key.clone())?;
                fill_item(&mut entry, "value", Some(value), budget)?;
                mapping.children.push(entry);
            }
            element.children.push(mapping);
            Ok(())
        }
    }
}

/// The stimulus as the root element of its own block (the original's
/// `_project` of a payload that is not a model): tagged as the original's
/// `_payload_tag` tags the payload's Python type (`dict`, `list`, `str`,
/// `int`, `float`, `bool`, `none_type`), a string as its text and anything else filled as its
/// `content` field, stamped `stimulus:event`, then `stimulus:id`,
/// `:source` and `:target` when set. A conversation output's messages
/// leave out the fields the original's `conversation.Message` excludes from
/// a model (`__model_facing_excluded_fields__`).
fn stimulus_element(
    envelope: Option<&Map<String, Json>>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let data = envelope.and_then(|envelope| envelope.get("data"));
    let shown = match envelope.and_then(|envelope| envelope.get("event")) {
        Some(Json::String(event)) if event == CONVERSATION_OUTPUT => {
            data.map(model_facing_messages)
        }
        _ => data.cloned(),
    };
    let tag = match &shown {
        None | Some(Json::Null) => "none_type",
        Some(Json::Bool(_)) => "bool",
        Some(Json::Number(number)) if number.is_f64() => "float",
        Some(Json::Number(_)) => "int",
        Some(Json::String(_)) => "str",
        Some(Json::Array(_)) => "list",
        Some(Json::Object(_)) => "dict",
    };
    let mut root = match &shown {
        Some(Json::String(text)) => budget.text_element(tag, text.clone())?,
        shown => {
            let mut root = budget.element(tag)?;
            fill_field(&mut root, "content", shown.as_ref(), budget)?;
            root
        }
    };
    let Some(envelope) = envelope else {
        return Ok(root);
    };
    let event = envelope
        .get("event")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    budget.attribute(&mut root, "stimulus:event", event)?;
    for key in ["id", "source", "target"] {
        if let Some(text) = envelope.get(key).and_then(Json::as_str)
            && !text.is_empty()
        {
            budget.attribute(&mut root, &format!("stimulus:{key}"), text.to_owned())?;
        }
    }
    Ok(root)
}

/// The conversation's output event, whose messages a model sees without
/// their correlation.
const CONVERSATION_OUTPUT: &str = "bot.ability.conversation.output";

/// The fields of a conversation message a model never sees (the original's
/// `conversation.Message.__model_facing_excluded_fields__`).
const MESSAGE_EXCLUDED_FIELDS: [&str; 3] = ["provenance", "source_ids", "target_ids"];

/// A conversation output's data with each of its `messages` without
/// [`MESSAGE_EXCLUDED_FIELDS`].
fn model_facing_messages(data: &Json) -> Json {
    let mut data = data.clone();
    if let Some(messages) = data.get_mut("messages").and_then(Json::as_array_mut) {
        for message in messages.iter_mut().filter_map(Json::as_object_mut) {
            for field in MESSAGE_EXCLUDED_FIELDS {
                message.remove(field);
            }
        }
    }
    data
}

/// Renders one model of a tuple.
type Render = fn(Option<&Map<String, Json>>, &mut Budget) -> Result<Element, &'static str>;

/// The original's `stm.Turn.__model_facing_element__`: one `<memory:turn>`
/// line, `at` (UTC, to the second), `direction`, `who` (each speaker's
/// `ref`) when it has speakers and `cut="true"` when the words were cut,
/// holding the words.
fn turn(turn: Option<&Map<String, Json>>, budget: &mut Budget) -> Result<Element, &'static str> {
    let field = |key: &str| turn.and_then(|turn| turn.get(key));
    let mut element = budget.element("memory:turn")?;
    if let Some(at) = field("at").and_then(Json::as_str) {
        budget.attribute(&mut element, "at", utc_time(at))?;
    }
    budget.attribute(
        &mut element,
        "direction",
        ecmascript_string(field("direction")),
    )?;
    let who = field("speakers")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|speaker| speaker.get("ref").and_then(Json::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    if !who.is_empty() {
        budget.attribute(&mut element, "who", who)?;
    }
    if field("cut") == Some(&Json::Bool(true)) {
        budget.attribute(&mut element, "cut", "true".to_owned())?;
    }
    let text = field("text").and_then(Json::as_str).unwrap_or_default();
    budget.scalar(text)?;
    element.text = Some(text.to_owned());
    Ok(element)
}

/// `%H:%M:%SZ` of an RFC 3339 UTC timestamp (the register stamps `at` in
/// UTC, `Z`); the text after its `T` up to the seconds, else as given.
fn utc_time(at: &str) -> String {
    at.split_once('T')
        .and_then(|(_, time)| time.get(..8))
        .map_or_else(|| at.to_owned(), |time| format!("{time}Z"))
}

/// A tuple of models: `<name count>` holding one element per model.
fn models(
    name: &str,
    models: &[Json],
    render: Render,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    budget.collection(models.len())?;
    let mut list = budget.element(name)?;
    budget.attribute(&mut list, "count", models.len().to_string())?;
    for model in models {
        list.children.push(render(model.as_object(), budget)?);
    }
    Ok(list)
}

/// The original's `episodes.CognitiveEpisode`: `focus`, `focus_candidates`,
/// `stimulus_name`, `output` (`cognition.types.EventData`), `behavior`.
fn episode(
    episode: Option<&Map<String, Json>>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let field = |key: &str| episode.and_then(|episode| episode.get(key));
    let mut element = budget.element("cognition:cognitive_episode")?;
    fill_field(&mut element, "focus", field("focus"), budget)?;
    let empty = Json::Array(Vec::new());
    let candidates = field("focus_candidates").filter(|value| value.is_array());
    fill_item(
        &mut element,
        "focus_candidates",
        Some(candidates.unwrap_or(&empty)),
        budget,
    )?;
    fill_field(
        &mut element,
        "stimulus_name",
        field("stimulus_name"),
        budget,
    )?;
    let output = field("output")
        .and_then(Json::as_array)
        .map_or(&[][..], Vec::as_slice);
    element
        .children
        .push(models("output", output, selection, budget)?);
    if let Some(behavior) = members(field("behavior")) {
        let mut holder = budget.element("behavior")?;
        holder.children.push(behavior_element(behavior, budget)?);
        element.children.push(holder);
    }
    Ok(element)
}

/// The original's `cognition.types.EventData`: target, event, data, reason.
fn selection(
    selection: Option<&Map<String, Json>>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let mut element = budget.element("cognition:event")?;
    for key in ["target", "event", "data", "reason"] {
        fill_field(
            &mut element,
            key,
            selection.and_then(|selection| selection.get(key)),
            budget,
        )?;
    }
    Ok(element)
}

/// The original's `behavior.events` `CreateData`, `ChangeData`, `BreakData`.
fn behavior_element(
    behavior: &Map<String, Json>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let kind = behavior
        .get("event")
        .and_then(Json::as_str)
        .map_or("create", |event| event.rsplit('.').next().unwrap_or(event));
    let mut element = budget.element(&format!("behavior:{kind}"))?;
    let keys: &[&str] = if kind == "break" {
        &["event", "name", "reason"]
    } else {
        &[
            "event",
            "name",
            "triggers",
            "description",
            "reason",
            "source",
        ]
    };
    for key in keys {
        fill_item(&mut element, key, behavior.get(*key), budget)?;
    }
    Ok(element)
}

/// The original's `directives.Directive`: text, kind (`instruction` by
/// default).
fn directive(
    directive: Option<&Map<String, Json>>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let field = |key: &str| directive.and_then(|directive| directive.get(key));
    let mut element = budget.element("cognition:directive")?;
    fill_field(&mut element, "text", field("text"), budget)?;
    let instruction = Json::String("instruction".to_owned());
    fill_field(
        &mut element,
        "kind",
        Some(field("kind").unwrap_or(&instruction)),
        budget,
    )?;
    Ok(element)
}

/// The original's environment block (`Environment.model_snapshot` from
/// the bot's perspective); `None` when the turn has no environment.
fn environment_element(value: &Json, budget: &mut Budget) -> Result<Option<Element>, &'static str> {
    let fields = match value {
        Json::Object(fields) => Some(fields),
        Json::Array(_) => None,
        _ => return Ok(None),
    };
    let field = |key: &str| fields.and_then(|fields| fields.get(key));
    let mut element = budget.element("environment")?;
    budget.attribute(&mut element, "id", ecmascript_string(field("id")))?;
    let own = members(field("self"));
    let own_field = |key: &str| own.and_then(|own| own.get(key));
    let mut own_element = budget.element("self")?;
    budget.attribute(
        &mut own_element,
        "state",
        ecmascript_string(own_field("state")),
    )?;
    let others: Map<String, Json> = own
        .map(|own| {
            own.iter()
                .filter(|(key, _)| key.as_str() != "state" && key.as_str() != "owned_devices")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    snapshot_attributes(&mut own_element, &others, budget)?;
    let owned = members(own_field("owned_devices"));
    let mut devices = budget.element("owned_devices")?;
    budget.collection(owned.map_or(0, Map::len))?;
    for device in owned.into_iter().flat_map(Map::values) {
        let device = device.as_object();
        let device_field = |key: &str| device.and_then(|device| device.get(key));
        let mut device_element = budget.element("device")?;
        for key in ["ref", "id", "state"] {
            budget.attribute(
                &mut device_element,
                key,
                ecmascript_string(device_field(key)),
            )?;
        }
        let empty = Map::new();
        snapshot_attributes(
            &mut device_element,
            members(device_field("attributes")).unwrap_or(&empty),
            budget,
        )?;
        devices.children.push(device_element);
    }
    own_element.children.push(devices);
    element.children.push(own_element);
    Ok(Some(element))
}

/// A snapshot's attributes as child elements, as the original renders
/// them (see the crate documentation).
fn snapshot_attributes(
    element: &mut Element,
    values: &Map<String, Json>,
    budget: &mut Budget,
) -> Result<(), &'static str> {
    budget.collection(values.len())?;
    for (key, value) in values {
        let tag = snapshot_tag(key);
        match value {
            Json::String(text) => {
                let child = budget.text_element(&tag, text.clone())?;
                element.children.push(child);
            }
            Json::Bool(_) | Json::Number(_) => {
                let text = match value {
                    Json::Number(number) => {
                        number.as_f64().map(ecmascript_number).unwrap_or_default()
                    }
                    other => other.to_string(),
                };
                let child = budget.text_element(&tag, text)?;
                element.children.push(child);
            }
            Json::Array(_) => fill_item(element, &tag, Some(value), budget)?,
            Json::Object(object) => {
                let mut child = budget.element(&tag)?;
                snapshot_attributes(&mut child, object, budget)?;
                element.children.push(child);
            }
            Json::Null => {}
        }
    }
    Ok(())
}

/// The original's sanitized tag of a key, counted in UTF-16 code units as
/// the JavaScript it replaces did.
fn snapshot_tag(key: &str) -> String {
    let mut tag = String::with_capacity(key.len());
    for character in key.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
            tag.push(character);
        } else {
            for _ in 0..character.len_utf16() {
                tag.push('_');
            }
        }
    }
    if tag.is_empty()
        || tag.starts_with(|first: char| first.is_ascii_digit() || first == '.' || first == '-')
    {
        tag.insert(0, '_');
    }
    tag
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

    use super::{Attribute, CognitionHost, Content, Data, Executed, Failure};

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

    impl CognitionHost for Host {
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
