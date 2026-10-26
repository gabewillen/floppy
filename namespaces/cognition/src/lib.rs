//! The `cognition` extension namespace: `<cognition:attention_bias_for_turn>`,
//! `<cognition:model_facing_content>`, `<cognition:model_facing_data>` and
//! `<cognition:within_ceiling>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The turn's attention policy, the projection of a turn for a model,
//! fitted to the model's context when a budget is given, and each tier's
//! frame of the turn's choices under its effort ceiling. All are
//! synchronous and quick, so they are executable content here. Documents
//! bind [`NAMESPACE_URI`] (conventionally with the prefix `cognition`):
//!
//! ```xml
//! <cognition:attention_bias_for_turn location="focus_candidates" focuslocation="focus"
//!                                    stimulus="stimulus" devices="devices" focus="focused_device"/>
//! <cognition:model_facing_content location="prompt" failurelocation="prompt_failure"
//!     systemlocation="system" fitlocation="fit" stimulus="stimulus" self="self" recent="recent"
//!     environment="environment" acted="acted" prior_episodes="prior_episodes"
//!     standing_directives="standing_directives" feedback="feedback" instructions="instructions"
//!     schema="schema" budget="budget" held="held" imageslocation="images"/>
//! <cognition:within_ceiling location="decision_choices" heldlocation="decision_held"
//!     choices="choices" ceiling="decision_effort_ceiling"/>
//! <cognition:model_facing_data location="prompt" root="lesson" data="lessonInput()"/>
//! ```
//!
//! Every attribute but `location`, `focuslocation`, `failurelocation`,
//! `systemlocation`, `fitlocation`, `heldlocation` and `root` is an
//! expression of the document's data model. Each element writes to its
//! locations and never sends.
//!
//! # `<cognition:model_facing_data location="L" root="R" data="D"/>`
//!
//! Any data as model-facing XML, so no prompt holds JSON text (decision
//! D201): `root` (an XML name, not an expression) names the root element,
//! and `data` evaluates to the value it holds. An object's members are the
//! root's fields, in key order: a single-line scalar an attribute, a string
//! over one line (such as a document) an element holding its text, as one
//! CDATA section when it holds markup, so the model reads a document as it
//! would write it, an array an element of `item`s (its `count` an
//! attribute), an object an element of its own fields, a member whose key is
//! not an XML name an `<entry key>`, and `null` and empty collections left
//! out. Any other value is the root's `content` field. The projection's
//! budgets hold; over one, the element raises `error.execution` and assigns
//! nothing.
//!
//! # `<cognition:attention_bias_for_turn location="L" focuslocation="F" stimulus="X" devices="D" focus="G"/>`
//!
//! The turn's attention bias from device identity only. `focus` is the
//! currently focused device (any value; `null` when none), `devices` the
//! configured devices (an array), and `stimulus` the turn's stimulus envelope
//! (an object, `{event, data, source, target?, id?}`).
//!
//! * The focus: `focus` when it is not `null` and is one of `devices`, else
//!   `null`. It is the first focus candidate when it is not `null`.
//! * The stimulus's device: for a `bot.input` stimulus, its data's
//!   `target_device` when the data is an object that has one (the environment
//!   stamps a device's input with it, decision D105), else `null`. It is
//!   appended to the candidates when it is not `null`, is one of `devices`, and
//!   is not a candidate already.
//! * A `bot.perceived` batch (the body's percepts taken together) is biased
//!   percept by percept: each percept's `stimulus`, in arrival order, appends
//!   its device as above. No percept outranks another.
//!
//! `focuslocation` (optional) receives the focus, then `location`
//! (required) receives the candidates, a JSON array. Values compare as JSON
//! values (ECMAScript strict equality on the strings device ids are).
//!
//! # `<cognition:model_facing_content location="L" stimulus="S" …/>`
//!
//! `location` (required) receives the user content a model reads for a
//! turn, its blocks joined by newlines, most stable first (decision D141),
//! each left out when absent:
//!
//! 1. the stable part of what the bot has in mind: `<cognition:recalled>`
//!    holding `recent` (optional, an array of short-term memory's turns, oldest
//!    first), uncounted, each turn one `<memory:turn>` line (`at` as
//!    `%H:%M:%SZ`, `direction`, `who` the speakers' `ref`s when it has any,
//!    `cut="true"` when its words were cut, holding its `text`);
//! 2. the turn's situation: the environment block (decision D106) of
//!    `environment` (optional; an object, anything else renders nothing):
//!    `<environment id>` holding `<self state>` with the bot's own attributes
//!    as children and `<owned_devices>` with one `<device ref id state on?>`
//!    per held device, its attributes as children (`on`, the object it sits on,
//!    only when it is a string), after `<firmware ref id state>` when `self`
//!    has a `firmware` object (the bot's firmware, shown as a device is), then
//!    what earlier stages already did on the turn (`acted`, optional: the
//!    selections they dispatched, an array, or `{percepts, acts}`) as
//!    `<cognition:acted>` with the `percepts` (`[0]` for an array: floppy's
//!    turn has one) and the `acts` as `cognition:event`s, or `<cognition:acted
//!    omitted="size" />` over its 16384 bytes, then the actions held from
//!    this tier (`held`, optional: an array of `{event, description?}`, as
//!    `<cognition:within_ceiling>` writes them; decision D206) as `<held
//!    count>` of one `<event name>` each, holding its description, or
//!    without the descriptions over 16384 bytes, or `<held count omitted="size"
//!    />` still over; nothing when none is held;
//! 3. the rest of what the bot has in mind: `<cognition:recalled>` holding
//!    `prior_episodes` (an array of cognitive episodes: `focus`,
//!    `focus_candidates`, `stimulus_name`, a failed or cancelled turn's
//!    `outcome` and `failure` (decision D139), `output` as
//!    `cognition:event`s and `behavior` as a `behavior:create`,
//!    `behavior:change` or `behavior:break`) and `standing_directives` (an
//!    array of directives: `text`, `kind`, `instruction` by default), each
//!    optional and with its `count`: floppy's recalled memory until
//!    associative memory exists (decision D97);
//! 4. the stimulus (`stimulus`, optional: without it there is no stimulus
//!    block, as in a situation render) as the root element of its own block,
//!    tagged by the JSON Schema type of its data (`object`, `array`, `string`,
//!    `integer` for a number written as an integer, `number` for any other
//!    number, `boolean`, or `null` for no data or a `null` stimulus), a string as its text and anything else as its `content`
//!    field, stamped `stimulus:event`, then `stimulus:id`, `stimulus:source`
//!    and `stimulus:target` when each is a non-empty string; a
//!    `bot.ability.conversation.output`'s messages leave out `provenance`,
//!    `source_ids` and `target_ids`, which a model never sees, and so do its
//!    parent's data and each parent's before it, with `voice_embedding`; an
//!    image it carries (a media object: an object with a string `image`, the
//!    reference floppy passes media by, and a `media_type`, a string or
//!    `null`, as a view or an `environment.visual` has) renders as the
//!    `image` entry's `<media:image media_type? ref="image-N" />`
//!    placeholder (N its place among the stimulus's images in document
//!    order, from 1), which carries the media type in place of the
//!    `media_type` entry, never the reference;
//! 5. the repair `feedback` (optional, a string; `null` is none).
//!
//! `imageslocation` (optional) receives the references of the stimulus's
//! images, in the order of their placeholders' `ref`s (`[]` for none), so a
//! model that takes images is sent each as an image part after the content
//! (decision D250); it reads which is which by its `ref`. Without
//! `imageslocation` the images are not sent, and each placeholder is
//! `<media:image media_type? omitted="media_type" />` instead, as mosfet.py
//! shows an image to a text-only model, so the model knows it is not seeing
//! it.
//!
//! `systemlocation` (optional) receives the system channel: the `instructions`
//! (optional, a string; `null` is none), trimmed, then a blank line and the
//! self block of `self` (optional; the cognition's `self` datum `{name?,
//! traits?, description?, behaviors?}`): `<persona>` holding `<name
//! taken_by="you">` for a `name` string or `<no_name_yet/>` for `null`
//! (nothing without one), then, with traits, `<traits given_by="owner">` of
//! `<trait>`s and `<description written_by="you">` (empty without one), then,
//! with behaviors (each `{name, lifetime, description?}`, the bot's active
//! learned behaviors, decision D154), `<behaviors learned_by="you">` of
//! `<behavior name lifetime>` holding its description, every value escaped;
//! `null` when the instructions and the self block are both empty.
//!
//! A field renders so: `null` is skipped, a scalar is an attribute (a number as
//! its JSON text), an object is an element of `<entry key>` elements each
//! holding its `value`, and an array an element with its `count` of `<item>`
//! elements, a scalar item as its text and an object or array item filled as
//! `item` (decision D99); an empty array or object renders nothing unless it is
//! an entry's value or an item. A snapshot attribute renders as a child element
//! named after its key (each character other than a letter, a digit, `_`, `.`
//! or `-` replaced by `_`, and `_` put first when the tag is empty or starts
//! with a digit, `.` or `-`): a string as its text, a boolean or number as its
//! JSON text, an object as nested elements, an array as a list. Object members
//! render in the data model's canonical order (sorted keys). Each block puts
//! every child element on its own line, indented two spaces per level:
//! attributes in order, an empty element as `<name />`.
//!
//! The budgets hold: 262144 encoded bytes, 4096 nodes, 4096 collection items
//! and 65536 characters per scalar for each block, and 65536 characters for the
//! system channel, the situation and the feedback. The system channel and the
//! tool's schema (`schema`, optional, its compact JSON) are reserved first; the
//! situation and the feedback are never trimmed, and the stimulus renders into
//! the bytes they leave of the prompt's 262144.
//!
//! What the bot has in mind never makes the projection fail (decision
//! D117): its stable part, then its rest, each render into the bytes
//! everything else leaves, their items newest (last) first; an item that
//! would exceed any budget is left out, the kept items keep their order, a
//! counted field's `count` is how many were kept, and `omitted="<n>"`
//! follows when any was left out. A part whose frame alone does not fit is
//! left out. Sizes are measured as rendered and indented, so the text always
//! fits.
//!
//! `budget` (optional, a number of bytes; `null` is none) is the fitted size of
//! the whole request (the model's context less the reserved completion); with
//! it the projection is fitted: the content renders whole, and when it is over
//! the budget less the system channel and the schema it renders again with what
//! the bot has in mind trimmed to that, and `fitlocation` (optional) receives
//! `fit`, `cut` (the trimmed content fits) or `overflow` (it is still over: the
//! stimulus, situation and feedback are never cut, and the request goes as it
//! is). Without a budget `fitlocation` receives `null`.
//!
//! Over a budget otherwise (the stimulus alone overflows, or the system
//! channel, the situation or the feedback is over the scalar budget),
//! `failurelocation` (optional) receives the exceeded budget's message (for
//! example `prompt projection collection item budget exceeded`) and `location`,
//! `systemlocation`, `fitlocation` and `imageslocation` receive `null`; without
//! `failurelocation` it is `error.execution`. Within them, `failurelocation`
//! receives `null`. The locations are assigned in the order `failurelocation`,
//! `systemlocation`, `fitlocation`, `imageslocation`, `location`.
//!
//! No metric counts the left-out items or the fits: this component's
//! telemetry is the closed per-entry-point observation below, so the count
//! is only the `omitted` attribute and the fit is `fitlocation`.
//!
//! # `<cognition:within_ceiling location="L" heldlocation="H" choices="C" ceiling="N"/>`
//!
//! One tier's frame of the turn's choices (decision D206): `choices`
//! (required) evaluates to the turn's choice entries, each an object with a
//! string `event` and optionally an `effort` (a whole number, as
//! an ability's `offerable` data declares it, decision D242; 0 without one), and `ceiling`
//! (optional) to the tier's effort ceiling, a whole number, or `null` (or no
//! attribute) for the whole frame. `location` (required) receives, in order,
//! the entries whose effort is at most the ceiling, each without its
//! `effort` (no stage reads it), and `heldlocation` (optional) receives the
//! actions held from the tier: one `{event, description?}` per event that
//! only entries over the ceiling offer, in order, the description the first
//! such entry's. Both are canonical JSON; `heldlocation` is assigned first.
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
//! The projection is bounded by the budgets above; the data model bounds the
//! values it hands this component.
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

/// The projection's encoded byte budget, and the prompt's.
pub const MAX_PROJECTION_BYTES: usize = 262_144;

/// The projection's node budget.
pub const MAX_PROJECTION_NODES: usize = 4_096;

/// The projection's collection item budget.
pub const MAX_PROJECTION_ITEMS: usize = 4_096;

/// The characters one scalar may have.
pub const MAX_SCALAR_CHARACTERS: usize = 65_536;

/// The message for each exceeded budget.
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
        "model_facing_data" => model_facing_data_element(attributes, content, host),
        "within_ceiling" => within_ceiling(attributes, content, host),
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

/// The turn's focus and focus candidates (see the crate documentation).
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

/// Executes `<cognition:within_ceiling>`.
fn within_ceiling<H: CognitionHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, held_location, choices, ceiling] = attribute_values(
        attributes,
        ["location", "heldlocation", "choices", "ceiling"],
        [
            "cognition:within_ceiling takes only the location, heldlocation, choices and ceiling attributes",
            "cognition:within_ceiling repeats an attribute",
            "cognition:within_ceiling has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "cognition:within_ceiling requires a location",
    ))?;
    let choices = choices.ok_or(Failure::Execution(
        "cognition:within_ceiling requires choices",
    ))?;
    no_content(content, "cognition:within_ceiling holds no content")?;
    let choices = json_of(&evaluate(
        host,
        choices,
        "cognition:within_ceiling choices did not evaluate",
    )?)
    .ok_or(Failure::Execution(
        "cognition:within_ceiling choices is not an array of choices",
    ))?;
    let ceiling = match ceiling {
        None => None,
        Some(ceiling) => match json_of(&evaluate(
            host,
            ceiling,
            "cognition:within_ceiling ceiling did not evaluate",
        )?) {
            Some(Json::Null) => None,
            Some(ceiling) => Some(ceiling.as_u64().ok_or(Failure::Execution(
                "cognition:within_ceiling ceiling is not a whole number or null",
            ))?),
            None => {
                return Err(Failure::Execution(
                    "cognition:within_ceiling ceiling is not a whole number or null",
                ));
            }
        },
    };
    let framed = frame(&choices, ceiling).map_err(Failure::Execution)?;
    if let Some(held_location) = held_location {
        assign_to(
            host,
            held_location,
            &Data::JsonArray(Json::Array(framed.held).to_string()),
            "cognition:within_ceiling could not assign heldlocation",
        )?;
    }
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(framed.offered).to_string()),
        "cognition:within_ceiling could not assign the choices",
    )?;
    Ok(Executed::Handled)
}

/// One tier's frame of the turn's choices (see [`frame`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The choices within the ceiling, each without its `effort`.
    pub offered: Vec<Json>,
    /// The actions held from the tier: `{event, description?}` each.
    pub held: Vec<Json>,
}

/// The frame of `choices` under `ceiling` (`None`: the whole frame): see the
/// crate documentation (`<cognition:within_ceiling>`).
///
/// # Errors
///
/// A static detail when `choices` is not an array of objects with a string
/// `event` and an optional whole-number `effort`.
pub fn frame(choices: &Json, ceiling: Option<u64>) -> Result<Frame, &'static str> {
    const MALFORMED: &str = "cognition:within_ceiling choices is not an array of choices";
    let entries = choices.as_array().ok_or(MALFORMED)?;
    let mut offered = Vec::with_capacity(entries.len());
    let mut over = Vec::new();
    for entry in entries {
        let mut entry = entry.as_object().cloned().ok_or(MALFORMED)?;
        let event = entry
            .get("event")
            .and_then(Json::as_str)
            .ok_or(MALFORMED)?
            .to_owned();
        let effort = match entry.remove("effort") {
            None => 0,
            Some(effort) => effort.as_u64().ok_or(MALFORMED)?,
        };
        if ceiling.is_none_or(|ceiling| effort <= ceiling) {
            offered.push((event, Json::Object(entry)));
        } else {
            over.push((event, entry));
        }
    }
    let mut held: Vec<Json> = Vec::new();
    for (event, entry) in over {
        let named = |item: &Json| item.get("event").and_then(Json::as_str) == Some(&event);
        if offered.iter().any(|(name, _)| *name == event) || held.iter().any(named) {
            continue;
        }
        let mut item = Map::new();
        if let Some(description) = entry.get("description").filter(|value| value.is_string()) {
            item.insert("description".into(), description.clone());
        }
        item.insert("event".into(), Json::String(event));
        held.push(Json::Object(item));
    }
    Ok(Frame {
        offered: offered.into_iter().map(|(_, entry)| entry).collect(),
        held,
    })
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
        held,
        images_location,
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
            "held",
            "imageslocation",
        ],
        [
            "cognition:model_facing_content takes only the location, failurelocation, systemlocation, fitlocation, stimulus, self, recent, environment, acted, prior_episodes, standing_directives, feedback, instructions, schema, budget, held and imageslocation attributes",
            "cognition:model_facing_content repeats an attribute",
            "cognition:model_facing_content has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "cognition:model_facing_content requires a location",
    ))?;
    no_content(content, "cognition:model_facing_content holds no content")?;
    let stimulus = stimulus
        .map(|stimulus| read_stimulus(host, stimulus))
        .transpose()?;
    let known = read_json(host, known, "self")?.unwrap_or(Json::Null);
    let recent = read_list(host, recent, "recent")?;
    let prior_episodes = read_list(host, prior_episodes, "prior_episodes")?;
    let standing_directives = read_list(host, standing_directives, "standing_directives")?;
    let environment = read_json(host, environment, "environment")?.unwrap_or(Json::Null);
    let acted = read_json(host, acted, "acted")?.unwrap_or(Json::Null);
    let held = read_list(host, held, "held")?;
    let feedback = read_text(host, feedback, "feedback")?;
    let instructions = read_text(host, instructions, "instructions")?;
    let tool_bytes =
        read_json(host, schema, "schema")?.map_or(0, |schema| schema.to_string().len());
    let budget = read_budget(host, budget)?;
    let input = TurnInput {
        stimulus: match &stimulus {
            None => Stimulus::Absent,
            Some(None) => Stimulus::Null,
            Some(Some(envelope)) => Stimulus::Envelope(envelope),
        },
        recent: &recent,
        environment: &environment,
        acted: &acted,
        held: &held,
        prior_episodes: &prior_episodes,
        standing_directives: &standing_directives,
        feedback: &feedback,
        images: images_location.is_some(),
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
            images_location,
            Some(location),
        ],
    )?;
    Ok(Executed::Handled)
}

/// Executes `<cognition:model_facing_data>`.
fn model_facing_data_element<H: CognitionHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, root, data] = attribute_values(
        attributes,
        ["location", "root", "data"],
        [
            "cognition:model_facing_data takes only the location, root and data attributes",
            "cognition:model_facing_data repeats an attribute",
            "cognition:model_facing_data has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "cognition:model_facing_data requires a location",
    ))?;
    let root = root.ok_or(Failure::Execution(
        "cognition:model_facing_data requires a root",
    ))?;
    let data = data.ok_or(Failure::Execution(
        "cognition:model_facing_data requires data",
    ))?;
    no_content(content, "cognition:model_facing_data holds no content")?;
    if !is_xml_name(root) {
        return Err(Failure::Execution(
            "cognition:model_facing_data root is not an XML name",
        ));
    }
    let data = read_json(host, Some(data), "data")?.unwrap_or(Json::Null);
    let text = model_facing_data(root, &data).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::String(text),
        "cognition:model_facing_data could not assign the content",
    )?;
    Ok(Executed::Handled)
}

/// `data` as model-facing XML under the root element `root` (see the crate
/// documentation): an object's members as the root's fields, a string over
/// one line as an element holding it (a CDATA section when it holds markup).
///
/// # Errors
///
/// The message of the first projection budget exceeded.
pub fn model_facing_data(root: &str, data: &Json) -> Result<String, &'static str> {
    let mut budget = Budget::default();
    let mut element = budget.element(root)?;
    match data {
        Json::Object(object) => data_fields(&mut element, object, &mut budget)?,
        value => data_field(&mut element, "content", value, &mut budget)?,
    }
    Ok(element.text(0))
}

/// An object's members as `element`'s fields, each a member whose key is
/// not an XML name as an `<entry key>`.
fn data_fields(
    element: &mut Element,
    object: &serde_json::Map<String, Json>,
    budget: &mut Budget,
) -> Result<(), &'static str> {
    for (key, value) in object {
        if is_xml_name(key) {
            data_field(element, key, value, budget)?;
        } else {
            let mut entry = budget.element("entry")?;
            budget.attribute(&mut entry, "key", key.clone())?;
            data_field(&mut entry, "value", value, budget)?;
            element.children.push(entry);
        }
    }
    Ok(())
}

/// `value` as `element`'s field `name`: a single-line scalar an attribute, a
/// string over one line an element holding it, an array an element of
/// `item`s, an object an element of its fields; `null` and empty
/// collections are left out.
fn data_field(
    element: &mut Element,
    name: &str,
    value: &Json,
    budget: &mut Budget,
) -> Result<(), &'static str> {
    match value {
        Json::Null => Ok(()),
        Json::Array(items) if items.is_empty() => Ok(()),
        Json::Object(object) if object.is_empty() => Ok(()),
        Json::String(text) if text.contains('\n') => {
            let mut field = budget.text_element(name, text.clone())?;
            field.cdata = true;
            element.children.push(field);
            Ok(())
        }
        Json::Bool(_) | Json::Number(_) | Json::String(_) => {
            let text = scalar_text(value).unwrap_or_default();
            budget.attribute(element, name, text)
        }
        Json::Array(items) => {
            budget.collection(items.len())?;
            let mut list = budget.element(name)?;
            budget.attribute(&mut list, "count", items.len().to_string())?;
            for item in items {
                let mut entry = match item {
                    Json::Object(_) | Json::Array(_) | Json::Null => budget.element("item")?,
                    Json::String(text) => {
                        let mut entry = budget.text_element("item", text.clone())?;
                        entry.cdata = true;
                        entry
                    }
                    scalar => {
                        budget.text_element("item", scalar_text(scalar).unwrap_or_default())?
                    }
                };
                match item {
                    Json::Object(object) => data_fields(&mut entry, object, budget)?,
                    Json::Array(_) => data_field(&mut entry, "item", item, budget)?,
                    _ => {}
                }
                list.children.push(entry);
            }
            element.children.push(list);
            Ok(())
        }
        Json::Object(object) => {
            let mut field = budget.element(name)?;
            data_fields(&mut field, object, budget)?;
            element.children.push(field);
            Ok(())
        }
    }
}

/// Whether `name` is an XML name a field may take: a letter or `_`, then
/// letters, digits, `_`, `-` and `.`.
fn is_xml_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
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
    locations: [Option<&str>; 5],
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
            data_of(&Json::from(projected.images)).unwrap_or(Data::Null),
            Data::String(projected.content),
        ],
        Err(message) if locations[0].is_some() => [
            Data::String(message.into()),
            Data::Null,
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
        "cognition:model_facing_content could not assign imageslocation",
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
        "held" => (
            "cognition:model_facing_content held did not evaluate",
            "cognition:model_facing_content held is not an array",
        ),
        "feedback" => (
            "cognition:model_facing_content feedback did not evaluate",
            "cognition:model_facing_content feedback is not a string",
        ),
        "instructions" => (
            "cognition:model_facing_content instructions did not evaluate",
            "cognition:model_facing_content instructions is not a string",
        ),
        "data" => (
            "cognition:model_facing_data data did not evaluate",
            "cognition:model_facing_data data is not a JSON value",
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

/// The stimulus a projection shows as its own root element.
#[derive(Clone, Copy, Debug)]
pub enum Stimulus<'a> {
    /// No stimulus block (the `stimulus` attribute left out): the content
    /// is the rest alone, as in a situation render.
    Absent,
    /// A `null` stimulus: a `<null />` root.
    Null,
    /// A stimulus envelope `{event, data?, id?, source?, target?}`.
    Envelope(&'a Map<String, Json>),
}

/// What `<cognition:model_facing_content>` projects, as floppy's JSON: the
/// stimulus, the situation (the environment block, then the `acted` fact),
/// what the bot has in mind (recent turns, episodes, directives) and the
/// feedback.
#[derive(Clone, Copy, Debug)]
pub struct TurnInput<'a> {
    /// The stimulus: its root element, or none.
    pub stimulus: Stimulus<'a>,
    /// The recent turns short-term memory holds, oldest first.
    pub recent: &'a [Json],
    /// The bot's environment, `null` for none.
    pub environment: &'a Json,
    /// What earlier stages already did on the turn: the selections they
    /// dispatched (an array), or `{percepts, acts}`; `null` or an empty
    /// array for nothing.
    pub acted: &'a Json,
    /// The actions held from the tier (decision D206): `{event,
    /// description?}` each; empty for none.
    pub held: &'a [Json],
    /// The prior cognitive episodes (floppy's recalled memory until
    /// associative memory exists, decision D97).
    pub prior_episodes: &'a [Json],
    /// The standing directives (decision D97, as `prior_episodes`).
    pub standing_directives: &'a [Json],
    /// The repair feedback, empty for none.
    pub feedback: &'a str,
    /// Whether the stimulus's images are sent with the content (an
    /// `imageslocation` takes them): else each placeholder says it was
    /// left out.
    pub images: bool,
}

/// How a projection was fitted to a fitted budget.
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
    /// The outcome's name, as `fitlocation` receives it.
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
    /// The references of the images the stimulus carries, in the order of
    /// their placeholders' `ref`s.
    pub images: Vec<String>,
}

/// The system channel: the static `instructions` then the self block of
/// `known`, joined by a blank line; empty when both are.
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

/// The self block of the cognition's `self` datum (`{name?, traits?,
/// description?, behaviors?}`, what identity and persona told it, and the
/// active behaviors it has learned): `<persona>` holding the name (`<name
/// taken_by="you">` for a string, `<no_name_yet/>` for `null`, nothing when
/// absent) and, with traits, the owner's `<traits given_by="owner">`, then the
/// self-written `<description written_by="you">` (empty without one), then,
/// with behaviors, `<behaviors learned_by="you">` (decision D154); empty when
/// `known` is not an object.
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
    let behaviors: Vec<(&str, &str, &str)> = known
        .get("behaviors")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|behavior| {
            Some((
                behavior.get("name")?.as_str()?,
                behavior.get("lifetime")?.as_str()?,
                behavior
                    .get("description")
                    .and_then(Json::as_str)
                    .unwrap_or_default(),
            ))
        })
        .collect();
    if !behaviors.is_empty() {
        block.push_str("<behaviors learned_by=\"you\">");
        for (name, lifetime, description) in behaviors {
            block.push_str("<behavior name=\"");
            block.push_str(&html_escape(name));
            block.push_str("\" lifetime=\"");
            block.push_str(&html_escape(lifetime));
            block.push_str("\">");
            block.push_str(&xml_escape(description));
            block.push_str("</behavior>");
        }
        block.push_str("</behaviors>");
    }
    block.push_str("</persona>");
    block
}

/// `text` with `&`, `<` and `>` escaped.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The model-facing content of `input` fitted to `budget`: the user content of
/// `input`, with the system channel `system` and a tool schema of `tool_bytes`
/// bytes reserved first (see the crate documentation).
///
/// # Errors
///
/// The message of the first budget exceeded: the system channel, the situation
/// or the feedback over the scalar budget, or the stimulus over the bytes they
/// and the tool leave (or any other budget). Recalled memory never fails the
/// projection.
pub fn project(
    input: &TurnInput<'_>,
    system: &str,
    tool_bytes: usize,
    budget: Option<usize>,
) -> Result<Projected, &'static str> {
    let (whole, images) = content(input, system, tool_bytes, None)?;
    let Some(budget) = budget else {
        return Ok(Projected {
            content: whole,
            fit: None,
            images,
        });
    };
    let budget = budget
        .saturating_sub(system.len())
        .saturating_sub(tool_bytes);
    if whole.len() <= budget {
        return Ok(Projected {
            content: whole,
            fit: Some(Fit::Fit),
            images,
        });
    }
    // Over: render again within the budget, which trims or leaves out only
    // the recalled memory (the stimulus, and so its images, renders the
    // same).
    let (content, images) = content(input, system, tool_bytes, Some(budget))?;
    let fit = if content.len() <= budget {
        Fit::Cut
    } else {
        Fit::Overflow
    };
    Ok(Projected {
        content,
        fit: Some(fit),
        images,
    })
}

/// The model-facing content within `budget_bytes`: the stable recalled part,
/// the situation, the varying recalled part, the stimulus, the feedback, joined
/// by newlines, each left out when absent, and the references of the images
/// the stimulus's placeholders attach.
fn content(
    input: &TurnInput<'_>,
    system: &str,
    tool_bytes: usize,
    budget_bytes: Option<usize>,
) -> Result<(String, Vec<String>), &'static str> {
    let situation = situation(input.environment, input.acted, input.held)?;
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
    let mut stimulus_budget = Budget {
        media: true,
        ..Budget::default()
    };
    let mut images = Vec::new();
    let stimulus = match input.stimulus {
        Stimulus::Absent => None,
        Stimulus::Null => Some(stimulus_element(None, &mut stimulus_budget)?.text(0)),
        Stimulus::Envelope(envelope) => {
            let mut root = stimulus_element(Some(envelope), &mut stimulus_budget)?;
            images = attach_images(&mut root, input.images);
            Some(root.text(0))
        }
    };
    let stimulus_bytes = stimulus.as_ref().map_or(0, String::len);
    if stimulus_bytes > limit {
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
        let mut remaining = available.checked_sub(stimulus_bytes + beside);
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
    // Each block joins the next with a newline: the stable recalled
    // memory, the situation, the varying recalled memory, the stimulus,
    // then the feedback.
    let content = [
        stable.as_deref(),
        Some(situation.as_str()),
        varying.as_deref(),
        stimulus.as_deref(),
        Some(input.feedback),
    ]
    .into_iter()
    .flatten()
    .filter(|block| !block.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    Ok((content, images))
}

/// The turn's situation: the environment block, then what earlier stages
/// already did on it, then what is held from the tier, joined by newlines;
/// empty for none.
fn situation(environment: &Json, acted: &Json, held: &[Json]) -> Result<String, &'static str> {
    let mut budget = Budget::default();
    let world = environment_element(environment, &mut budget)?.map(|element| element.text(0));
    let fact = acted_fact(acted);
    let held = held_block(held);
    Ok([world, fact, held]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The `acted` fact of a turn's situation, within `MAX_ACTED_BYTES`:
/// `<cognition:acted>` with the `percepts` acted on and the `acts` dispatched,
/// or the bare element with `omitted="size"` when it does not fit; `None` when
/// nothing was done.
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

/// The actions held from the tier (decision D206), within
/// `MAX_ACTED_BYTES`: `<held count>` of one `<event name>` per item with a
/// string `event`, holding its description; without the descriptions when
/// that does not fit, and `<held count omitted="size" />` when that does not
/// either; `None` when nothing is held.
fn held_block(held: &[Json]) -> Option<String> {
    let held: Vec<(&str, Option<&str>)> = held
        .iter()
        .filter_map(|item| {
            let event = item.get("event")?.as_str()?;
            Some((event, item.get("description").and_then(Json::as_str)))
        })
        .collect();
    if held.is_empty() {
        return None;
    }
    let render = |descriptions: bool| {
        let mut budget = Budget::default();
        budget.collection(held.len())?;
        let mut element = budget.element("held")?;
        budget.attribute(&mut element, "count", held.len().to_string())?;
        for (event, description) in &held {
            let mut child = match description.filter(|_| descriptions) {
                Some(text) => budget.text_element("event", (*text).to_owned())?,
                None => budget.element("event")?,
            };
            budget.attribute(&mut child, "name", (*event).to_owned())?;
            element.children.push(child);
        }
        let text = element.text(0);
        if text.len() > MAX_ACTED_BYTES {
            return Err(BYTE_BUDGET);
        }
        Ok::<_, &'static str>(text)
    };
    Some(
        render(true)
            .or_else(|_| render(false))
            .unwrap_or_else(|_| format!("<held count=\"{}\" omitted=\"size\" />", held.len())),
    )
}

/// The bytes the acted fact may take, and the held block: a sixteenth of
/// [`MAX_PROJECTION_BYTES`].
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

/// One part of what the bot has in mind rendered within `limit` bytes:
/// `<cognition:recalled>` holding each field with items, the rest at their
/// defaults rendering nothing; `None` when its frame does not fit.
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

/// Renders each deferred field's items, newest first, into the bytes the rest
/// of `root` leaves of `limit`. An item that would exceed any budget is left
/// out, and its field's `omitted` counts them (the attribute is dropped when
/// none is); kept items keep their order. Sizes are measured as rendered and
/// indented.
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
        // `</field>` (exactly the bytes the rendered text adds).
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

/// The indentation of one level: two spaces.
const INDENT: &str = "  ";

/// The projection's budget so far: encoded bytes, nodes and collection
/// items.
#[derive(Clone, Copy, Debug, Default)]
struct Budget {
    bytes: usize,
    nodes: usize,
    items: usize,
    /// Whether a media object renders its image as a placeholder (the
    /// stimulus's projection): elsewhere it renders as any object.
    media: bool,
}

impl Budget {
    /// The budget so far, to rewind to.
    fn mark(&self) -> Self {
        *self
    }

    /// Rewinds to a [`Budget::mark`].
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

    /// One element, counted as a node and its name's bytes.
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
            cdata: false,
            image: None,
        })
    }

    /// An image placeholder, `<media:image media_type?>`, for the image
    /// `reference` names, with room counted for its `ref`, which
    /// [`attach_images`] sets.
    fn placeholder(
        &mut self,
        reference: &str,
        media_type: Option<&str>,
    ) -> Result<Element, &'static str> {
        let mut element = self.element(IMAGE_PLACEHOLDER)?;
        if let Some(media_type) = media_type {
            self.attribute(&mut element, "media_type", media_type.to_owned())?;
        }
        self.markup(IMAGE_REF_ROOM)?;
        element.image = Some(reference.to_owned());
        Ok(element)
    }

    /// Sets an attribute, its name counted as bytes and its value as a
    /// scalar.
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

/// `text` with `&`, `<`, `>`, `"` and `'` escaped (`'` as `&#x27;`), as
/// the byte budget counts a scalar.
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
    /// Whether its text, when it holds markup, is written as one CDATA
    /// section (a document a model-facing data field holds), not escaped.
    cdata: bool,
    /// The reference of the image an image placeholder stands for, never
    /// shown; `None` for any other element.
    image: Option<String>,
}

/// An image placeholder's tag.
const IMAGE_PLACEHOLDER: &str = "media:image";

/// The most bytes a placeholder's verdict adds: ` omitted="media_type"`
/// (a ` ref="image-N"` is shorter, N at most the node budget).
const IMAGE_REF_ROOM: &str = " omitted=\"media_type\"";

/// The placeholders of `element` and its descendants, in document order.
fn placeholders(element: &mut Element) -> Vec<&mut Element> {
    if element.image.is_some() {
        return vec![element];
    }
    element.children.iter_mut().flat_map(placeholders).collect()
}

/// Numbers the placeholders under `root` in document order when their
/// images are `sent`: each gains `ref="image-N"`, N its place from 1, and
/// their references are returned in `ref` order. Else each gains
/// `omitted="media_type"`, as mosfet.py's text-only model sees an image,
/// and none is returned.
fn attach_images(root: &mut Element, sent: bool) -> Vec<String> {
    let mut references = Vec::new();
    for element in placeholders(root) {
        if let Some(reference) = &element.image {
            if !sent {
                element
                    .attributes
                    .push(("omitted".to_owned(), "media_type".to_owned()));
                continue;
            }
            references.push(reference.clone());
            element
                .attributes
                .push(("ref".to_owned(), format!("image-{}", references.len())));
        }
    }
    references
}

/// The image a media object carries (an object with a string `image`, the
/// reference floppy passes media by, and a `media_type` member, a string or
/// `null`: a view, a visual stimulus): its reference and media type.
fn media_object(object: &Map<String, Json>) -> Option<(&str, Option<&str>)> {
    let reference = object.get("image")?.as_str()?;
    let media_type = object.get("media_type")?;
    match media_type {
        Json::String(media_type) => Some((reference, Some(media_type))),
        Json::Null => Some((reference, None)),
        _ => None,
    }
}

impl Element {
    /// The element at indentation `level`: each child on its own line,
    /// indented two spaces per level, attributes in order, an empty element
    /// as `<name />`.
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
                let cdata = self.cdata && text.contains(['<', '&']);
                if cdata {
                    out.push_str("<![CDATA[");
                    out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
                    out.push_str("]]>");
                }
                for character in text.chars().filter(|_| !cdata) {
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

/// A scalar's text: a boolean as `true`/`false`, a number as its JSON
/// text, a string as itself; `None` for anything else.
fn scalar_text(value: &Json) -> Option<String> {
    match value {
        Json::Bool(value) => Some(if *value { "true" } else { "false" }.to_owned()),
        Json::Number(value) => value.as_f64().map(ecmascript_number),
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

/// A field over a JSON value (see the crate documentation): an empty array or
/// object renders nothing, as it tells the model nothing its absence does not.
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

/// A value placed as an entry of an enclosing collection, which renders even
/// when it is an empty array or object, keeping that collection's count true.
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
            // A media object's image is its placeholder, which carries its
            // media type: the reference is never shown.
            let image = media_object(object).filter(|_| budget.media);
            for (key, value) in object {
                if image.is_some() && key == "media_type" {
                    continue;
                }
                let mut entry = budget.element("entry")?;
                budget.attribute(&mut entry, "key", key.clone())?;
                match image {
                    Some((reference, media_type)) if key == "image" => entry
                        .children
                        .push(budget.placeholder(reference, media_type)?),
                    _ => fill_item(&mut entry, "value", Some(value), budget)?,
                }
                mapping.children.push(entry);
            }
            element.children.push(mapping);
            Ok(())
        }
    }
}

/// The stimulus as the root element of its own block: tagged by the kind of its
/// data in JSON Schema's type names (`object`, `array`, `string`, `integer`,
/// `number`, `boolean`, `null`), a string as its text and anything else filled
/// as its `content` field, stamped
/// `stimulus:event`, then `stimulus:id`, `:source` and `:target` when set. A
/// conversation output's messages leave out `MESSAGE_EXCLUDED_FIELDS`.
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
        None | Some(Json::Null) => "null",
        Some(Json::Bool(_)) => "boolean",
        Some(Json::Number(number)) if number.is_f64() => "number",
        Some(Json::Number(_)) => "integer",
        Some(Json::String(_)) => "string",
        Some(Json::Array(_)) => "array",
        Some(Json::Object(_)) => "object",
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

/// The fields of a conversation message a model never sees.
const MESSAGE_EXCLUDED_FIELDS: [&str; 3] = ["provenance", "source_ids", "target_ids"];

/// The fields of a parent's data a model never sees: a message's
/// correlation and a voice's embedding (biometric).
const PARENT_EXCLUDED_FIELDS: [&str; 4] =
    ["provenance", "source_ids", "target_ids", "voice_embedding"];

/// A conversation output's data with each of its `messages` without
/// [`MESSAGE_EXCLUDED_FIELDS`], and its `parent`'s data, and each parent's
/// before it (a turn heard as speech carries its speech, mosfet.py's
/// `TurnData.parent`), without [`PARENT_EXCLUDED_FIELDS`].
fn model_facing_messages(data: &Json) -> Json {
    let mut data = data.clone();
    if let Some(messages) = data.get_mut("messages").and_then(Json::as_array_mut) {
        for message in messages.iter_mut().filter_map(Json::as_object_mut) {
            for field in MESSAGE_EXCLUDED_FIELDS {
                message.remove(field);
            }
        }
    }
    let mut parent = data.get_mut("parent");
    while let Some(parent_data) = parent
        .and_then(|parent| parent.get_mut("data"))
        .and_then(Json::as_object_mut)
    {
        for field in PARENT_EXCLUDED_FIELDS {
            parent_data.remove(field);
        }
        parent = parent_data.get_mut("parent");
    }
    data
}

/// Renders one model of a tuple.
type Render = fn(Option<&Map<String, Json>>, &mut Budget) -> Result<Element, &'static str>;

/// One short-term memory turn as a `<memory:turn>` line: `at` (UTC, to the
/// second), `direction`, `who` (each speaker's `ref`) when it has speakers and
/// `cut="true"` when the words were cut, holding the words.
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

/// A cognitive episode: `focus`, `focus_candidates`, `stimulus_name`,
/// `outcome` and `failure` (a failed or cancelled turn's, decision D139),
/// `output` (as `cognition:event`s), `behavior`.
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
    // A failed or cancelled turn's (decision D139); a completed turn's
    // episode has neither.
    fill_field(&mut element, "outcome", field("outcome"), budget)?;
    fill_field(&mut element, "failure", field("failure"), budget)?;
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

/// A selection as a `cognition:event`: target, event, data, reason.
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

/// A behavior event as a `behavior:create`, `behavior:change` or
/// `behavior:break`.
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

/// A directive: text, kind (`instruction` by default).
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

/// The environment block, from the bot's perspective; `None` when the turn has
/// no environment.
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
    // The firmware is shown exactly as a device is, under the tag
    // `firmware`.
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

/// One actor element, a device or the firmware: `<tag ref id state on?>` with
/// its attributes as children.
fn actor_element(
    tag: &str,
    actor: &Map<String, Json>,
    budget: &mut Budget,
) -> Result<Element, &'static str> {
    let mut element = budget.element(tag)?;
    for key in ["ref", "id", "state"] {
        budget.attribute(&mut element, key, ecmascript_string(actor.get(key)))?;
    }
    // Where the actor is, not what it shows: a snapshot's `on` is an
    // attribute of the element.
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

/// A snapshot's attributes as child elements (see the crate
/// documentation).
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

/// A key as a sanitized tag (see the crate documentation), counted in UTF-16
/// code units as the JavaScript it replaces did.
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
