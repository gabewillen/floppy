//! The `conversation` extension namespace as a
//! `stateforward:scxml/executable-content-extension` Wasm component: the
//! conversation's and its turn detector's bookkeeping, which is
//! synchronous and quick.
//!
//! | Element | Writes |
//! | --- | --- |
//! | `<conversation:message_for_turn>` | the committed message for a turn and its provenance |
//! | `<conversation:relationship_for_input>` | the relationship an input belongs to (by identity collection score), or a new one with its context reference |
//! | `<conversation:people_for_input>` | the input with each voice embedding replaced by its person (decision D194), the people created, the voices to store, and the new people's representative voices |
//! | `<conversation:fold_voices>` | a person's representative voice with more voices folded in (decision D205) |
//! | `<conversation:people_to_merge>` | the two people whose representatives are alike enough to merge first, and their merged representative (decision D205) |
//! | `<conversation:merged_context_refs>` | the relationship context references that change when one person is merged into another (decision D205) |
//! | `<conversation:participant_for_source>` | the relationship with the source as a participant |
//! | `<conversation:fail_turn>` | the relationship without the failed source's participant |
//! | `<conversation:detector_refs>` | every participant's `[session_ref, participant]` |
//! | `<conversation:new_track_ref>` | a participant's track |
//! | `<conversation:content_for_detector>` | the stimulus that opens a source's turn |
//! | `<conversation:complete_turn>` | the inbound message the completed turns make |
//! | `<conversation:ingest_content>` | the open turn's content with a stimulus taken in |
//! | `<conversation:on_end>` | the closed turn, or its failure |
//! | `<conversation:turn_participants>` | the participants a cognition handoff stamps for a turn |
//!
//! Documents bind [`NAMESPACE_URI`] (conventionally with the prefix
//! `conversation`). Every attribute but `location` and `indexlocation` is
//! an expression of the document's data model; an element whose
//! `location` it also reads (`relationship_for_input`,
//! `participant_for_source`, `fail_turn`, `ingest_content`) evaluates it
//! as an expression first. Each element writes to its locations and never
//! sends. No element holds the history: the document appends each message
//! with an `<assign>`, so the history never crosses the extension boundary
//! (whose values are bounded on wasm builds).
//!
//! # Identities
//!
//! An identity is a name (a string) or an embedding (an array of numbers).
//! Two identities are the same when their sort keys are (a name's text,
//! an embedding's JSON text). A relationship is an object
//! `{session_ref, context_ref, source_ids, target_ids, participants}`;
//! its other members are kept as they are.
//!
//! # `<conversation:message_for_turn location="L" direction="D" event="E" message="M" provenance="P"/>`
//!
//! `direction` is the literal `inbound` or `outbound` (not an expression).
//! `event` (required) evaluates to the committing event's name, a non-empty
//! string. `message` (required) evaluates to an object with `source_ids`
//! and `target_ids` (arrays of identities), `content_type` (a non-empty
//! string) and optionally `content` (any value; absent is `null`) and
//! `participants` (at most [`MAX_PARTICIPANTS`] `{ref, role}`, each with an
//! optional boolean `own` and an optional `resembles`, at most
//! [`MAX_RESEMBLANCES`] `{ref, similarity}`; absent or `null` is none); its other members are
//! not committed. `provenance`
//! (optional) evaluates to an object (`null` or `undefined` is none) whose
//! `id`, `session_ref` and `turn_ref` the provenance takes. `location`
//! (required) receives the committed message,
//! `{direction, source_ids, target_ids, content, content_type,
//! participants, provenance: {event, id, source, target, session_ref,
//! turn_ref}}`:
//!
//! * `content`: inbound, `null` when the content type, lower-cased, starts
//!   with `audio/` (media never enters the model-facing history), else the
//!   content as given; outbound, the content as given.
//! * `provenance.id`, `source` and `target`: the provenance's, each when
//!   it is a string, else `null`; `turn_ref` is the provenance's
//!   `turn_ref`, else `null`.
//! * `provenance.session_ref`: the provenance's `session_ref` when it is a
//!   string; else, inbound, the message's own relationship reference
//!   `relationship-<context_ref>`, and outbound `null`.
//!
//! # `<conversation:relationship_for_input location="L" indexlocation="I" input="X"/>`
//!
//! The relationship the input `X` (an object with `source_ids` and
//! `target_ids`) belongs to, among the relationships at `location` (an
//! array, in the order they began). Input without targets shares the first
//! relationship without targets (the ambient one). Other input takes the
//! best scoring relationship, ties to the lower `session_ref`: an ambient
//! one whose participants include a target scores [`SIMILARITY_THRESHOLD`];
//! any other scores the lower of its source and target groups' match, in
//! either direction, the higher direction counting. Two groups match one
//! to one, names equal and embeddings at least [`SIMILARITY_THRESHOLD`]
//! alike by cosine, refused when another assignment comes within
//! [`SIMILARITY_MARGIN`]; the score is the mean. With no match the input
//! begins a relationship of its own, appended: its `context_ref` is the
//! input's relationship context reference ([`relationship_context_ref`]),
//! its `session_ref` `relationship-` and that, its identities sorted, and
//! its participants empty. `location` receives the
//! relationships and `indexlocation` (required) the relationship's index.
//!
//! # `<conversation:people_for_input location="L" input="X" voices="V" after="N"/>`
//!
//! The people of the input `X` (an object with `source_ids` and
//! `target_ids`), as long-term memory holds them (decisions D194 and
//! D205). `voices` (required) evaluates to the representative voices
//! memory holds, one per person, an array of `[person_ref, voice]` (a
//! non-empty string and a representative's compact text,
//! [`encode_voice`]; an entry of another shape is a damaged row and is
//! skipped), and `after` (required) to a whole number, the last number of
//! the people sequence the document holds. Each embedding of the input's
//! `source_ids`, then of its `target_ids`, in order, is matched as
//! [`person_for_voice`] matches it, against the representatives and the
//! voices of the people created before it: a person scores the highest
//! cosine of the voice with theirs, and the best at
//! [`VOICE_SIMILARITY_THRESHOLD`] or more is the voice's person unless another
//! comes within [`SIMILARITY_MARGIN`]; a voice that is no one's is a new
//! person, `person-<n>`, numbered on after `after`. `location` (required)
//! receives `{input, people, voices, representatives}`: `input` the input
//! with each embedding replaced by its person's ref and each list without
//! repeats (the first kept), its other members as they are; `people` the
//! refs of the people created, in order; `voices` `[person_ref,
//! embedding]` for each embedding heard, the embedding as its JSON text,
//! to store for that person; and `representatives` `[person_ref, voice]`,
//! each new person's first voice as its representative's compact text
//! (a voice without a norm has none).
//!
//! # `<conversation:fold_voices location="L" centroid="C" weight="W" voices="V"/>`
//!
//! A person's representative voice reinforced (decision D205):
//! `centroid` (required) evaluates to its compact text, `weight` to the
//! number of voices folded into it, and `voices` to an array of
//! embeddings' JSON texts heard for the person since. `location`
//! (required) receives [`fold_voices`]'s `{centroid, weight, folded}`.
//!
//! # `<conversation:people_to_merge location="L" people="P"/>`
//!
//! `people` (required) evaluates to the representatives memory holds,
//! `[person_ref, voice, weight]` each, oldest person first. `location`
//! (required) receives [`people_to_merge`]'s `{into, from, centroid,
//! weight}`, or `null` when no two people are alike enough to be one.
//!
//! # `<conversation:merged_context_refs location="L" refs="R" from="F" into="I"/>`
//!
//! `refs` (required) evaluates to an array of stored context references,
//! `from` and `into` to two person refs. `location` (required) receives
//! [`merged_context_refs`]'s `[[old, new], ...]`.
//!
//! # `<conversation:participant_for_source location="L" source="S"/>` and `<conversation:fail_turn location="L" source="S"/>`
//!
//! `location` holds one relationship. `participant_for_source` appends the
//! identity `source` to its `participants` when it is not one already (a
//! participant is created on first sight); `fail_turn` drops it (a
//! participant whose detector failed is not kept). `location` receives the
//! relationship.
//!
//! # `<conversation:detector_refs location="L" relationships="R"/>`
//!
//! `location` receives `[session_ref, participant]` of every participant of
//! the relationships `R`, ordered by `session_ref` and then identity.
//!
//! # `<conversation:new_track_ref location="L" source="S"/>`
//!
//! A participant's track: a name is its own track, an embedding `track-`
//! and its unit vector as ECMAScript's `JSON.stringify` writes it. A `null`
//! (or `undefined`) source has no track: `location` receives `null`.
//!
//! # `<conversation:content_for_detector location="L" input="X" source="S"/>`
//!
//! The stimulus that opens the source's turn: `text/plain` (any case)
//! string content as `{kind: "text", source_participant_ref, content}`,
//! `audio/*` string content as `{kind: "audio", source_participant_ref,
//! content, sample_rate_hz, channels}` (each `null` when absent), anything
//! else as `{kind: "content", source_participant_ref, content,
//! content_type}`.
//!
//! # `<conversation:complete_turn location="L" event="E" input="X" turns="T" session_ref="R" participants="P"/>`
//!
//! The inbound message the turns `T` (an array of the detectors' completed
//! turns, at least one) make of the input `X`: as
//! `<conversation:message_for_turn>` commits it, with the turns' texts
//! joined by spaces as `text/plain` when any has text (each distinct text
//! once, decision D79); else no content for `audio/*`; else the input's
//! content; the provenance's `turn_ref` the first turn's and its
//! `session_ref` the string `R`; and its participants `P` (optional, as a
//! message's `participants`: the ones `<conversation:turn_participants>`
//! stamps for the turn; decision D176).
//!
//! # `<conversation:ingest_content location="L" content="C"/>`
//!
//! The open turn's content, `{text_parts, audio, audio_sample_rate_hz,
//! audio_channels, content, content_type}` at `location` (anything but an
//! object is the cleared content), with the stimulus `C` (an object with a
//! `kind`, or `null` for none) taken in: a text's stripped text when not
//! empty; an audio chunk that has content, the first rate and channels
//! winning; a content's content and type; an event as `{event, payload}`
//! of type `application/event`. An image is not the turn's content
//! (images are a turn's attachments), so it changes nothing.
//!
//! # `<conversation:on_end location="L" turn="T" participant_ref="P" conversation_ref="C" turn_ref="R" source_participant_ref="S"/>`
//!
//! The closed turn of the content `T`: `{turn}` (the refs,
//! `self_participant_ref` the participant's, the joined and stripped text,
//! the audio, the content and type of the text, else the audio, else the
//! content, and the rate and channels when there is audio), or `{failure}`
//! (stage `turn`) when the turn has neither text, audio nor content, or
//! both text and audio.
//!
//! # `<conversation:turn_participants location="L" input="X" parent="P"/>`
//!
//! The participants a cognition handoff stamps for the inbound turn that
//! caused it (decision D176): the input
//! `X`'s (an object with `source_ids` and `target_ids`) string identities,
//! in identity order, `source_ids` as `{ref, role:
//! "source"}` and then `target_ids` as `{ref, role: "target"}`, followed by
//! the participants the turn's parent `P` (optional; an envelope, `null`
//! for none) already carries in its data's `participants` (each `{ref,
//! role}`). An embedding, or a name longer than
//! [`MAX_PARTICIPANT_REF_CHARACTERS`] characters (or empty), is not a
//! producer-stable ref and is never stamped. Deduplicated, the first
//! occurrence winning, and bounded at [`MAX_PARTICIPANTS`]. `location`
//! receives the participants, an array.
//!
//! # Failures
//!
//! A malformed element (a missing, empty, unknown or repeated attribute,
//! another `direction`, or any content but white space), an expression that
//! fails or evaluates to a value of the wrong kind, or a failed assignment
//! is an `execution` error with a static detail (SCXML `error.execution`);
//! nothing is assigned then. A host failure that is not recoverable is
//! passed through unchanged.
//!
//! # Bounds
//!
//! The data model bounds the values it hands this component, so the
//! relationships `relationship_for_input` and `detector_refs` read are
//! bounded too.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries an identity, content, an event name,
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
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "conversation";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/conversation";

/// The least cosine similarity at which two embeddings are the same
/// participant.
pub const SIMILARITY_THRESHOLD: f64 = 0.85;

/// The least cosine similarity at which a voice is a person's
/// ([`people::person_for_voice`]): mosfet.py's voice match policy
/// (`identification.MatchPolicy`, threshold 0.50). The speaker embeddings
/// floppy hears (`WeSpeaker` `ResNet34` on api.willen.dev) put one voice's
/// utterances at 0.60 to 0.77 of each other and different voices under
/// 0.25, so [`SIMILARITY_THRESHOLD`] made every utterance a new person.
pub const VOICE_SIMILARITY_THRESHOLD: f64 = 0.50;

/// The least lead the best one-to-one assignment of two identity groups
/// needs over any other.
pub const SIMILARITY_MARGIN: f64 = 0.05;

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
pub trait ConversationHost {
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

/// The direction of a committed message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// From a participant.
    Inbound,
    /// From the bot.
    Outbound,
}

impl Direction {
    fn name(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, value, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: ConversationHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "message_for_turn" => message_for_turn(attributes, content, host),
        "relationship_for_input" => relationship_for_input(attributes, content, host),
        "people_for_input" => people::people_for_input_element(attributes, content, host),
        "fold_voices" => people::fold_voices_element(attributes, content, host),
        "people_to_merge" => people::people_to_merge_element(attributes, content, host),
        "merged_context_refs" => people::merged_context_refs_element(attributes, content, host),
        "participant_for_source" => participant_for_source(attributes, content, host, true),
        "fail_turn" => participant_for_source(attributes, content, host, false),
        "detector_refs" => detector_refs_element(attributes, content, host),
        "new_track_ref" => new_track_ref_element(attributes, content, host),
        "content_for_detector" => content_for_detector_element(attributes, content, host),
        "complete_turn" => complete_turn(attributes, content, host),
        "ingest_content" => ingest_content_element(attributes, content, host),
        "on_end" => on_end_element(attributes, content, host),
        "turn_participants" => turn_participants_element(attributes, content, host),
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
fn evaluate<H: ConversationHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: ConversationHost>(
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

/// Executes `<conversation:message_for_turn>`.
fn message_for_turn<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, direction, event, message, provenance] = attribute_values(
        attributes,
        ["location", "direction", "event", "message", "provenance"],
        "conversation:message_for_turn takes only the location, direction, event, message and provenance attributes",
        "conversation:message_for_turn repeats an attribute",
        "conversation:message_for_turn has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:message_for_turn requires a location",
    ))?;
    let direction = match direction {
        Some("inbound") => Direction::Inbound,
        Some("outbound") => Direction::Outbound,
        Some(_) => {
            return Err(Failure::Execution(
                "conversation:message_for_turn direction is not inbound or outbound",
            ));
        }
        None => {
            return Err(Failure::Execution(
                "conversation:message_for_turn requires a direction",
            ));
        }
    };
    let event = event.ok_or(Failure::Execution(
        "conversation:message_for_turn requires an event",
    ))?;
    let message = message.ok_or(Failure::Execution(
        "conversation:message_for_turn requires a message",
    ))?;
    no_content(content, "conversation:message_for_turn holds no content")?;
    let event = read_event(
        host,
        event,
        [
            "conversation:message_for_turn event did not evaluate",
            "conversation:message_for_turn event is not a non-empty string",
        ],
    )?;
    let Some(Json::Object(message)) = json_of(&evaluate(
        host,
        message,
        "conversation:message_for_turn message did not evaluate",
    )?) else {
        return Err(Failure::Execution(
            "conversation:message_for_turn message is not an object",
        ));
    };
    let provenance = provenance_of(host, provenance)?;
    let (committed, _) = committed_message(direction, &event, &message, &provenance)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(committed).to_string()),
        "conversation:message_for_turn could not assign the message",
    )?;
    Ok(Executed::Handled)
}

/// Evaluates `event` to a non-empty string, else `malformed`.
fn read_event<H: ConversationHost>(
    host: &mut H,
    event: &str,
    [failed, malformed]: [&'static str; 2],
) -> Result<String, Failure<H::Error>> {
    match evaluate(host, event, failed)? {
        Data::String(event) if !event.is_empty() => Ok(event),
        _ => Err(Failure::Execution(malformed)),
    }
}

/// Evaluates `provenance`, when present, to an object (`null` or
/// `undefined` is none).
fn provenance_of<H: ConversationHost>(
    host: &mut H,
    provenance: Option<&str>,
) -> Result<Map<String, Json>, Failure<H::Error>> {
    let Some(provenance) = provenance else {
        return Ok(Map::new());
    };
    match evaluate(
        host,
        provenance,
        "conversation:message_for_turn provenance did not evaluate",
    )? {
        Data::Null | Data::Undefined | Data::Absent => Ok(Map::new()),
        value => match json_of(&value) {
            Some(Json::Object(provenance)) => Ok(provenance),
            _ => Err(Failure::Execution(
                "conversation:message_for_turn provenance is not an object",
            )),
        },
    }
}

/// The committed message and its relationship context reference (see the
/// crate documentation, `<conversation:message_for_turn>`).
///
/// # Errors
///
/// [`Failure::Execution`] when the message's identities or content type are
/// malformed.
pub fn committed_message<E>(
    direction: Direction,
    event: &str,
    message: &Map<String, Json>,
    provenance: &Map<String, Json>,
) -> Result<(Map<String, Json>, String), Failure<E>> {
    let identities = |name: &str, malformed: &'static str| match message.get(name) {
        Some(Json::Array(ids)) if ids.iter().all(is_identity) => Ok(ids.clone()),
        _ => Err(Failure::Execution(malformed)),
    };
    let source_ids = identities(
        "source_ids",
        "conversation:message_for_turn message source_ids is not an array of identities",
    )?;
    let target_ids = identities(
        "target_ids",
        "conversation:message_for_turn message target_ids is not an array of identities",
    )?;
    let content_type = match message.get("content_type") {
        Some(Json::String(content_type)) if !content_type.is_empty() => content_type.clone(),
        _ => {
            return Err(Failure::Execution(
                "conversation:message_for_turn message content_type is not a non-empty string",
            ));
        }
    };
    let content = match direction {
        Direction::Inbound if content_type.to_lowercase().starts_with("audio/") => Json::Null,
        _ => message.get("content").cloned().unwrap_or(Json::Null),
    };
    let participants = match message.get("participants") {
        None | Some(Json::Null) => Vec::new(),
        Some(participants) => message_participants(participants).ok_or(Failure::Execution(
            "conversation message participants are not at most 32 {ref, role, own?, resembles?}",
        ))?,
    };
    let context_ref = relationship_context_ref(&source_ids, &target_ids);
    let string = |name: &str| match provenance.get(name) {
        Some(Json::String(text)) => Json::String(text.clone()),
        _ => Json::Null,
    };
    let session_ref = match (direction, string("session_ref")) {
        (Direction::Inbound, Json::Null) => Json::String(format!("relationship-{context_ref}")),
        (_, session_ref) => session_ref,
    };
    let mut committed_provenance = Map::new();
    committed_provenance.insert("event".into(), Json::String(event.into()));
    committed_provenance.insert("id".into(), string("id"));
    committed_provenance.insert("source".into(), string("source"));
    committed_provenance.insert("target".into(), string("target"));
    committed_provenance.insert("session_ref".into(), session_ref);
    committed_provenance.insert(
        "turn_ref".into(),
        provenance.get("turn_ref").cloned().unwrap_or(Json::Null),
    );
    let mut committed = Map::new();
    committed.insert("direction".into(), Json::String(direction.name().into()));
    committed.insert("source_ids".into(), Json::Array(source_ids));
    committed.insert("target_ids".into(), Json::Array(target_ids));
    committed.insert("content".into(), content);
    committed.insert("content_type".into(), Json::String(content_type));
    committed.insert("participants".into(), Json::Array(participants));
    committed.insert("provenance".into(), Json::Object(committed_provenance));
    Ok((committed, context_ref))
}

/// A message's participants (decision D176): at
/// most [`MAX_PARTICIPANTS`], each `{ref, role}` with an optional boolean
/// `own` and an optional `resembles` ([`is_resembles`]), its ref non-empty and at most [`MAX_PARTICIPANT_REF_CHARACTERS`]
/// characters, its role `source` or `target`; `None` otherwise.
fn message_participants(participants: &Json) -> Option<Vec<Json>> {
    let participants = participants.as_array()?;
    if participants.len() > MAX_PARTICIPANTS {
        return None;
    }
    participants
        .iter()
        .map(|participant| {
            let members = participant.as_object()?;
            let reference = members.get("ref")?.as_str()?;
            let role = members.get("role")?.as_str()?;
            let own = match members.get("own") {
                None => 0,
                Some(Json::Bool(_)) => 1,
                Some(_) => return None,
            };
            let resembles = match members.get("resembles") {
                None => 0,
                Some(resembles) if is_resembles(resembles) => 1,
                Some(_) => return None,
            };
            (members.len() == 2 + own + resembles
                && !reference.is_empty()
                && reference.chars().count() <= MAX_PARTICIPANT_REF_CHARACTERS
                && matches!(role, "source" | "target"))
            .then(|| participant.clone())
        })
        .collect()
}

/// A participant's `resembles`: at
/// most [`MAX_RESEMBLANCES`] `{ref, similarity}`, each ref non-empty and at
/// most [`MAX_PARTICIPANT_REF_CHARACTERS`] characters and each similarity
/// a number from -1 to 1.
fn is_resembles(resembles: &Json) -> bool {
    resembles.as_array().is_some_and(|resembles| {
        resembles.len() <= MAX_RESEMBLANCES
            && resembles.iter().all(|resemblance| {
                resemblance.as_object().is_some_and(|members| {
                    members.len() == 2
                        && members
                            .get("ref")
                            .and_then(Json::as_str)
                            .is_some_and(|reference| {
                                !reference.is_empty()
                                    && reference.chars().count() <= MAX_PARTICIPANT_REF_CHARACTERS
                            })
                        && members
                            .get("similarity")
                            .and_then(Json::as_f64)
                            .is_some_and(|similarity| (-1.0..=1.0).contains(&similarity))
                })
            })
    })
}

/// An identity: a string, or an array of numbers (an embedding).
fn is_identity(value: &Json) -> bool {
    match value {
        Json::String(_) => true,
        Json::Array(values) => values.iter().all(Json::is_number),
        _ => false,
    }
}

/// One identity as a relationship context reference holds it: a name, or
/// an embedding.
#[derive(Clone, Debug, PartialEq)]
enum Identity {
    Name(String),
    Embedding(Vec<f64>),
}

impl Identity {
    /// The identity of a JSON identity (a string, or an array of numbers).
    fn of(value: &Json) -> Option<Self> {
        match value {
            Json::String(name) => Some(Self::Name(name.clone())),
            Json::Array(values) => values
                .iter()
                .map(Json::as_f64)
                .collect::<Option<Vec<_>>>()
                .map(Self::Embedding),
            _ => None,
        }
    }

    /// Its key within a context reference: names before embeddings, a name
    /// by its text, an embedding by its [`key_vector`] text.
    fn sort_key(&self) -> (u8, String) {
        match self {
            Self::Name(name) => (0, name.clone()),
            Self::Embedding(components) => (1, key_vector(components)),
        }
    }

    /// The identity as a context reference writes it: a name as
    /// [`ascii_json_string`], an embedding as [`key_vector`].
    fn json(&self) -> String {
        match self {
            Self::Name(name) => ascii_json_string(name),
            Self::Embedding(components) => key_vector(components),
        }
    }
}

/// The relationship context reference of a source and a target identity
/// set: the JSON text `{"identity_groups":[<group>,<group>]}`. Each
/// embedding is normalized to its unit vector twice (an embedding without
/// one, empty or zero, is kept as given); each group drops repeated
/// identities (a set) and is sorted by [`Identity::sort_key`]; the two
/// groups are ordered by their keys, so the context reference is the same in
/// either direction. Numbers are written by [`key_number`] and names as
/// JSON strings in printable ASCII (every other character escaped).
///
/// A context reference is a stored key: a relationship's memories are kept and
/// recalled by it (`bot_memory.context_ref`, matched byte for byte), and
/// its `session_ref` is `relationship-` and it. Its bytes stay as they are
/// so every stored memory is still recalled.
#[must_use]
pub fn relationship_context_ref(source_ids: &[Json], target_ids: &[Json]) -> String {
    let group = |ids: &[Json]| {
        let mut identities: Vec<((u8, String), Identity)> = ids
            .iter()
            .filter_map(Identity::of)
            .map(|identity| match identity {
                Identity::Embedding(components) => {
                    Identity::Embedding(unit_vector(&unit_vector(&components)))
                }
                name @ Identity::Name(_) => name,
            })
            .map(|identity| (identity.sort_key(), identity))
            .collect();
        identities.sort_by(|left, right| left.0.cmp(&right.0));
        identities.dedup_by(|left, right| left.0 == right.0);
        identities
    };
    let mut groups = [group(source_ids), group(target_ids)];
    groups.sort_by(|left, right| {
        left.iter()
            .map(|(key, _)| key)
            .cmp(right.iter().map(|(key, _)| key))
    });
    let groups: Vec<String> = groups
        .iter()
        .map(|group| {
            let identities: Vec<String> =
                group.iter().map(|(_, identity)| identity.json()).collect();
            format!("[{}]", identities.join(","))
        })
        .collect();
    format!("{{\"identity_groups\":[{}]}}", groups.join(","))
}

/// The vector over its L2 norm (the squares summed exactly by [`fsum`];
/// scaled by the largest magnitude first
/// when they overflow), each zero component a positive zero; an empty or
/// zero vector is returned as given.
fn unit_vector(components: &[f64]) -> Vec<f64> {
    let components: Vec<f64> = components
        .iter()
        .map(|component| if *component == 0.0 { 0.0 } else { *component })
        .collect();
    let squared = fsum(components.iter().map(|component| component * component));
    let (components, norm) = if squared.is_finite() {
        (components, squared.sqrt())
    } else {
        let scale = components
            .iter()
            .fold(0.0_f64, |largest, component| largest.max(component.abs()));
        let scaled: Vec<f64> = components
            .iter()
            .map(|component| component / scale)
            .collect();
        let norm = fsum(scaled.iter().map(|component| component * component)).sqrt();
        (scaled, norm)
    };
    if components.is_empty() || norm == 0.0 || !norm.is_finite() {
        return components;
    }
    components
        .iter()
        .map(|component| component / norm)
        .collect()
}

/// The correctly rounded sum of `values` (Shewchuk's partials, rounded
/// half to even); a non-finite partial makes the sum non-finite.
fn fsum(values: impl Iterator<Item = f64>) -> f64 {
    let mut partials: Vec<f64> = Vec::new();
    for value in values {
        let mut x = value;
        let mut kept = 0;
        for index in 0..partials.len() {
            let mut y = partials[index];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let high = x + y;
            let low = y - (high - x);
            if low != 0.0 {
                partials[kept] = low;
                kept += 1;
            }
            x = high;
        }
        partials.truncate(kept);
        partials.push(x);
        if !x.is_finite() {
            return x;
        }
    }
    let Some(mut high) = partials.pop() else {
        return 0.0;
    };
    let mut low = 0.0;
    while let Some(y) = partials.pop() {
        let x = high;
        high = x + y;
        low = y - (high - x);
        if low != 0.0 {
            break;
        }
    }
    if let Some(next) = partials.last()
        && ((low < 0.0 && *next < 0.0) || (low > 0.0 && *next > 0.0))
    {
        let y = low * 2.0;
        let x = high + y;
        // Exact float equality is the algorithm: the doubled remainder
        // survived the addition unrounded.
        #[allow(clippy::float_cmp, reason = "fsum's exact rounding test")]
        let exact = y == x - high;
        if exact {
            high = x;
        }
    }
    high
}

/// An embedding as a context reference writes it: `[` and its components
/// as [`key_number`] writes them, comma-separated, then `]`.
fn key_vector(components: &[f64]) -> String {
    let texts: Vec<String> = components.iter().copied().map(key_number).collect();
    format!("[{}]", texts.join(","))
}

/// A finite number as a context reference writes it: the shortest
/// round-trip digits, positional with at least one fraction digit when the
/// decimal exponent is from -4 to 15 (`1.0`, `0.0001`), else
/// `d[.ddd]e±XX` with at least two exponent digits (`1e-05`, `1e+16`).
#[must_use]
pub fn key_number(value: f64) -> String {
    if !value.is_finite() {
        return "NaN".into();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }
    // Rust's `{:e}` writes the shortest round-trip digits: `d[.ddd]e<exp>`.
    let scientific = format!("{:e}", value.abs());
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let Ok(exponent) = exponent.parse::<i64>() else {
        return scientific;
    };
    if !(-4..16).contains(&exponent) {
        let exponent_sign = if exponent < 0 { "-" } else { "+" };
        return format!("{sign}{mantissa}e{exponent_sign}{:02}", exponent.abs());
    }
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let whole_digits = usize::try_from(exponent + 1).unwrap_or(0);
    let body = if exponent < 0 {
        let zeros = usize::try_from(-exponent - 1).unwrap_or(0);
        format!("0.{}{digits}", "0".repeat(zeros))
    } else if digits.len() <= whole_digits {
        format!("{digits}{}.0", "0".repeat(whole_digits - digits.len()))
    } else {
        let (whole, fraction) = digits.split_at(whole_digits);
        format!("{whole}.{fraction}")
    };
    format!("{sign}{body}")
}

/// A string as a context reference writes it, a JSON string in printable
/// ASCII: quote, backslash and the control characters `\b \f \n \r \t` escaped
/// short, every other character outside `' '..='~'` as lower-case `\uxxxx`
/// (UTF-16 surrogate pairs beyond the BMP).
fn ascii_json_string(text: &str) -> String {
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
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0_u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    out.push_str("\\u");
                    for shift in [12, 8, 4, 0] {
                        let digit = u32::from((*unit >> shift) & 0xf);
                        out.push(char::from_digit(digit, 16).unwrap_or('0'));
                    }
                }
            }
        }
    }
    out.push('"');
    out
}

/// Evaluates `expression` to JSON, else `malformed`.
fn read_json<H: ConversationHost>(
    host: &mut H,
    expression: &str,
    [failed, malformed]: [&'static str; 2],
) -> Result<Json, Failure<H::Error>> {
    json_of(&evaluate(host, expression, failed)?).ok_or(Failure::Execution(malformed))
}

/// Evaluates `expression` to an identity, or `None` for `null` and
/// `undefined`; anything else is `malformed`.
fn read_source<H: ConversationHost>(
    host: &mut H,
    expression: &str,
    [failed, malformed]: [&'static str; 2],
) -> Result<Option<Json>, Failure<H::Error>> {
    match evaluate(host, expression, failed)? {
        Data::Null | Data::Undefined | Data::Absent => Ok(None),
        value => match json_of(&value) {
            Some(source) if is_identity(&source) => Ok(Some(source)),
            _ => Err(Failure::Execution(malformed)),
        },
    }
}

/// The identities of `name` in `object`, else `malformed`.
fn identities_of<'a>(
    object: &'a Map<String, Json>,
    name: &str,
    malformed: &'static str,
) -> Result<&'a [Json], &'static str> {
    match object.get(name) {
        Some(Json::Array(ids)) if ids.iter().all(is_identity) => Ok(ids),
        _ => Err(malformed),
    }
}

/// One identity's key in identity order, compared in code point order: a
/// name's text, else an embedding's JSON text, names first (as
/// `<identity:sorted_identities>` orders them); two identities are the
/// same when their keys are.
fn identity_key(id: &Json) -> String {
    match id {
        Json::String(name) => format!("0{name}"),
        other @ Json::Array(_) => format!("1{other}"),
        other => format!("2{other}"),
    }
}

/// The identities in identity order (stable, repeats kept).
fn sorted_identities(ids: &[Json]) -> Vec<Json> {
    let mut keyed: Vec<(String, &Json)> = ids.iter().map(|id| (identity_key(id), id)).collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.into_iter().map(|(_, id)| id.clone()).collect()
}

/// An embedding's components, or `None` for a name.
fn embedding(id: &Json) -> Option<Vec<f64>> {
    match id {
        Json::Array(components) => Some(
            components
                .iter()
                .map(|component| component.as_f64().unwrap_or(f64::NAN))
                .collect(),
        ),
        _ => None,
    }
}

/// An embedding over its norm, the squares summed in order (as the
/// conversation's JavaScript did); `None` for a vector without one.
fn ordered_unit_vector(components: &[f64]) -> Option<Vec<f64>> {
    let mut sum = 0.0;
    for component in components {
        sum += component * component;
    }
    let norm = sum.sqrt();
    (norm > 0.0).then(|| {
        components
            .iter()
            .map(|component| component / norm)
            .collect()
    })
}

/// The cosine similarity of two vectors: `None` for vectors of other
/// sizes or without a norm.
fn cosine_similarity(left: &[f64], right: &[f64]) -> Option<f64> {
    let left = ordered_unit_vector(left)?;
    let right = ordered_unit_vector(right)?;
    if left.len() != right.len() {
        return None;
    }
    let mut sum = 0.0;
    for (left, right) in left.iter().zip(&right) {
        sum += left * right;
    }
    Some(sum)
}

/// Whether `target` is `participant`: names equal, or embeddings at least
/// [`SIMILARITY_THRESHOLD`] alike.
fn is_same_participant(target: &Json, participant: &Json) -> bool {
    match (embedding(target), embedding(participant)) {
        (Some(target), Some(participant)) => cosine_similarity(&target, &participant)
            .is_some_and(|score| score >= SIMILARITY_THRESHOLD),
        _ => target == participant,
    }
}

/// One pair's score: names equal score 1, embeddings their cosine when at
/// least [`SIMILARITY_THRESHOLD`]; anything else does not match.
fn pair_score(left: &Json, right: &Json) -> Option<f64> {
    match (embedding(left), embedding(right)) {
        (Some(left), Some(right)) => {
            cosine_similarity(&left, &right).filter(|score| *score >= SIMILARITY_THRESHOLD)
        }
        _ => (left == right).then_some(1.0),
    }
}

/// The highest total one-to-one assignment of rows to columns (its total
/// and each row's column) without the forbidden row and column pair, the
/// first found among equal totals.
fn best_assignment(
    scores: &[Vec<Option<f64>>],
    forbidden: Option<(usize, usize)>,
) -> Option<(f64, Vec<usize>)> {
    fn visit(
        scores: &[Vec<Option<f64>>],
        forbidden: Option<(usize, usize)>,
        row: usize,
        total: f64,
        columns: &mut Vec<usize>,
        best: &mut Option<(f64, Vec<usize>)>,
    ) {
        let Some(cells) = scores.get(row) else {
            if best.as_ref().is_none_or(|(most, _)| total > *most) {
                *best = Some((total, columns.clone()));
            }
            return;
        };
        for (column, score) in cells.iter().enumerate() {
            if let Some(score) = score
                && !columns.contains(&column)
                && forbidden != Some((row, column))
            {
                columns.push(column);
                visit(scores, forbidden, row + 1, total + score, columns, best);
                columns.pop();
            }
        }
    }
    let mut best = None;
    visit(scores, forbidden, 0, 0.0, &mut Vec::new(), &mut best);
    best
}

/// The identity collection score of two identity groups: the
/// mean of their best one-to-one match, refused when another assignment
/// comes within [`SIMILARITY_MARGIN`]; `None` for groups of other sizes,
/// empty ones, or no match.
fn collection_score(observed: &[Json], expected: &[Json]) -> Option<f64> {
    if observed.len() != expected.len() || observed.is_empty() {
        return None;
    }
    let scores: Vec<Vec<Option<f64>>> = observed
        .iter()
        .map(|left| {
            expected
                .iter()
                .map(|right| pair_score(left, right))
                .collect()
        })
        .collect();
    let (total, columns) = best_assignment(&scores, None)?;
    let mut second: Option<f64> = None;
    for (row, column) in columns.iter().enumerate() {
        if let Some((alternative, _)) = best_assignment(&scores, Some((row, *column)))
            && second.is_none_or(|second| alternative > second)
        {
            second = Some(alternative);
        }
    }
    if second.is_some_and(|second| total - second <= SIMILARITY_MARGIN) {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "a group holds a handful of identities"
    )]
    let count = observed.len() as f64;
    Some(total / count)
}

/// The lower of two scores, `None` when either is.
fn lower_score(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    Some(left?.min(right?))
}

/// One relationship's fields the score reads.
struct Profile<'a> {
    session_ref: &'a str,
    source_ids: &'a [Json],
    target_ids: &'a [Json],
    participants: &'a [Json],
}

/// The relationship at one position, else a static detail.
fn profile(relationship: &Json) -> Result<Profile<'_>, &'static str> {
    const MALFORMED: &str =
        "conversation:relationship_for_input location is not an array of relationships";
    let Json::Object(relationship) = relationship else {
        return Err(MALFORMED);
    };
    let Some(Json::String(session_ref)) = relationship.get("session_ref") else {
        return Err(MALFORMED);
    };
    Ok(Profile {
        session_ref,
        source_ids: identities_of(relationship, "source_ids", MALFORMED)?,
        target_ids: identities_of(relationship, "target_ids", MALFORMED)?,
        participants: identities_of(relationship, "participants", MALFORMED)?,
    })
}

/// One relationship's score for the input's sources and targets.
fn relationship_score(candidate: &Profile<'_>, sources: &[Json], targets: &[Json]) -> Option<f64> {
    if candidate.target_ids.is_empty()
        && targets.iter().any(|target| {
            candidate
                .participants
                .iter()
                .any(|participant| is_same_participant(target, participant))
        })
    {
        return Some(SIMILARITY_THRESHOLD);
    }
    let direct = lower_score(
        collection_score(sources, candidate.source_ids),
        collection_score(targets, candidate.target_ids),
    );
    let reverse = lower_score(
        collection_score(sources, candidate.target_ids),
        collection_score(targets, candidate.source_ids),
    );
    match (direct, reverse) {
        (None, reverse) => reverse,
        (direct, None) => direct,
        (Some(direct), Some(reverse)) => Some(direct.max(reverse)),
    }
}

/// ECMAScript's `<` of two strings: UTF-16 code unit order.
fn utf16_less(left: &str, right: &str) -> bool {
    left.encode_utf16().lt(right.encode_utf16())
}

/// The index of the relationship the input belongs to, else `None` (see
/// the crate documentation, `<conversation:relationship_for_input>`).
///
/// # Errors
///
/// A static detail when a relationship is malformed.
pub fn relationship_index_for(
    relationships: &[Json],
    sources: &[Json],
    targets: &[Json],
) -> Result<Option<usize>, &'static str> {
    let profiles = relationships
        .iter()
        .map(profile)
        .collect::<Result<Vec<_>, _>>()?;
    if targets.is_empty() {
        return Ok(profiles.iter().position(|each| each.target_ids.is_empty()));
    }
    let mut best: Option<(usize, f64)> = None;
    for (index, each) in profiles.iter().enumerate() {
        let Some(score) = relationship_score(each, sources, targets) else {
            continue;
        };
        let better = match best {
            None => true,
            // Exact float equality is the JavaScript it replaces: a tie.
            #[allow(clippy::float_cmp, reason = "a tie between equal scores")]
            Some((at, most)) => {
                score > most
                    || (score == most
                        && profiles
                            .get(at)
                            .is_some_and(|held| utf16_less(each.session_ref, held.session_ref)))
            }
        };
        if better {
            best = Some((index, score));
        }
    }
    Ok(best.map(|(index, _)| index))
}

/// Executes `<conversation:relationship_for_input>`.
fn relationship_for_input<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, index_location, input] = attribute_values(
        attributes,
        ["location", "indexlocation", "input"],
        "conversation:relationship_for_input takes only the location, indexlocation and input attributes",
        "conversation:relationship_for_input repeats an attribute",
        "conversation:relationship_for_input has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:relationship_for_input requires a location",
    ))?;
    let index_location = index_location.ok_or(Failure::Execution(
        "conversation:relationship_for_input requires an indexlocation",
    ))?;
    let input = input.ok_or(Failure::Execution(
        "conversation:relationship_for_input requires an input",
    ))?;
    no_content(
        content,
        "conversation:relationship_for_input holds no content",
    )?;
    let Json::Array(mut relationships) = read_json(
        host,
        location,
        [
            "conversation:relationship_for_input location did not evaluate",
            "conversation:relationship_for_input location is not an array of relationships",
        ],
    )?
    else {
        return Err(Failure::Execution(
            "conversation:relationship_for_input location is not an array of relationships",
        ));
    };
    let Json::Object(input) = read_json(
        host,
        input,
        [
            "conversation:relationship_for_input input did not evaluate",
            "conversation:relationship_for_input input is not an object",
        ],
    )?
    else {
        return Err(Failure::Execution(
            "conversation:relationship_for_input input is not an object",
        ));
    };
    let index = relationship_for(&mut relationships, &input).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(relationships).to_string()),
        "conversation:relationship_for_input could not assign the relationships",
    )?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "the data model bounds the relationships far under 2^53"
    )]
    let index = index as f64;
    let index = scxml_types::FiniteNumber::new(index).map_err(|_| {
        Failure::Execution("conversation:relationship_for_input could not assign the index")
    })?;
    assign_to(
        host,
        index_location,
        &Data::Number(index),
        "conversation:relationship_for_input could not assign the index",
    )?;
    Ok(Executed::Handled)
}

/// The index of the input's
/// relationship among `relationships`, appending a new one when none
/// matches.
///
/// # Errors
///
/// A static detail when a relationship or the input is malformed.
pub fn relationship_for(
    relationships: &mut Vec<Json>,
    input: &Map<String, Json>,
) -> Result<usize, &'static str> {
    let sources = identities_of(
        input,
        "source_ids",
        "conversation:relationship_for_input input source_ids is not an array of identities",
    )?;
    let targets = identities_of(
        input,
        "target_ids",
        "conversation:relationship_for_input input target_ids is not an array of identities",
    )?;
    if let Some(index) = relationship_index_for(relationships, sources, targets)? {
        return Ok(index);
    }
    let context_ref = relationship_context_ref(sources, targets);
    let mut relationship = Map::new();
    relationship.insert(
        "session_ref".into(),
        Json::String(format!("relationship-{context_ref}")),
    );
    relationship.insert("context_ref".into(), Json::String(context_ref));
    relationship.insert("source_ids".into(), Json::Array(sorted_identities(sources)));
    relationship.insert("target_ids".into(), Json::Array(sorted_identities(targets)));
    relationship.insert("participants".into(), Json::Array(Vec::new()));
    relationships.push(Json::Object(relationship));
    Ok(relationships.len() - 1)
}

/// Executes `<conversation:participant_for_source>` (`add`) and
/// `<conversation:fail_turn>`.
fn participant_for_source<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
    add: bool,
) -> Result<Executed, Failure<H::Error>> {
    let details = if add {
        [
            "conversation:participant_for_source takes only the location and source attributes",
            "conversation:participant_for_source repeats an attribute",
            "conversation:participant_for_source has an empty attribute",
            "conversation:participant_for_source requires a location",
            "conversation:participant_for_source requires a source",
            "conversation:participant_for_source holds no content",
            "conversation:participant_for_source location did not evaluate",
            "conversation:participant_for_source location is not a relationship",
            "conversation:participant_for_source source did not evaluate",
            "conversation:participant_for_source source is not an identity",
            "conversation:participant_for_source could not assign the relationship",
        ]
    } else {
        [
            "conversation:fail_turn takes only the location and source attributes",
            "conversation:fail_turn repeats an attribute",
            "conversation:fail_turn has an empty attribute",
            "conversation:fail_turn requires a location",
            "conversation:fail_turn requires a source",
            "conversation:fail_turn holds no content",
            "conversation:fail_turn location did not evaluate",
            "conversation:fail_turn location is not a relationship",
            "conversation:fail_turn source did not evaluate",
            "conversation:fail_turn source is not an identity",
            "conversation:fail_turn could not assign the relationship",
        ]
    };
    let [
        refused,
        repeated,
        empty,
        no_location,
        no_source,
        has_content,
        location_failed,
        not_relationship,
        source_failed,
        not_identity,
        not_assigned,
    ] = details;
    let [location, source] =
        attribute_values(attributes, ["location", "source"], refused, repeated, empty)?;
    let location = location.ok_or(Failure::Execution(no_location))?;
    let source = source.ok_or(Failure::Execution(no_source))?;
    no_content(content, has_content)?;
    let Json::Object(mut relationship) =
        read_json(host, location, [location_failed, not_relationship])?
    else {
        return Err(Failure::Execution(not_relationship));
    };
    let Some(Json::Array(participants)) = relationship.get_mut("participants") else {
        return Err(Failure::Execution(not_relationship));
    };
    let source = read_source(host, source, [source_failed, not_identity])?
        .ok_or(Failure::Execution(not_identity))?;
    let key = identity_key(&source);
    if add {
        if !participants
            .iter()
            .any(|participant| identity_key(participant) == key)
        {
            participants.push(source);
        }
    } else {
        participants.retain(|participant| identity_key(participant) != key);
    }
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(relationship).to_string()),
        not_assigned,
    )?;
    Ok(Executed::Handled)
}

/// Executes `<conversation:detector_refs>`.
fn detector_refs_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    const MALFORMED: &str =
        "conversation:detector_refs relationships is not an array of relationships";
    let [location, relationships] = attribute_values(
        attributes,
        ["location", "relationships"],
        "conversation:detector_refs takes only the location and relationships attributes",
        "conversation:detector_refs repeats an attribute",
        "conversation:detector_refs has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:detector_refs requires a location",
    ))?;
    let relationships = relationships.ok_or(Failure::Execution(
        "conversation:detector_refs requires relationships",
    ))?;
    no_content(content, "conversation:detector_refs holds no content")?;
    let Json::Array(relationships) = read_json(
        host,
        relationships,
        [
            "conversation:detector_refs relationships did not evaluate",
            MALFORMED,
        ],
    )?
    else {
        return Err(Failure::Execution(MALFORMED));
    };
    let refs = detector_refs(&relationships).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(refs).to_string()),
        "conversation:detector_refs could not assign the detector references",
    )?;
    Ok(Executed::Handled)
}

/// `[session_ref,
/// participant]` of every participant, ordered by `session_ref` (UTF-16
/// code units) and then identity.
///
/// # Errors
///
/// A static detail when a relationship is malformed.
pub fn detector_refs(relationships: &[Json]) -> Result<Vec<Json>, &'static str> {
    const MALFORMED: &str =
        "conversation:detector_refs relationships is not an array of relationships";
    let mut refs: Vec<(&str, String, &Json)> = Vec::new();
    for relationship in relationships {
        let Json::Object(relationship) = relationship else {
            return Err(MALFORMED);
        };
        let Some(Json::String(session_ref)) = relationship.get("session_ref") else {
            return Err(MALFORMED);
        };
        for participant in identities_of(relationship, "participants", MALFORMED)? {
            refs.push((session_ref, identity_key(participant), participant));
        }
    }
    refs.sort_by(|left, right| {
        left.0
            .encode_utf16()
            .cmp(right.0.encode_utf16())
            .then_with(|| left.1.cmp(&right.1))
    });
    Ok(refs
        .into_iter()
        .map(|(session_ref, _, participant)| {
            Json::Array(vec![Json::String(session_ref.into()), participant.clone()])
        })
        .collect())
}

/// Executes `<conversation:new_track_ref>`.
fn new_track_ref_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, source] = attribute_values(
        attributes,
        ["location", "source"],
        "conversation:new_track_ref takes only the location and source attributes",
        "conversation:new_track_ref repeats an attribute",
        "conversation:new_track_ref has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:new_track_ref requires a location",
    ))?;
    let source = source.ok_or(Failure::Execution(
        "conversation:new_track_ref requires a source",
    ))?;
    no_content(content, "conversation:new_track_ref holds no content")?;
    let source = read_source(
        host,
        source,
        [
            "conversation:new_track_ref source did not evaluate",
            "conversation:new_track_ref source is not an identity or null",
        ],
    )?;
    let track = source
        .as_ref()
        .map_or(Data::Null, |source| Data::String(new_track_ref(source)));
    assign_to(
        host,
        location,
        &track,
        "conversation:new_track_ref could not assign the track",
    )?;
    Ok(Executed::Handled)
}

/// The track of one identity: a name is its own
/// track, an embedding `track-` and its unit vector (the squares summed in
/// order) as ECMAScript's `JSON.stringify` writes it.
#[must_use]
pub fn new_track_ref(source: &Json) -> String {
    match source {
        Json::String(name) => name.clone(),
        other => {
            let components = embedding(other).unwrap_or_default();
            let mut sum = 0.0;
            for component in &components {
                sum += component * component;
            }
            let norm = sum.sqrt();
            let texts: Vec<String> = components
                .iter()
                .map(|component| ecmascript_number(component / norm))
                .collect();
            format!("track-[{}]", texts.join(","))
        }
    }
}

/// A number as ECMAScript's `JSON.stringify` writes it (`null` when it is
/// not finite).
fn ecmascript_number(value: f64) -> String {
    if !value.is_finite() {
        return "null".into();
    }
    if value.fract() == 0.0 && value.abs() < 1e21 {
        if value == 0.0 {
            return "0".into();
        }
        return format!("{value:.0}");
    }
    if (1e-6..1e21).contains(&value.abs()) {
        return value.to_string();
    }
    let text = format!("{value:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if exponent.starts_with('-') => format!("{mantissa}e{exponent}"),
        Some((mantissa, exponent)) => format!("{mantissa}e+{exponent}"),
        None => text,
    }
}

/// Executes `<conversation:content_for_detector>`.
fn content_for_detector_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, input, source] = attribute_values(
        attributes,
        ["location", "input", "source"],
        "conversation:content_for_detector takes only the location, input and source attributes",
        "conversation:content_for_detector repeats an attribute",
        "conversation:content_for_detector has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:content_for_detector requires a location",
    ))?;
    let input = input.ok_or(Failure::Execution(
        "conversation:content_for_detector requires an input",
    ))?;
    let source = source.ok_or(Failure::Execution(
        "conversation:content_for_detector requires a source",
    ))?;
    no_content(
        content,
        "conversation:content_for_detector holds no content",
    )?;
    let Json::Object(input) = read_json(
        host,
        input,
        [
            "conversation:content_for_detector input did not evaluate",
            "conversation:content_for_detector input is not an object",
        ],
    )?
    else {
        return Err(Failure::Execution(
            "conversation:content_for_detector input is not an object",
        ));
    };
    let source = read_source(
        host,
        source,
        [
            "conversation:content_for_detector source did not evaluate",
            "conversation:content_for_detector source is not an identity",
        ],
    )?
    .ok_or(Failure::Execution(
        "conversation:content_for_detector source is not an identity",
    ))?;
    let stimulus = content_for_detector(&input, source).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(stimulus).to_string()),
        "conversation:content_for_detector could not assign the stimulus",
    )?;
    Ok(Executed::Handled)
}

/// The stimulus that opens the
/// source's turn (see the crate documentation).
///
/// # Errors
///
/// A static detail when the input has no string `content_type`.
pub fn content_for_detector(
    input: &Map<String, Json>,
    source: Json,
) -> Result<Map<String, Json>, &'static str> {
    let Some(Json::String(content_type)) = input.get("content_type") else {
        return Err("conversation:content_for_detector input content_type is not a string");
    };
    let lowered = content_type.to_lowercase();
    let content = input.get("content");
    let mut stimulus = Map::new();
    stimulus.insert("source_participant_ref".into(), source);
    let text = matches!(content, Some(Json::String(_)));
    let kind = if lowered == "text/plain" && text {
        "text"
    } else if lowered.starts_with("audio/") && text {
        let or_null = |name: &str| input.get(name).cloned().unwrap_or(Json::Null);
        stimulus.insert("sample_rate_hz".into(), or_null("sample_rate_hz"));
        stimulus.insert("channels".into(), or_null("channels"));
        "audio"
    } else {
        stimulus.insert("content_type".into(), Json::String(content_type.clone()));
        "content"
    };
    stimulus.insert("kind".into(), Json::String(kind.into()));
    if let Some(content) = content {
        stimulus.insert("content".into(), content.clone());
    }
    Ok(stimulus)
}

/// Executes `<conversation:complete_turn>`.
fn complete_turn<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, event, input, turns, session_ref, participants] = attribute_values(
        attributes,
        [
            "location",
            "event",
            "input",
            "turns",
            "session_ref",
            "participants",
        ],
        "conversation:complete_turn takes only the location, event, input, turns, session_ref and participants attributes",
        "conversation:complete_turn repeats an attribute",
        "conversation:complete_turn has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:complete_turn requires a location",
    ))?;
    let event = event.ok_or(Failure::Execution(
        "conversation:complete_turn requires an event",
    ))?;
    let input = input.ok_or(Failure::Execution(
        "conversation:complete_turn requires an input",
    ))?;
    let turns = turns.ok_or(Failure::Execution(
        "conversation:complete_turn requires turns",
    ))?;
    let session_ref = session_ref.ok_or(Failure::Execution(
        "conversation:complete_turn requires a session_ref",
    ))?;
    no_content(content, "conversation:complete_turn holds no content")?;
    let event = read_event(
        host,
        event,
        [
            "conversation:complete_turn event did not evaluate",
            "conversation:complete_turn event is not a non-empty string",
        ],
    )?;
    let Json::Object(input) = read_json(
        host,
        input,
        [
            "conversation:complete_turn input did not evaluate",
            "conversation:complete_turn input is not an object",
        ],
    )?
    else {
        return Err(Failure::Execution(
            "conversation:complete_turn input is not an object",
        ));
    };
    let Json::Array(turns) = read_json(
        host,
        turns,
        [
            "conversation:complete_turn turns did not evaluate",
            "conversation:complete_turn turns is not an array of turns",
        ],
    )?
    else {
        return Err(Failure::Execution(
            "conversation:complete_turn turns is not an array of turns",
        ));
    };
    let Data::String(session_ref) = evaluate(
        host,
        session_ref,
        "conversation:complete_turn session_ref did not evaluate",
    )?
    else {
        return Err(Failure::Execution(
            "conversation:complete_turn session_ref is not a string",
        ));
    };
    let participants = match participants {
        None => Json::Null,
        Some(participants) => match evaluate(
            host,
            participants,
            "conversation:complete_turn participants did not evaluate",
        )? {
            Data::Null | Data::Undefined | Data::Absent => Json::Null,
            value => json_of(&value).ok_or(Failure::Execution(
                "conversation:complete_turn participants are not at most 32 {ref, role, own?, resembles?}",
            ))?,
        },
    };
    let (mut product, provenance) =
        turn_product(&input, &turns, &session_ref).map_err(Failure::Execution)?;
    product.insert("participants".into(), participants);
    let (message, _) = committed_message(Direction::Inbound, &event, &product, &provenance)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(message).to_string()),
        "conversation:complete_turn could not assign the message",
    )?;
    Ok(Executed::Handled)
}

/// A JSON object's members.
type Members = Map<String, Json>;

/// The inbound message the input and the turns make, and
/// its provenance (the first turn's `turn_ref`, the relationship's
/// `session_ref`).
///
/// # Errors
///
/// A static detail when there is no turn, a turn is not an object, or the
/// input has no string `content_type`.
pub fn turn_product(
    input: &Map<String, Json>,
    turns: &[Json],
    session_ref: &str,
) -> Result<(Members, Members), &'static str> {
    const MALFORMED: &str = "conversation:complete_turn turns is not an array of turns";
    let Some(Json::Object(first)) = turns.first() else {
        return Err(MALFORMED);
    };
    let mut texts: Vec<&str> = Vec::new();
    for turn in turns {
        let Json::Object(turn) = turn else {
            return Err(MALFORMED);
        };
        if let Some(Json::String(text)) = turn.get("text")
            && !text.is_empty()
            && !texts.contains(&text.as_str())
        {
            texts.push(text);
        }
    }
    let Some(Json::String(content_type)) = input.get("content_type") else {
        return Err("conversation:complete_turn input content_type is not a string");
    };
    let mut product = Map::new();
    for name in ["source_ids", "target_ids"] {
        product.insert(name.into(), input.get(name).cloned().unwrap_or(Json::Null));
    }
    if texts.is_empty() {
        let audio = content_type.to_lowercase().starts_with("audio/");
        let content = match input.get("content") {
            Some(content) if !audio => content.clone(),
            _ => Json::Null,
        };
        product.insert("content".into(), content);
        product.insert("content_type".into(), Json::String(content_type.clone()));
    } else {
        product.insert("content".into(), Json::String(texts.join(" ")));
        product.insert("content_type".into(), Json::String("text/plain".into()));
    }
    let mut provenance = Map::new();
    provenance.insert(
        "turn_ref".into(),
        first.get("turn_ref").cloned().unwrap_or(Json::Null),
    );
    provenance.insert("session_ref".into(), Json::String(session_ref.into()));
    Ok((product, provenance))
}

/// The open turn's content, as the turn detector keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnContent {
    /// The stripped texts, in order.
    pub text_parts: Vec<Json>,
    /// The audio chunks' references, in order.
    pub audio: Vec<Json>,
    /// The first audio chunk's rate.
    pub audio_sample_rate_hz: Json,
    /// The first audio chunk's channels.
    pub audio_channels: Json,
    /// The modality-neutral content.
    pub content: Json,
    /// Its content type.
    pub content_type: Json,
}

impl TurnContent {
    /// The cleared content.
    #[must_use]
    pub fn cleared() -> Self {
        Self {
            text_parts: Vec::new(),
            audio: Vec::new(),
            audio_sample_rate_hz: Json::Null,
            audio_channels: Json::Null,
            content: Json::Null,
            content_type: Json::Null,
        }
    }

    /// The content a value holds: an object's members (an absent one is
    /// cleared), anything else the cleared content.
    #[must_use]
    pub fn of(value: &Json) -> Self {
        let Json::Object(members) = value else {
            return Self::cleared();
        };
        let array = |name: &str| match members.get(name) {
            Some(Json::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        let member = |name: &str| members.get(name).cloned().unwrap_or(Json::Null);
        Self {
            text_parts: array("text_parts"),
            audio: array("audio"),
            audio_sample_rate_hz: member("audio_sample_rate_hz"),
            audio_channels: member("audio_channels"),
            content: member("content"),
            content_type: member("content_type"),
        }
    }

    /// The content as an object.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut members = Map::new();
        members.insert("text_parts".into(), Json::Array(self.text_parts.clone()));
        members.insert("audio".into(), Json::Array(self.audio.clone()));
        members.insert(
            "audio_sample_rate_hz".into(),
            self.audio_sample_rate_hz.clone(),
        );
        members.insert("audio_channels".into(), self.audio_channels.clone());
        members.insert("content".into(), self.content.clone());
        members.insert("content_type".into(), self.content_type.clone());
        Json::Object(members)
    }

    /// Takes in one stimulus (see the crate
    /// documentation, `<conversation:ingest_content>`).
    pub fn ingest(&mut self, stimulus: &Json) {
        let Json::Object(stimulus) = stimulus else {
            return;
        };
        let member = |name: &str| stimulus.get(name).cloned().unwrap_or(Json::Null);
        match stimulus.get("kind").and_then(Json::as_str) {
            Some("text") => {
                if let Some(Json::String(text)) = stimulus.get("content") {
                    let text = js_trim(text);
                    if !text.is_empty() {
                        self.text_parts.push(Json::String(text.into()));
                    }
                }
            }
            Some("audio") => {
                let chunk = member("content");
                if is_truthy(&chunk) {
                    self.audio.push(chunk);
                    if self.audio_sample_rate_hz.is_null() {
                        self.audio_sample_rate_hz = member("sample_rate_hz");
                    }
                    if self.audio_channels.is_null() {
                        self.audio_channels = member("channels");
                    }
                }
            }
            Some("content") => {
                self.content = member("content");
                self.content_type = member("content_type");
            }
            Some("event") => {
                let mut event = Map::new();
                event.insert("event".into(), member("event"));
                event.insert(
                    "payload".into(),
                    stimulus
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| Json::Object(Map::new())),
                );
                self.content = Json::Object(event);
                self.content_type = Json::String("application/event".into());
            }
            _ => {}
        }
    }

    /// The closed turn, `{turn}` or `{failure}`
    /// (see the crate documentation, `<conversation:on_end>`).
    #[must_use]
    pub fn closed(&self, refs: &TurnRefs) -> Json {
        let parts: Vec<&str> = self.text_parts.iter().filter_map(Json::as_str).collect();
        let text = js_trim(&parts.join(" ")).to_owned();
        let has_audio = !self.audio.is_empty();
        let failure = if text.is_empty() && !has_audio && self.content.is_null() {
            Some("Turn closed with neither text, audio, nor modality-neutral content.")
        } else if !text.is_empty() && has_audio {
            Some("Turn product cannot contain both text and audio.")
        } else {
            None
        };
        let mut closed = Map::new();
        if let Some(message) = failure {
            let mut failure = Map::new();
            failure.insert("stage".into(), Json::String("turn".into()));
            failure.insert("message".into(), Json::String(message.into()));
            failure.insert("participant_ref".into(), refs.participant_ref.clone());
            failure.insert("conversation_ref".into(), refs.conversation_ref.clone());
            failure.insert("turn_ref".into(), refs.turn_ref.clone());
            failure.insert(
                "source_participant_ref".into(),
                refs.source_participant_ref.clone(),
            );
            closed.insert("failure".into(), Json::Object(failure));
            return Json::Object(closed);
        }
        let (content, content_type) = if !text.is_empty() {
            (
                Json::String(text.clone()),
                Json::String("text/plain".into()),
            )
        } else if has_audio {
            (
                Json::Array(self.audio.clone()),
                Json::String("audio/pcm".into()),
            )
        } else {
            (self.content.clone(), self.content_type.clone())
        };
        let mut turn = Map::new();
        turn.insert("conversation_ref".into(), refs.conversation_ref.clone());
        turn.insert("turn_ref".into(), refs.turn_ref.clone());
        turn.insert("participant_ref".into(), refs.participant_ref.clone());
        turn.insert("self_participant_ref".into(), refs.participant_ref.clone());
        turn.insert(
            "source_participant_ref".into(),
            refs.source_participant_ref.clone(),
        );
        turn.insert("text".into(), Json::String(text));
        turn.insert("audio".into(), Json::Array(self.audio.clone()));
        turn.insert("content".into(), content);
        turn.insert("content_type".into(), content_type);
        let when_audio = |value: &Json| if has_audio { value.clone() } else { Json::Null };
        turn.insert(
            "sample_rate_hz".into(),
            when_audio(&self.audio_sample_rate_hz),
        );
        turn.insert("channels".into(), when_audio(&self.audio_channels));
        closed.insert("turn".into(), Json::Object(turn));
        Json::Object(closed)
    }
}

/// The open turn's references (the turn detector's instance state).
#[derive(Clone, Debug, PartialEq)]
pub struct TurnRefs {
    /// The detector's participant.
    pub participant_ref: Json,
    /// The open turn's conversation.
    pub conversation_ref: Json,
    /// The open turn.
    pub turn_ref: Json,
    /// The participant whose content the turn assembles.
    pub source_participant_ref: Json,
}

/// ECMAScript's truthiness of a JSON value.
fn is_truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(value) => *value,
        Json::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Json::String(text) => !text.is_empty(),
        Json::Array(_) | Json::Object(_) => true,
    }
}

/// ECMAScript's `String.prototype.trim`: white space and line terminators
/// removed from both ends.
fn js_trim(text: &str) -> &str {
    let space = |c: char| {
        matches!(
            c,
            '\u{9}'
                | '\u{a}'
                | '\u{b}'
                | '\u{c}'
                | '\u{d}'
                | ' '
                | '\u{a0}'
                | '\u{1680}'
                | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    };
    text.trim_matches(space)
}

/// Executes `<conversation:ingest_content>`.
fn ingest_content_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, stimulus] = attribute_values(
        attributes,
        ["location", "content"],
        "conversation:ingest_content takes only the location and content attributes",
        "conversation:ingest_content repeats an attribute",
        "conversation:ingest_content has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:ingest_content requires a location",
    ))?;
    let stimulus = stimulus.ok_or(Failure::Execution(
        "conversation:ingest_content requires a content",
    ))?;
    no_content(content, "conversation:ingest_content holds no content")?;
    let mut turn = match evaluate(
        host,
        location,
        "conversation:ingest_content location did not evaluate",
    )? {
        value @ Data::JsonObject(_) => TurnContent::of(&json_of(&value).unwrap_or(Json::Null)),
        _ => TurnContent::cleared(),
    };
    let stimulus = match evaluate(
        host,
        stimulus,
        "conversation:ingest_content content did not evaluate",
    )? {
        Data::Null | Data::Undefined | Data::Absent => Json::Null,
        value => match json_of(&value) {
            Some(stimulus @ Json::Object(_)) => stimulus,
            _ => {
                return Err(Failure::Execution(
                    "conversation:ingest_content content is not a stimulus or null",
                ));
            }
        },
    };
    turn.ingest(&stimulus);
    assign_to(
        host,
        location,
        &Data::JsonObject(turn.to_json().to_string()),
        "conversation:ingest_content could not assign the turn's content",
    )?;
    Ok(Executed::Handled)
}

/// Executes `<conversation:on_end>`.
fn on_end_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [
        location,
        turn,
        participant_ref,
        conversation_ref,
        turn_ref,
        source_participant_ref,
    ] = attribute_values(
        attributes,
        [
            "location",
            "turn",
            "participant_ref",
            "conversation_ref",
            "turn_ref",
            "source_participant_ref",
        ],
        "conversation:on_end takes only the location, turn, participant_ref, conversation_ref, turn_ref and source_participant_ref attributes",
        "conversation:on_end repeats an attribute",
        "conversation:on_end has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:on_end requires a location",
    ))?;
    let turn = turn.ok_or(Failure::Execution("conversation:on_end requires a turn"))?;
    let participant_ref = participant_ref.ok_or(Failure::Execution(
        "conversation:on_end requires a participant_ref",
    ))?;
    let conversation_ref = conversation_ref.ok_or(Failure::Execution(
        "conversation:on_end requires a conversation_ref",
    ))?;
    let turn_ref = turn_ref.ok_or(Failure::Execution(
        "conversation:on_end requires a turn_ref",
    ))?;
    let source_participant_ref = source_participant_ref.ok_or(Failure::Execution(
        "conversation:on_end requires a source_participant_ref",
    ))?;
    no_content(content, "conversation:on_end holds no content")?;
    let turn = match evaluate(host, turn, "conversation:on_end turn did not evaluate")? {
        value @ Data::JsonObject(_) => TurnContent::of(&json_of(&value).unwrap_or(Json::Null)),
        _ => TurnContent::cleared(),
    };
    let mut reference =
        |expression: &str, failed: &'static str| -> Result<Json, Failure<H::Error>> {
            Ok(match evaluate(host, expression, failed)? {
                Data::Undefined | Data::Absent => Json::Null,
                value => json_of(&value).unwrap_or(Json::Null),
            })
        };
    let refs = TurnRefs {
        participant_ref: reference(
            participant_ref,
            "conversation:on_end participant_ref did not evaluate",
        )?,
        conversation_ref: reference(
            conversation_ref,
            "conversation:on_end conversation_ref did not evaluate",
        )?,
        turn_ref: reference(turn_ref, "conversation:on_end turn_ref did not evaluate")?,
        source_participant_ref: reference(
            source_participant_ref,
            "conversation:on_end source_participant_ref did not evaluate",
        )?,
    };
    assign_to(
        host,
        location,
        &Data::JsonObject(turn.closed(&refs).to_string()),
        "conversation:on_end could not assign the closed turn",
    )?;
    Ok(Executed::Handled)
}

/// The most participants a payload carries.
pub const MAX_PARTICIPANTS: usize = 32;

/// The longest participant ref, in characters.
pub const MAX_PARTICIPANT_REF_CHARACTERS: usize = 200;

/// The most identities a participant resembles.
pub const MAX_RESEMBLANCES: usize = 3;

/// `<conversation:turn_participants>`'s detail for an input of the wrong
/// kind.
const MALFORMED_INPUT: &str =
    "conversation:turn_participants input is not an object with source_ids and target_ids";

/// Executes `<conversation:turn_participants>`.
fn turn_participants_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, input, parent] = attribute_values(
        attributes,
        ["location", "input", "parent"],
        "conversation:turn_participants takes only the location, input and parent attributes",
        "conversation:turn_participants repeats an attribute",
        "conversation:turn_participants has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:turn_participants requires a location",
    ))?;
    let input = input.ok_or(Failure::Execution(
        "conversation:turn_participants requires an input",
    ))?;
    no_content(content, "conversation:turn_participants holds no content")?;
    let Json::Object(input) = read_json(
        host,
        input,
        [
            "conversation:turn_participants input did not evaluate",
            MALFORMED_INPUT,
        ],
    )?
    else {
        return Err(Failure::Execution(MALFORMED_INPUT));
    };
    let sources =
        identities_of(&input, "source_ids", MALFORMED_INPUT).map_err(Failure::Execution)?;
    let targets =
        identities_of(&input, "target_ids", MALFORMED_INPUT).map_err(Failure::Execution)?;
    let carried = match parent {
        None => Vec::new(),
        Some(parent) => {
            match evaluate(
                host,
                parent,
                "conversation:turn_participants parent did not evaluate",
            )? {
                Data::Null | Data::Undefined | Data::Absent => Vec::new(),
                value => carried_participants(json_of(&value)).ok_or(Failure::Execution(
                    "conversation:turn_participants parent carries participants that are not {ref, role}",
                ))?,
            }
        }
    };
    let participants = turn_participants(sources, targets, &carried);
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(participants).to_string()),
        "conversation:turn_participants could not assign the participants",
    )?;
    Ok(Executed::Handled)
}

/// The participants an envelope's data carries: none when it has no data
/// or no `participants`; `None` when they are not each `{ref, role}`.
fn carried_participants(parent: Option<Json>) -> Option<Vec<Json>> {
    let parent = parent?;
    let Some(participants) = parent.get("data").and_then(|data| data.get("participants")) else {
        return parent.is_object().then(Vec::new);
    };
    participants
        .as_array()?
        .iter()
        .map(|participant| {
            let participant = participant.as_object()?;
            let reference = participant.get("ref")?.as_str()?;
            let role = participant.get("role")?.as_str()?;
            (participant.len() == 2 && matches!(role, "source" | "target"))
                .then(|| participant_of(reference, role))
        })
        .collect()
}

/// One participant, `{ref, role}`.
fn participant_of(reference: &str, role: &str) -> Json {
    let mut participant = Map::new();
    participant.insert("ref".to_owned(), Json::String(reference.to_owned()));
    participant.insert("role".to_owned(), Json::String(role.to_owned()));
    Json::Object(participant)
}

/// The participants a cognition handoff stamps for a turn (see the crate
/// documentation, `<conversation:turn_participants>`).
#[must_use]
pub fn turn_participants(sources: &[Json], targets: &[Json], carried: &[Json]) -> Vec<Json> {
    let stamped = sorted_identities(sources)
        .into_iter()
        .map(|identity| (identity, "source"))
        .chain(
            sorted_identities(targets)
                .into_iter()
                .map(|identity| (identity, "target")),
        )
        .filter_map(|(identity, role)| match identity {
            Json::String(reference)
                if !reference.is_empty()
                    && reference.chars().count() <= MAX_PARTICIPANT_REF_CHARACTERS =>
            {
                Some(participant_of(&reference, role))
            }
            _ => None,
        });
    let mut participants: Vec<Json> = Vec::new();
    for participant in stamped.chain(carried.iter().cloned()) {
        if !participants.contains(&participant) {
            participants.push(participant);
        }
    }
    participants.truncate(MAX_PARTICIPANTS);
    participants
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

    use super::{Attribute, Content, ConversationHost, Data, Executed, Failure};

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

    impl ConversationHost for Host {
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

mod people;

pub use people::{
    PERSON_REF_PREFIX, VOICE_PREFIX, decode_voice, encode_voice, fold_voices, merged_context_refs,
    people_for, people_to_merge, person_for_voice,
};

#[cfg(test)]
mod tests;
