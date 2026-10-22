//! The `cognition` extension namespace: `<cognition:attention_bias_for_turn>`
//! and `<cognition:model_facing_content>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The original's cognition module owns the turn's attention policy
//! (`abilities/cognition/types.py` `attention_bias_for_turn`, with the
//! body's `_interrupt_reference` and `_target_device_reference`) and the
//! projection of a turn's input for a model (`abilities/cognition/event.py`
//! `model_facing_content`, with `processing.py`
//! `InputData.model_facing_content`'s budget for the instructions and the
//! tool). Both are synchronous and quick, so they are executable content
//! here, under the original's names. Documents bind [`NAMESPACE_URI`]
//! (conventionally with the prefix `cognition`):
//!
//! ```xml
//! <cognition:attention_bias_for_turn location="focus_candidates" focuslocation="focus"
//!                                    stimulus="stimulus" devices="devices" focus="focused_device"/>
//! <cognition:model_facing_content location="prompt" failurelocation="prompt_failure"
//!     stimulus="stimulus" prior_episodes="prior_episodes" standing_directives="standing_directives"
//!     environment="environment" instructions="system" schema="schema"/>
//! ```
//!
//! Every attribute but `location`, `focuslocation` and `failurelocation` is
//! an expression of the document's data model. Each element writes to its
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
//! * A `bot.perceived` batch (the body's percepts taken together) is biased
//!   percept by percept, as the original's is: each percept's `stimulus`,
//!   in arrival order, appends its device as above. No percept outranks
//!   another.
//!
//! `focuslocation` (optional) receives the focus, then `location`
//! (required) receives the candidates, a JSON array. Values compare as JSON
//! values (ECMAScript strict equality on the strings device ids are).
//!
//! # `<cognition:model_facing_content location="L" failurelocation="F" stimulus="S" prior_episodes="P" standing_directives="D" environment="E" instructions="I" schema="T"/>`
//!
//! The original's `model_facing_xml(ProcessorInput)` over floppy's JSON:
//! `location` (required) receives the pseudo-XML text a model reads for a
//! turn, `<cognition:processor_input>` holding
//!
//! * `host_input`, a `processing:input` whose `input` field is the
//!   stimulus `stimulus` (required) evaluates to (nothing when it is
//!   `null`): `<input>` with the stimulus's data as its `content` field,
//!   stamped `stimulus:event`, then `stimulus:id`, `stimulus:source` and
//!   `stimulus:target` when each is a non-empty string;
//! * `prior_episodes` (required, an array of the original's
//!   `CognitiveEpisode`s: `focus`, `focus_candidates`, `stimulus_name`,
//!   `output` as `cognition:event`s and `behavior` as a `behavior:create`,
//!   `behavior:change` or `behavior:break`) and `standing_directives`
//!   (required, an array of the original's `Directive`s: `text`, `kind`,
//!   `instruction` by default), each a list with its `count`;
//! * the environment block (decision D106) of `environment` (optional; an
//!   object, anything else renders nothing), the original's
//!   `Environment.model_snapshot` layout: `<environment id>` holding `<self
//!   state>` with the bot's own attributes as children and
//!   `<owned_devices>` with one `<device ref id state on?>` per held
//!   device, its attributes as children (`on`, the object it sits on, only
//!   when it is a string), after `<firmware ref id state>` when `self` has
//!   a `firmware` object (the bot's firmware, shown as a device is).
//!
//! A field renders as the original's `_fill_field` renders a value that is
//! not a model: `null` is skipped, a scalar is an attribute (a number as
//! Python's `str` writes it), an object is an element of `<entry key>`
//! elements each holding its `value`, and an array an element with its
//! `count` of `<item>` elements, a scalar item as its text and an object or
//! array item filled as `item` (decision D99). A snapshot attribute renders
//! as a child element named after its key (each character other than a
//! letter, a digit, `_`, `.` or `-` replaced by `_`, and `_` put first when
//! the tag is empty or starts with a digit, `.` or `-`): a string as its
//! text, a boolean or number as its JSON text, an object as nested
//! elements, an array as a list. Object members render in the data model's
//! canonical order (sorted keys). The text is `ElementTree.indent(space="
//! ")` then `ElementTree.tostring`: attributes in order, an empty element
//! as `<name />`.
//!
//! The original's budgets hold: 262144 encoded bytes, 4096 nodes, 4096
//! collection items and 65536 characters per scalar for the projection,
//! and 65536 characters for the instructions (`instructions`, optional, a
//! string; `null` is none). The instructions and the tool's schema
//! (`schema`, optional, its compact JSON) are reserved first, and the
//! projection renders into the bytes they leave of the prompt's 262144
//! (the original's `InputData.model_facing_content`).
//!
//! `prior_episodes` and `standing_directives` are recalled context, the
//! original's budgeted fields (`__model_facing_budgeted_fields__`, both
//! `"tail"`), so they never make the projection fail (the original's
//! `b1654d6`): their elements are placed where they belong, but their items
//! render after the rest of the projection, each field in turn, newest
//! (last) first, into the bytes the rest leaves. An item that would exceed
//! any budget is left out; the kept items keep their order, the field's
//! `count` is how many were kept, and `omitted="<n>"` follows it when any
//! was left out. Sizes are measured as rendered and indented, so the text
//! always fits.
//!
//! Over a budget otherwise (the stimulus alone overflows, or the
//! instructions are over the scalar budget), `failurelocation` (optional)
//! receives the original's `ValueError` message (for example `prompt
//! projection collection item budget exceeded`) and `location` receives
//! `null`; without `failurelocation` it is `error.execution`. Within them,
//! `failurelocation` receives `null`. `failurelocation` is assigned before
//! `location`.
//!
//! The original also counts the left-out items in its
//! `bot.processing.prompt.recalled.omitted` metric; this component's
//! telemetry is the closed per-entry-point observation below, so the count
//! is only the `omitted` attribute.
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
    for arrived in arrivals(stimulus) {
        let reference = input_device(arrived);
        if !reference.is_null() && devices.contains(&reference) && !candidates.contains(&reference)
        {
            candidates.push(reference);
        }
    }
    (focus, candidates)
}

/// The stimuli a turn perceived, in arrival order: each percept's stimulus
/// of a `bot.perceived` batch (one that is no object arrives as nothing),
/// else the stimulus itself.
fn arrivals(stimulus: &Map<String, Json>) -> Vec<&Map<String, Json>> {
    if stimulus.get("event").and_then(Json::as_str) == Some("bot.perceived")
        && let Some(Json::Array(percepts)) =
            stimulus.get("data").and_then(|data| data.get("percepts"))
    {
        return percepts
            .iter()
            .filter_map(|percept| percept.get("stimulus").and_then(Json::as_object))
            .collect();
    }
    vec![stimulus]
}

/// A `bot.input` stimulus's `target_device`, else `null`.
fn input_device(stimulus: &Map<String, Json>) -> Json {
    if stimulus.get("event").and_then(Json::as_str) == Some("bot.input")
        && let Some(Json::Object(data)) = stimulus.get("data")
    {
        return data.get("target_device").cloned().unwrap_or(Json::Null);
    }
    Json::Null
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
        stimulus,
        prior_episodes,
        standing_directives,
        environment,
        instructions,
        schema,
    ] = attribute_values(
        attributes,
        [
            "location",
            "failurelocation",
            "stimulus",
            "prior_episodes",
            "standing_directives",
            "environment",
            "instructions",
            "schema",
        ],
        [
            "cognition:model_facing_content takes only the location, failurelocation, stimulus, prior_episodes, standing_directives, environment, instructions and schema attributes",
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
    let prior_episodes = prior_episodes.ok_or(Failure::Execution(
        "cognition:model_facing_content requires prior_episodes",
    ))?;
    let standing_directives = standing_directives.ok_or(Failure::Execution(
        "cognition:model_facing_content requires standing_directives",
    ))?;
    no_content(content, "cognition:model_facing_content holds no content")?;
    let stimulus = read_stimulus(host, stimulus)?;
    let prior_episodes = read_array(
        host,
        prior_episodes,
        "cognition:model_facing_content prior_episodes did not evaluate",
        "cognition:model_facing_content prior_episodes is not an array",
    )?;
    let standing_directives = read_array(
        host,
        standing_directives,
        "cognition:model_facing_content standing_directives did not evaluate",
        "cognition:model_facing_content standing_directives is not an array",
    )?;
    let (environment, instructions, tool_bytes) =
        read_reserved(host, environment, instructions, schema)?;
    let input = ProcessorInput {
        stimulus: stimulus.as_ref(),
        prior_episodes: &prior_episodes,
        standing_directives: &standing_directives,
        environment: &environment,
    };
    let (prompt, failure) = match project(&input, &instructions, tool_bytes) {
        Ok(prompt) => (Data::String(prompt), Data::Null),
        Err(message) => match failure_location {
            Some(_) => (Data::Null, Data::String(message.into())),
            None => return Err(Failure::Execution(message)),
        },
    };
    if let Some(failure_location) = failure_location {
        assign_to(
            host,
            failure_location,
            &failure,
            "cognition:model_facing_content could not assign failurelocation",
        )?;
    }
    assign_to(
        host,
        location,
        &prompt,
        "cognition:model_facing_content could not assign the content",
    )?;
    Ok(Executed::Handled)
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

/// The array `expression` evaluates to, else `failed` or `malformed`.
fn read_array<H: CognitionHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Vec<Json>, Failure<H::Error>> {
    match json_of(&evaluate(host, expression, failed)?) {
        Some(Json::Array(items)) => Ok(items),
        _ => Err(Failure::Execution(malformed)),
    }
}

/// `<cognition:model_facing_content>`'s optional inputs: the environment
/// (`null` for none), the instructions (empty for none) and the tool
/// schema's compact JSON bytes (0 for none).
fn read_reserved<H: CognitionHost>(
    host: &mut H,
    environment: Option<&str>,
    instructions: Option<&str>,
    schema: Option<&str>,
) -> Result<(Json, String, usize), Failure<H::Error>> {
    let environment = environment
        .map(|environment| {
            json_of(&evaluate(
                host,
                environment,
                "cognition:model_facing_content environment did not evaluate",
            )?)
            .ok_or(Failure::Execution(
                "cognition:model_facing_content environment is not a JSON value",
            ))
        })
        .transpose()?
        .unwrap_or(Json::Null);
    let instructions = instructions
        .map(|instructions| {
            match evaluate(
                host,
                instructions,
                "cognition:model_facing_content instructions did not evaluate",
            )? {
                Data::String(text) => Ok(text),
                Data::Null => Ok(String::new()),
                _ => Err(Failure::Execution(
                    "cognition:model_facing_content instructions is not a string",
                )),
            }
        })
        .transpose()?
        .unwrap_or_default();
    let tool_bytes = schema
        .map(|schema| {
            json_of(&evaluate(
                host,
                schema,
                "cognition:model_facing_content schema did not evaluate",
            )?)
            .map(|schema| schema.to_string().len())
            .ok_or(Failure::Execution(
                "cognition:model_facing_content schema is not a JSON value",
            ))
        })
        .transpose()?
        .unwrap_or(0);
    Ok((environment, instructions, tool_bytes))
}

/// What `<cognition:model_facing_content>` projects: the original's
/// `ProcessorInput` fields.
#[derive(Clone, Copy, Debug)]
pub struct ProcessorInput<'a> {
    /// The stimulus envelope, `None` for none.
    pub stimulus: Option<&'a Map<String, Json>>,
    /// The prior cognitive episodes.
    pub prior_episodes: &'a [Json],
    /// The standing directives.
    pub standing_directives: &'a [Json],
    /// The bot's environment, `null` for none.
    pub environment: &'a Json,
}

/// The model-facing text of `input` under the original's budgets, with
/// `instructions` and a tool schema of `tool_bytes` bytes reserved from the
/// prompt's first (see the crate documentation).
///
/// # Errors
///
/// The original's message for the first budget exceeded: the instructions
/// over the scalar budget, or the projection without its recalled items
/// over the bytes the instructions and the tool leave (or any other
/// budget). Recalled items never fail the projection: those that do not fit
/// are left out and counted.
pub fn project(
    input: &ProcessorInput<'_>,
    instructions: &str,
    tool_bytes: usize,
) -> Result<String, &'static str> {
    if instructions.chars().count() > MAX_SCALAR_CHARACTERS {
        return Err(SCALAR_BUDGET);
    }
    let limit = MAX_PROJECTION_BYTES
        .saturating_sub(instructions.len())
        .saturating_sub(tool_bytes);
    let mut budget = Budget::default();
    let mut root = budget.element("cognition:processor_input")?;
    let mut host_input = budget.element("host_input")?;
    let mut processing_input = budget.element("processing:input")?;
    if let Some(stimulus) = input.stimulus {
        processing_input
            .children
            .push(stimulus_element(stimulus, &mut budget)?);
    }
    host_input.children.push(processing_input);
    root.children.push(host_input);
    let mut deferred = Vec::new();
    for (name, items, render) in [
        ("prior_episodes", input.prior_episodes, episode as Render),
        ("standing_directives", input.standing_directives, directive),
    ] {
        let (list, recalled) = budgeted(name, items, render, &mut budget)?;
        if let Some(recalled) = recalled {
            deferred.push((root.children.len(), recalled));
        }
        root.children.push(list);
    }
    if let Some(environment) = environment_element(input.environment, &mut budget)? {
        root.children.push(environment);
    }
    render_budgeted(&mut root, &deferred, limit, &mut budget)?;
    let rendered = root.text(0);
    if rendered.len() > limit {
        return Err(BYTE_BUDGET);
    }
    Ok(rendered)
}

/// The original's `_BudgetedField`: a recalled tuple whose items render
/// after the rest of the projection, kept from its tail (both of
/// `ProcessorInput`'s budgeted fields, `prior_episodes` and
/// `standing_directives`, are `"tail"`: recalled oldest first, so the
/// newest survive).
#[derive(Clone, Copy)]
struct Recalled<'a> {
    items: &'a [Json],
    render: Render,
}

/// A budgeted tuple field (the original's `_fill_field` with `keep`): with
/// items, its element is placed now with the upper bounds `count` and
/// `omitted` (both its length) and its items deferred to
/// [`render_budgeted`]; without, it renders as any tuple.
fn budgeted<'a>(
    name: &str,
    items: &'a [Json],
    render: Render,
    budget: &mut Budget,
) -> Result<(Element, Option<Recalled<'a>>), &'static str> {
    if items.is_empty() {
        return Ok((models(name, items, render, budget)?, None));
    }
    let mut list = budget.element(name)?;
    budget.attribute(&mut list, "count", items.len().to_string())?;
    budget.attribute(&mut list, "omitted", items.len().to_string())?;
    Ok((list, Some(Recalled { items, render })))
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
/// `limit`: the stimulus alone overflows.
fn render_budgeted(
    root: &mut Element,
    deferred: &[(usize, Recalled<'_>)],
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
/// documentation).
fn fill_field(
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
                    None => fill_field(&mut list, "item", Some(item), budget)?,
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
                fill_field(&mut entry, "value", Some(value), budget)?;
                mapping.children.push(entry);
            }
            element.children.push(mapping);
            Ok(())
        }
    }
}

/// The stimulus envelope as the original's `hsm.Event` field: `<input>`
/// holding its data as content, stamped `stimulus:event`, then
/// `stimulus:id`, `:source` and `:target` when set.
fn stimulus_element(
    envelope: &Map<String, Json>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let mut input = budget.element("input")?;
    fill_field(&mut input, "content", envelope.get("data"), budget)?;
    let event = envelope
        .get("event")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    budget.attribute(&mut input, "stimulus:event", event)?;
    for key in ["id", "source", "target"] {
        if let Some(text) = envelope.get(key).and_then(Json::as_str)
            && !text.is_empty()
        {
            budget.attribute(&mut input, &format!("stimulus:{key}"), text.to_owned())?;
        }
    }
    Ok(input)
}

/// Renders one model of a tuple.
type Render = fn(Option<&Map<String, Json>>, &mut Budget) -> Result<Element, &'static str>;

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
    fill_field(
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
        fill_field(&mut element, key, behavior.get(*key), budget)?;
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
                .filter(|(key, _)| !["state", "firmware", "owned_devices"].contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    snapshot_attributes(&mut own_element, &others, budget)?;
    // The firmware is shown exactly as a device is (the original's
    // `_render_firmware`), under the reference `firmware`.
    if let Some(firmware) = members(own_field("firmware")) {
        let firmware = actor_element("firmware", firmware, budget)?;
        own_element.children.push(firmware);
    }
    let owned = members(own_field("owned_devices"));
    let mut devices = budget.element("owned_devices")?;
    budget.collection(owned.map_or(0, Map::len))?;
    let empty = Map::new();
    for device in owned.into_iter().flat_map(Map::values) {
        let device = actor_element("device", device.as_object().unwrap_or(&empty), budget)?;
        devices.children.push(device);
    }
    own_element.children.push(devices);
    element.children.push(own_element);
    Ok(Some(element))
}

/// One actor element, a device or the firmware (the original's
/// `_render_actor_element`): `<tag ref id state on?>` with its attributes
/// as children.
fn actor_element(
    tag: &str,
    actor: &Map<String, Json>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let mut element = budget.element(tag)?;
    for key in ["ref", "id", "state"] {
        budget.attribute(&mut element, key, ecmascript_string(actor.get(key)))?;
    }
    // Where the actor is, not what it shows (the original lifts a
    // snapshot's `on` onto the element).
    if let Some(Json::String(on)) = actor.get("on") {
        budget.attribute(&mut element, "on", on.clone())?;
    }
    let empty = Map::new();
    snapshot_attributes(
        &mut element,
        members(actor.get("attributes")).unwrap_or(&empty),
        budget,
    )?;
    Ok(element)
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
            Json::Array(_) => fill_field(element, &tag, Some(value), budget)?,
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
