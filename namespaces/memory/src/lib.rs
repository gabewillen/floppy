//! The `memory` extension namespace: `<memory:to_json>`,
//! `<memory:contents>`, `<memory:cut_at_word>`,
//! `<memory:stm_projection>` and `<memory:scored_candidates>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The JSON text a conversation memory row holds, the contents of one
//! statement's result of a memory select, each content cell parsed, the
//! two pure helpers of the short-term register: a turn's words bounded
//! and cut at a whole word, and a stimulus's bounded scalar projection,
//! and the associative recall scores of memory select's candidates. The
//! memory ability applies the statements through the sql
//! service; these elements only build and read what it stores, so they
//! are synchronous executable content. Documents bind [`NAMESPACE_URI`]
//! (conventionally with the prefix `memory`):
//!
//! ```xml
//! <memory:to_json location="memory_payload" message="input"/>
//! <memory:contents location="prior_episodes" result="_event.data.results[0]"/>
//! <memory:cut_at_word location="heard" words="transcript"/>
//! <memory:stm_projection location="stm_payload" data="_event.data"/>
//! <memory:scored_candidates location="scored" output="found" cue="cue" weights="weights"/>
//! ```
//!
//! Every attribute but `location` is an expression of the document's data
//! model. Each element writes to its location and never sends.
//!
//! # `<memory:to_json location="L" message="M"/>`
//!
//! The memory JSON of a conversation input. `message`
//! (required) evaluates to an object with `source_ids` and `target_ids`
//! (arrays of identities: a string, or an array of numbers),
//! `content_type` (a string) and `content` (any value; absent is `null`).
//! `location` (required) receives the JSON text
//! `{"content":C,"content_type":T,"source_ids":S,"target_ids":G}`, with
//! the keys sorted and compact separators:
//!
//! * `content`: arrays and objects tagged collision-safe: an array is
//!   `{"__stateforward_memory_content__":"list","value":[...]}`, an object
//!   `{"__stateforward_memory_content__":"mapping","value":[[key,value],...]}`
//!   with its entries ordered by their keys in code point order; numbers
//!   are JSON numbers as the data model hands them.
//! * `source_ids`, `target_ids`: the identities in identity order
//!   ([`sort_key`]) without repeats, each name a JSON string and each
//!   embedding its array of numbers.
//! * Strings are written as ECMAScript's `JSON.stringify` writes them
//!   (characters outside ASCII kept as they are).
//!
//! Nothing compares the stored text as bytes; a reader parses it.
//!
//! # `<memory:contents location="L" result="R"/>`
//!
//! The contents of one statement's result.
//! `result` (required) evaluates to one result of a
//! `bot.ability.memory.output`, an object whose `rows` is an array of rows
//! with `values`. `location` (required) receives, in row order, the first
//! value of each row that is a string holding a JSON object, parsed; any
//! other row is skipped.
//!
//! # `<memory:cut_at_word location="L" words="W"/>`
//!
//! A turn's words within a bound. `words` (required) evaluates to a string.
//! `location` (required) receives `{"text": T, "cut": B}`: words of at most
//! [`MAX_TURN_TEXT_BYTES`] UTF-8 bytes are `T` unchanged and `B` false;
//! longer words are cut, `B` true: `T` is their longest prefix of whole
//! characters within the bound, less the partial word it ends in when the
//! bound falls inside a word (neither its last character nor the next one
//! is white space) and an earlier word break exists, with trailing white
//! space removed. White space is Unicode `White_Space`.
//!
//! # `<memory:stm_projection location="L" data="D"/>`
//!
//! A stimulus's bounded scalar projection. `data` (required) evaluates to
//! any value. `location` (required) receives an object: for an object,
//! its fields in their order, keeping each whose key is not empty and
//! whose value is a boolean, a number, or a string of at most
//! [`MAX_PROJECTION_TEXT_LENGTH`] characters (code points), until
//! [`MAX_PROJECTION_FIELDS`] are kept; every other field (arrays, objects,
//! null, longer strings) is dropped, never truncated. A bare boolean,
//! number or bounded string is `{"value": D}`; anything else is `{}`.
//!
//! # `<memory:scored_candidates location="L" output="O" cue="C" weights="W"/>`
//!
//! The associative recall scores of one candidates output (decision D256,
//! mosfet.py's Associative). `output` (required) evaluates to the
//! `bot.ability.memory.output` whose first statement's rows are the
//! candidates, each row's values the id, the `created_at` text, the
//! content length, and the stored vector (the `i8:` compact text
//! [`floppy_guest::quantized::decode`] reads, or `null` for an episode
//! without one). `cue` (required) evaluates to the cue's vector, an array
//! of finite numbers. `weights` (required) evaluates to the scores'
//! weights, an object with the numbers `similarity`, `recency` and
//! `recency_half_life_ms` (positive). `location` (required) receives the
//! scored candidates as a JSON array, best first, ties in their row
//! order, each `{"id":I,"length":N,"order":K,"similarity":S,"score":T}`:
//! `S` is the cosine of the cue with the stored vector, `0` for an
//! episode without one, one of another length than the cue, or a zero
//! vector, and `T` is `weights.similarity * S + weights.recency *
//! 0.5^((newest - age) / weights.recency_half_life_ms)`, the age the
//! candidate's `created_at` gives (its milliseconds, UTC; a `created_at`
//! of another form or a missing cell is 0) before the newest candidate's.
//! Candidates past [`MAX_CANDIDATES`] raise `error.execution` instead.
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
//! The data model bounds the values it hands this component, and
//! `scored_candidates` refuses more candidates than [`MAX_CANDIDATES`]
//! (wasm builds).
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one
//! closed signal event. No attribute carries content, an identity, a row,
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

use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "memory";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/memory";

/// The tag of a collision-safe encoded array or object.
pub const CONTENT_TAG: &str = "__stateforward_memory_content__";

/// Most UTF-8 bytes of one turn's text; a longer turn is cut at a whole
/// word.
pub const MAX_TURN_TEXT_BYTES: usize = 2048;

/// Longest string, in characters, a stimulus projection keeps; a longer
/// one is dropped.
pub const MAX_PROJECTION_TEXT_LENGTH: usize = 40;

/// Most fields one stimulus projection keeps.
pub const MAX_PROJECTION_FIELDS: usize = 16;

/// Most candidates one `scored_candidates` output may score (wasm builds;
/// the memory select asks for 64, so this only refuses a runaway input).
pub const MAX_CANDIDATES: usize = wasm_bound(4096);

/// The largest integer ECMAScript calls safe (`Number.MAX_SAFE_INTEGER`).
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub use floppy_guest::element::{Attribute, Content, Executed, Failure, Host};
use floppy_guest::element::{assign_to, attribute_values, evaluate, no_content, number};

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
        "to_json" => to_json_element(attributes, content, host),
        "contents" => contents_element(attributes, content, host),
        "cut_at_word" => cut_at_word_element(attributes, content, host),
        "stm_projection" => stm_projection_element(attributes, content, host),
        "scored_candidates" => scored_candidates_element(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// Evaluates `expression` to a JSON object, else `malformed`.
fn read_object<H: Host>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
    malformed: &'static str,
) -> Result<Map<String, Json>, Failure<H::Error>> {
    match evaluate(host, expression, failed)? {
        Data::JsonObject(text) => match serde_json::from_str(&text) {
            Ok(Json::Object(object)) => Ok(object),
            _ => Err(Failure::Execution(malformed)),
        },
        _ => Err(Failure::Execution(malformed)),
    }
}

/// Executes `<memory:to_json>`.
fn to_json_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, message] = attribute_values(
        attributes,
        ["location", "message"],
        [
            "memory:to_json takes only the location and message attributes",
            "memory:to_json repeats an attribute",
            "memory:to_json has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution("memory:to_json requires a location"))?;
    let message = message.ok_or(Failure::Execution("memory:to_json requires a message"))?;
    no_content(content, "memory:to_json holds no content")?;
    let message = read_object(
        host,
        message,
        "memory:to_json message did not evaluate",
        "memory:to_json message is not an object",
    )?;
    let text = to_json(&message).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::String(text),
        "memory:to_json could not assign the memory JSON",
    )?;
    Ok(Executed::Handled)
}

/// The memory JSON of a conversation input (see the crate documentation,
/// `<memory:to_json>`).
///
/// # Errors
///
/// A static detail when the identities or the content type are malformed.
pub fn to_json(message: &Map<String, Json>) -> Result<String, &'static str> {
    let identities = |name: &str, malformed: &'static str| match message.get(name) {
        Some(Json::Array(ids)) if ids.iter().all(is_identity) => Ok(identities_json(ids)),
        _ => Err(malformed),
    };
    let source_ids = identities(
        "source_ids",
        "memory:to_json message source_ids is not an array of identities",
    )?;
    let target_ids = identities(
        "target_ids",
        "memory:to_json message target_ids is not an array of identities",
    )?;
    let content_type = match message.get("content_type") {
        Some(Json::String(content_type)) => js_string(content_type),
        _ => return Err("memory:to_json message content_type is not a string"),
    };
    let content = encode_content(message.get("content").unwrap_or(&Json::Null));
    Ok(format!(
        "{{\"content\":{content},\"content_type\":{content_type},\"source_ids\":{source_ids},\"target_ids\":{target_ids}}}"
    ))
}

/// A content with its arrays and objects tagged collision-safe, an
/// object's entries ordered by their keys in code point order. Every
/// object it builds holds the tag and then `value`, so its JSON text has
/// sorted keys.
fn encode_content(value: &Json) -> Json {
    let tagged = |kind: &str, value: Json| {
        let mut object = Map::new();
        object.insert(CONTENT_TAG.into(), Json::String(kind.into()));
        object.insert("value".into(), value);
        Json::Object(object)
    };
    match value {
        Json::Array(items) => tagged(
            "list",
            Json::Array(items.iter().map(encode_content).collect()),
        ),
        Json::Object(members) => {
            let mut entries: Vec<(&String, &Json)> = members.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            tagged(
                "mapping",
                Json::Array(
                    entries
                        .into_iter()
                        .map(|(key, value)| {
                            Json::Array(vec![Json::String(key.clone()), encode_content(value)])
                        })
                        .collect(),
                ),
            )
        }
        other => other.clone(),
    }
}

/// The identities in identity order without repeats ([`sort_key`]), as
/// one JSON text.
#[must_use]
pub fn identities_json(ids: &[Json]) -> String {
    let mut keyed: Vec<(String, &Json)> = ids.iter().map(|id| (sort_key(id), id)).collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.dedup_by(|right, left| left.0 == right.0);
    Json::Array(keyed.into_iter().map(|(_, id)| id.clone()).collect()).to_string()
}

/// An identity: a string, or an array of numbers (an embedding).
fn is_identity(value: &Json) -> bool {
    match value {
        Json::String(_) => true,
        Json::Array(values) => values.iter().all(Json::is_number),
        _ => false,
    }
}

/// An identity's sort key, compared in code point order: `0` and a name's
/// text, or `1` and an embedding's JSON text (so every name sorts before
/// every embedding).
#[must_use]
pub fn sort_key(id: &Json) -> String {
    match id {
        Json::String(name) => format!("0{name}"),
        other @ Json::Array(_) => format!("1{other}"),
        other => format!("2{other}"),
    }
}

/// A string as ECMAScript's `JSON.stringify` writes it: quote, backslash
/// and `\b \f \n \r \t` escaped short, the other control characters as
/// lower-case `\u00xx`, everything else as it is.
#[must_use]
pub fn js_string(text: &str) -> String {
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
                push_unit(&mut out, u16::try_from(u32::from(character)).unwrap_or(0));
            }
            _ => out.push(character),
        }
    }
    out.push('"');
    out
}

/// Pushes `\uxxxx` for one UTF-16 code unit, lower-case.
fn push_unit(out: &mut String, unit: u16) {
    out.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        let digit = u32::from((unit >> shift) & 0xf);
        out.push(char::from_digit(digit, 16).unwrap_or('0'));
    }
}

/// Executes `<memory:contents>`.
fn contents_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, result] = attribute_values(
        attributes,
        ["location", "result"],
        [
            "memory:contents takes only the location and result attributes",
            "memory:contents repeats an attribute",
            "memory:contents has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution("memory:contents requires a location"))?;
    let result = result.ok_or(Failure::Execution("memory:contents requires a result"))?;
    no_content(content, "memory:contents holds no content")?;
    let result = read_object(
        host,
        result,
        "memory:contents result did not evaluate",
        "memory:contents result is not an object",
    )?;
    let contents = contents(&result).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(contents).to_string()),
        "memory:contents could not assign the contents",
    )?;
    Ok(Executed::Handled)
}

/// The contents of one statement's result: the
/// first value of each row that is a string holding a JSON object, parsed,
/// in row order; other rows are skipped.
///
/// # Errors
///
/// A static detail when the result has no rows array, or a row no values
/// array.
pub fn contents(result: &Map<String, Json>) -> Result<Vec<Json>, &'static str> {
    let Some(Json::Array(rows)) = result.get("rows") else {
        return Err("memory:contents result has no rows array");
    };
    let mut contents = Vec::new();
    for row in rows {
        let Some(Json::Array(values)) = row.get("values") else {
            return Err("memory:contents result has a row without a values array");
        };
        if let Some(Json::String(text)) = values.first()
            && let Ok(content @ Json::Object(_)) = serde_json::from_str::<Json>(text)
        {
            contents.push(content);
        }
    }
    Ok(contents)
}

/// Executes `<memory:cut_at_word>`.
fn cut_at_word_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, words] = attribute_values(
        attributes,
        ["location", "words"],
        [
            "memory:cut_at_word takes only the location and words attributes",
            "memory:cut_at_word repeats an attribute",
            "memory:cut_at_word has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution("memory:cut_at_word requires a location"))?;
    let words = words.ok_or(Failure::Execution("memory:cut_at_word requires words"))?;
    no_content(content, "memory:cut_at_word holds no content")?;
    let Data::String(words) = evaluate(host, words, "memory:cut_at_word words did not evaluate")?
    else {
        return Err(Failure::Execution(
            "memory:cut_at_word words are not a string",
        ));
    };
    let (text, cut) = cut_at_word(&words);
    let mut turn = Map::new();
    turn.insert("text".into(), Json::String(text));
    turn.insert("cut".into(), Json::Bool(cut));
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(turn).to_string()),
        "memory:cut_at_word could not assign the cut words",
    )?;
    Ok(Executed::Handled)
}

/// `words` within [`MAX_TURN_TEXT_BYTES`],
/// cut at the last whole word, and whether it was cut (see the crate
/// documentation, `<memory:cut_at_word>`).
#[must_use]
pub fn cut_at_word(words: &str) -> (String, bool) {
    if words.len() <= MAX_TURN_TEXT_BYTES {
        return (words.to_owned(), false);
    }
    // The whole characters within the bound: a character the bound splits
    // is dropped.
    let end = (0..=MAX_TURN_TEXT_BYTES)
        .rev()
        .find(|index| words.is_char_boundary(*index))
        .unwrap_or(0);
    let (mut head, tail) = words.split_at(end);
    let ends_in_space = head.chars().next_back().is_some_and(char::is_whitespace);
    let next_is_space = tail.chars().next().is_some_and(char::is_whitespace);
    if !(ends_in_space || next_is_space) {
        // The bound fell inside a word: drop that partial word.
        if let Some(rest) = before_last_word(head) {
            head = rest;
        }
    }
    (head.trim_end_matches(char::is_whitespace).to_owned(), true)
}

/// `text` without its trailing white space, its last word, and the white
/// space before that word; `None` when it holds at most one word.
fn before_last_word(text: &str) -> Option<&str> {
    let trimmed = text.trim_end_matches(char::is_whitespace);
    let (index, _) = trimmed
        .char_indices()
        .rev()
        .find(|(_, character)| char::is_whitespace(*character))?;
    let rest = trimmed
        .get(..index)
        .unwrap_or_default()
        .trim_end_matches(char::is_whitespace);
    (!rest.is_empty()).then_some(rest)
}

/// Executes `<memory:stm_projection>`.
fn stm_projection_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, data] = attribute_values(
        attributes,
        ["location", "data"],
        [
            "memory:stm_projection takes only the location and data attributes",
            "memory:stm_projection repeats an attribute",
            "memory:stm_projection has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "memory:stm_projection requires a location",
    ))?;
    let data = data.ok_or(Failure::Execution("memory:stm_projection requires data"))?;
    no_content(content, "memory:stm_projection holds no content")?;
    let value = evaluate(host, data, "memory:stm_projection data did not evaluate")?;
    let projection = stm_projection(&value).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(projection).to_string()),
        "memory:stm_projection could not assign the projection",
    )?;
    Ok(Executed::Handled)
}

/// An object's fields in their order, a repeated key keeping its first
/// place and its last value.
struct OrderedFields(Vec<(String, Json)>);

impl<'de> serde::Deserialize<'de> for OrderedFields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = OrderedFields;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<OrderedFields, A::Error> {
                let mut fields: Vec<(String, Json)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Json>()? {
                    match fields.iter_mut().find(|(known, _)| *known == key) {
                        Some(field) => field.1 = value,
                        None => fields.push((key, value)),
                    }
                }
                Ok(OrderedFields(fields))
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

/// A projected scalar: a boolean, a number, or a
/// string of at most [`MAX_PROJECTION_TEXT_LENGTH`] characters; anything
/// else is dropped.
fn scalar_projection_value(value: &Json) -> Option<Json> {
    match value {
        Json::Bool(_) | Json::Number(_) => Some(value.clone()),
        Json::String(text) if text.chars().count() <= MAX_PROJECTION_TEXT_LENGTH => {
            Some(value.clone())
        }
        _ => None,
    }
}

/// The stimulus projection of one value (see the crate
/// documentation, `<memory:stm_projection>`).
///
/// # Errors
///
/// A static detail when an object's JSON text does not parse.
pub fn stm_projection(data: &Data) -> Result<Map<String, Json>, &'static str> {
    let scalar = match data {
        Data::JsonObject(text) => {
            let OrderedFields(fields) = serde_json::from_str(text)
                .map_err(|_| "memory:stm_projection data is not a JSON object")?;
            let mut projection = Map::new();
            for (key, value) in fields {
                if projection.len() >= MAX_PROJECTION_FIELDS {
                    break;
                }
                if key.is_empty() {
                    continue;
                }
                if let Some(projected) = scalar_projection_value(&value) {
                    projection.insert(key, projected);
                }
            }
            return Ok(projection);
        }
        Data::Bool(value) => Some(Json::Bool(*value)),
        Data::Number(value) => Some(finite_number(value.get())),
        Data::String(text) => scalar_projection_value(&Json::String(text.clone())),
        Data::Absent | Data::Undefined | Data::Null | Data::JsonArray(_) | Data::Xml(_) => None,
    };
    let mut projection = Map::new();
    if let Some(value) = scalar {
        projection.insert("value".into(), value);
    }
    Ok(projection)
}

/// A finite number as JSON: a safe integer as an integer, else a float.
fn finite_number(value: f64) -> Json {
    if value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a safe integer converts exactly"
        )]
        let integer = value as i64;
        Json::from(integer)
    } else {
        serde_json::Number::from_f64(value).map_or(Json::Null, Json::Number)
    }
}

/// Executes `<memory:scored_candidates>`.
fn scored_candidates_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, output, cue, weights] = attribute_values(
        attributes,
        ["location", "output", "cue", "weights"],
        [
            "memory:scored_candidates takes only the location, output, cue and weights attributes",
            "memory:scored_candidates repeats an attribute",
            "memory:scored_candidates has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution(
        "memory:scored_candidates requires a location",
    ))?;
    let output = output.ok_or(Failure::Execution(
        "memory:scored_candidates requires an output",
    ))?;
    let cue = cue.ok_or(Failure::Execution(
        "memory:scored_candidates requires a cue",
    ))?;
    let weights = weights.ok_or(Failure::Execution(
        "memory:scored_candidates requires weights",
    ))?;
    no_content(content, "memory:scored_candidates holds no content")?;
    let output = read_object(
        host,
        output,
        "memory:scored_candidates output did not evaluate",
        "memory:scored_candidates output is not an object",
    )?;
    let cue = read_vector(host, cue, "memory:scored_candidates cue did not evaluate")?;
    let weights = read_object(
        host,
        weights,
        "memory:scored_candidates weights did not evaluate",
        "memory:scored_candidates weights are not an object",
    )?;
    let weights = RecallWeights::of(&weights).map_err(Failure::Execution)?;
    let scored = scored_candidates(&output, &cue, &weights).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonArray(Json::Array(scored).to_string()),
        "memory:scored_candidates could not assign the scores",
    )?;
    Ok(Executed::Handled)
}

/// The cue's vector: an array of finite numbers, else `error.execution`.
fn read_vector<H: Host>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Vec<f64>, Failure<H::Error>> {
    const MALFORMED: &str = "memory:scored_candidates cue is not an array of finite numbers";
    let Data::JsonArray(text) = evaluate(host, expression, failed)? else {
        return Err(Failure::Execution(MALFORMED));
    };
    let Json::Array(values) =
        serde_json::from_str(&text).map_err(|_| Failure::Execution(MALFORMED))?
    else {
        return Err(Failure::Execution(MALFORMED));
    };
    values
        .into_iter()
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or(Failure::Execution(MALFORMED))
        })
        .collect()
}

/// The weights of the associative recall scores (decision D256,
/// mosfet.py's Weights): how much a candidate's cosine and its recency
/// each count, and the recency's half-life.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecallWeights {
    /// How much a candidate's cosine with the cue counts.
    pub similarity: f64,
    /// How much a candidate's recency counts.
    pub recency: f64,
    /// The half-life of the recency, in milliseconds.
    pub recency_half_life_ms: f64,
}

impl RecallWeights {
    /// The weights of a weights object: `{similarity, recency,
    /// recency_half_life_ms}`, each a finite number, the half-life positive.
    ///
    /// # Errors
    ///
    /// A static detail when a weight is missing, is not a finite number, or
    /// the half-life is not positive.
    pub fn of(weights: &Map<String, Json>) -> Result<Self, &'static str> {
        const MALFORMED: &str =
            "memory:scored_candidates weights are not numbers with a positive half-life";
        let number = |name: &str| {
            weights
                .get(name)
                .and_then(Json::as_f64)
                .filter(|value| value.is_finite())
        };
        let weights = Self {
            similarity: number("similarity").ok_or(MALFORMED)?,
            recency: number("recency").ok_or(MALFORMED)?,
            recency_half_life_ms: number("recency_half_life_ms").ok_or(MALFORMED)?,
        };
        (weights.recency_half_life_ms > 0.0)
            .then_some(weights)
            .ok_or(MALFORMED)
    }
}

/// The candidates of one candidates output scored (see the crate
/// documentation, `<memory:scored_candidates>`): `{id, length, order,
/// similarity, score}` each, best first, ties in their row order.
///
/// # Errors
///
/// A static detail when the output has no first result, no rows array, a
/// row without a values array, or more candidates than
/// [`MAX_CANDIDATES`].
pub fn scored_candidates(
    output: &Map<String, Json>,
    cue: &[f64],
    weights: &RecallWeights,
) -> Result<Vec<Json>, &'static str> {
    const NO_RESULTS: &str = "memory:scored_candidates output has no first result";
    const NO_VALUES: &str = "memory:scored_candidates output has a row without a values array";
    const TOO_MANY: &str = "memory:scored_candidates output has too many candidates";
    let rows = output
        .get("results")
        .and_then(Json::as_array)
        .and_then(|results| results.first())
        .and_then(Json::as_object)
        .and_then(|result| result.get("rows"))
        .and_then(Json::as_array)
        .ok_or(NO_RESULTS)?;
    if exceeds_wasm_bound(rows.len(), MAX_CANDIDATES) {
        return Err(TOO_MANY);
    }
    let rows: Vec<Candidate> = rows
        .iter()
        .map(|row| {
            let values = row
                .get("values")
                .and_then(Json::as_array)
                .ok_or(NO_VALUES)?;
            Ok(Candidate {
                id: values.first().cloned().unwrap_or(Json::Null),
                created_at_ms: created_at_ms(values.get(1)),
                length: as_number(values.get(2)),
                similarity: values
                    .get(3)
                    .and_then(Json::as_str)
                    .and_then(floppy_guest::quantized::decode)
                    .map_or(0.0, |stored| similarity(cue, &stored)),
            })
        })
        .collect::<Result<Vec<Candidate>, &'static str>>()?;
    let newest = rows.iter().map(|row| row.created_at_ms).fold(0.0, f64::max);
    let mut scored: Vec<(f64, usize, Candidate)> = rows
        .into_iter()
        .zip(0..)
        .map(|(row, order)| {
            let score = score(&row, newest, weights);
            (score, order, row)
        })
        .collect();
    scored.sort_by(
        |(left_score, left_order, _), (right_score, right_order, _)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left_order.cmp(right_order))
        },
    );
    Ok(scored
        .into_iter()
        .enumerate()
        .map(|(order, (score, _, row))| {
            let mut candidate = Map::with_capacity(5);
            candidate.insert("id".into(), row.id);
            candidate.insert("length".into(), number(row.length));
            candidate.insert("order".into(), Json::from(order));
            candidate.insert("similarity".into(), number(row.similarity));
            candidate.insert("score".into(), number(score));
            Json::Object(candidate)
        })
        .collect())
}

/// One candidate's score (see the crate documentation,
/// `<memory:scored_candidates>`): its similarity times the similarity
/// weight, plus its recency times the recency weight.
fn score(row: &Candidate, newest: f64, weights: &RecallWeights) -> f64 {
    let recency = 0.5_f64.powf((newest - row.created_at_ms) / weights.recency_half_life_ms);
    weights.similarity * row.similarity + weights.recency * recency
}

/// One candidate of a candidates output, before its score.
struct Candidate {
    /// Its `memory_id`, as the row holds it.
    id: Json,
    /// Its `created_at`'s milliseconds (`0` for text of another form).
    created_at_ms: f64,
    /// Its content length, as ECMAScript's `Number` reads the cell.
    length: f64,
    /// Its cosine with the cue (`0` without a vector, one of another
    /// length, or a zero vector).
    similarity: f64,
}

/// A cell as ECMAScript's `Number` reads it: `null` is 0, a boolean its 0/1,
/// a string its leading number, a missing cell `NaN`.
fn as_number(value: Option<&Json>) -> f64 {
    match value {
        Some(Json::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Json::String(text)) => text.parse().unwrap_or(f64::NAN),
        Some(Json::Bool(true)) => 1.0,
        Some(Json::Bool(false) | Json::Null) => 0.0,
        None | Some(Json::Array(_) | Json::Object(_)) => f64::NAN,
    }
}

/// The cosine of the cue with a stored vector, 0 for another space or a
/// zero vector (the cue is the unit vector the Embedding ability answers).
fn similarity(cue: &[f64], stored: &[f64]) -> f64 {
    if stored.len() != cue.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm = 0.0;
    for (stored, cue) in stored.iter().zip(cue) {
        dot += stored * cue;
        norm += stored * stored;
    }
    if norm > 0.0 { dot / norm.sqrt() } else { 0.0 }
}

/// A `created_at`'s milliseconds (SQLite's and Postgres's text, UTC:
/// `YYYY-MM-DD[ T]HH:MM:SS` with an optional fraction), `0` for text of
/// another form.
fn created_at_ms(value: Option<&Json>) -> f64 {
    let Some(Json::String(text)) = value else {
        return 0.0;
    };
    parse_created_at(text).unwrap_or(0.0)
}

/// The milliseconds of `YYYY-MM-DD[ T]HH:MM:SS` with its optional fraction
/// of seconds (SQLite's and Postgres's UTC text), `None` for another form.
fn parse_created_at(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let digit = |index: usize| -> Option<u32> {
        let byte = *bytes.get(index)?;
        byte.is_ascii_digit().then_some(u32::from(byte - b'0'))
    };
    let pair = |index: usize| -> Option<u32> { Some(digit(index)? * 10 + digit(index + 1)?) };
    let year = i64::from(digit(0)?) * 1000
        + i64::from(digit(1)?) * 100
        + i64::from(digit(2)?) * 10
        + i64::from(digit(3)?);
    let months = pair(5)?;
    let days = pair(8)?;
    let hours = pair(11)?;
    let minutes = pair(14)?;
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b' ' | b'T'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    // The seconds with their optional fraction, as `parseFloat` reads the
    // captured run: two digits, then `.` and its digits.
    let seconds = match (bytes.get(19), bytes.get(20)) {
        (Some(b'.'), Some(byte)) if byte.is_ascii_digit() => {
            let end = bytes[20..]
                .iter()
                .position(|byte| !byte.is_ascii_digit())
                .map_or(text.len(), |rest| 20 + rest);
            text[17..end].parse().ok()?
        }
        _ => f64::from(pair(17)?),
    };
    let days = days_from_civil(year, months, days);
    #[allow(
        clippy::cast_precision_loss,
        reason = "a whole second count since 1970 (about 1.8e9) is exact in f64"
    )]
    let base_ms = (days * 86_400 + i64::from(hours) * 3_600 + i64::from(minutes) * 60) as f64;
    Some(base_ms * 1000.0 + seconds * 1000.0)
}

/// The days since 1970-01-01 of a UTC civil date (Hinnant's algorithm).
fn days_from_civil(year: i64, months: u32, days: u32) -> i64 {
    let year = year - i64::from(months <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_pattern = (months + 9) % 12;
    let day_of_year = i64::from(153 * month_pattern + 2) / 5 + i64::from(days) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

floppy_guest::extension_guest!();

#[cfg(test)]
mod tests;
