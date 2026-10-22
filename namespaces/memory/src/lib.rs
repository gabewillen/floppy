//! The `memory` extension namespace: `<memory:to_json>`,
//! `<memory:contents>`, `<memory:cut_at_word>` and
//! `<memory:stm_projection>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The original's memory module (`abilities/memory`): the JSON text a
//! conversation memory row holds (`Memory.to_json`, with `_encode_content`
//! and `value.identities_json`), the contents of one statement's
//! result of a memory select (`OutputData.contents`), each content cell
//! parsed as its caller does, and the two pure helpers of the short-term
//! register (`stm.py`): a turn's words bounded and cut at a whole word
//! (`_cut_at_word`, what `TurnRecordData.of` does), and a stimulus's
//! bounded scalar projection (`_payload_projection`, what `event_record`
//! records). The memory ability applies the statements through the sql
//! service; these elements only build and read what it stores, so they
//! are synchronous executable content. Documents bind [`NAMESPACE_URI`]
//! (conventionally with the prefix `memory`):
//!
//! ```xml
//! <memory:to_json location="memory_payload" message="input"/>
//! <memory:contents location="prior_episodes" result="_event.data.results[0]"/>
//! <memory:cut_at_word location="heard" words="transcript"/>
//! <memory:stm_projection location="stm_payload" data="_event.data"/>
//! ```
//!
//! Every attribute but `location` is an expression of the document's data
//! model. Each element writes to its location and never sends.
//!
//! # `<memory:to_json location="L" message="M"/>`
//!
//! The original's `Memory.to_json` of a conversation input. `message`
//! (required) evaluates to an object with `source_ids` and `target_ids`
//! (arrays of identities: a string, or an array of numbers),
//! `content_type` (a string) and `content` (any value; absent is `null`).
//! `location` (required) receives the JSON text
//! `{"content":C,"content_type":T,"source_ids":S,"target_ids":G}`, with
//! the keys sorted and compact separators:
//!
//! * `content`: arrays and objects tagged collision-safe (the original's
//!   `_encode_content`): an array is `{"__stateforward_memory_content__":
//!   "list","value":[...]}`, an object `{"__stateforward_memory_content__":
//!   "mapping","value":[[key,value],...]}` with its entries ordered by
//!   their keys' ASCII JSON text; a number that is not a safe integer
//!   (within 2^53 - 1) is written as Python's `repr` writes a float, a
//!   safe integer as an integer.
//! * `source_ids`, `target_ids`: the original's `identities_json`: the
//!   identities in the original's order without repeats, each name as a
//!   JSON string and each embedding as a list of Python floats.
//! * Strings are written as ECMAScript's `JSON.stringify` writes them
//!   (characters outside ASCII kept as they are).
//!
//! # `<memory:contents location="L" result="R"/>`
//!
//! The original's `OutputData.contents` of one statement's result.
//! `result` (required) evaluates to one result of a
//! `bot.ability.memory.output`, an object whose `rows` is an array of rows
//! with `values`. `location` (required) receives, in row order, the first
//! value of each row that is a string holding a JSON object, parsed; any
//! other row is skipped, as the original's readers skip a content that
//! does not validate.
//!
//! # `<memory:cut_at_word location="L" words="W"/>`
//!
//! The original's `_cut_at_word`. `words` (required) evaluates to a string.
//! `location` (required) receives `{"text": T, "cut": B}`: words of at most
//! [`MAX_TURN_TEXT_BYTES`] UTF-8 bytes are `T` unchanged and `B` false;
//! longer words are cut, `B` true: `T` is their longest prefix of whole
//! characters within the bound, less the partial word it ends in when the
//! bound falls inside a word (neither its last character nor the next one
//! is white space) and an earlier word break exists, with trailing white
//! space removed. White space is Python's `str.isspace` (Unicode
//! `White_Space`, and U+001C to U+001F).
//!
//! # `<memory:stm_projection location="L" data="D"/>`
//!
//! The original's `_payload_projection`. `data` (required) evaluates to
//! any value. `location` (required) receives an object: for an object,
//! its fields in their order, keeping each whose key is not empty and
//! whose value is a boolean, a number, or a string of at most
//! [`MAX_PROJECTION_TEXT_LENGTH`] characters (code points), until
//! [`MAX_PROJECTION_FIELDS`] are kept; every other field (arrays, objects,
//! null, longer strings) is dropped, never truncated. A bare boolean,
//! number or bounded string is `{"value": D}`; anything else is `{}`.
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

use scxml_types::DataValue as Data;
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "memory";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/memory";

/// The tag of a collision-safe encoded array or object (the original's
/// `_encode_content`).
pub const CONTENT_TAG: &str = "__stateforward_memory_content__";

/// Most UTF-8 bytes of one turn's text (the original's
/// `MAX_TURN_TEXT_BYTES`); a longer turn is cut at a whole word.
pub const MAX_TURN_TEXT_BYTES: usize = 2048;

/// Longest string, in characters, a stimulus projection keeps (the
/// original's `MAX_PROJECTION_TEXT_LENGTH`); a longer one is dropped.
pub const MAX_PROJECTION_TEXT_LENGTH: usize = 40;

/// Most fields one stimulus projection keeps (the original's
/// `_MAX_PROJECTION_FIELDS`).
pub const MAX_PROJECTION_FIELDS: usize = 16;

/// The largest integer ECMAScript calls safe (`Number.MAX_SAFE_INTEGER`).
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

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
pub trait MemoryHost {
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
pub fn execute<H: MemoryHost>(
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
fn evaluate<H: MemoryHost>(
    host: &mut H,
    expression: &str,
    failed: &'static str,
) -> Result<Data, Failure<H::Error>> {
    host.read(expression)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Assigns `value` to `location`, else `failed`.
fn assign_to<H: MemoryHost>(
    host: &mut H,
    location: &str,
    value: &Data,
    failed: &'static str,
) -> Result<(), Failure<H::Error>> {
    host.assign(location, value)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution(failed))
}

/// Evaluates `expression` to a JSON object, else `malformed`.
fn read_object<H: MemoryHost>(
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
fn to_json_element<H: MemoryHost>(
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

/// The original's `Memory.to_json` of a conversation input (see the crate
/// documentation, `<memory:to_json>`).
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
    let content = python_json(&encode_content(
        message.get("content").unwrap_or(&Json::Null),
    ));
    Ok(format!(
        "{{\"content\":{content},\"content_type\":{content_type},\"source_ids\":{source_ids},\"target_ids\":{target_ids}}}"
    ))
}

/// The original's `_encode_content`: arrays and objects tagged
/// collision-safe, an object's entries ordered by their keys' ASCII JSON
/// text.
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
            let mut entries: Vec<(String, &String, &Json)> = members
                .iter()
                .map(|(key, value)| (ascii_json(key), key, value))
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            tagged(
                "mapping",
                Json::Array(
                    entries
                        .into_iter()
                        .map(|(_, key, value)| {
                            Json::Array(vec![Json::String(key.clone()), encode_content(value)])
                        })
                        .collect(),
                ),
            )
        }
        other => other.clone(),
    }
}

/// `json.dumps(value, sort_keys=True, separators=(",", ":"))` over an
/// encoded content: object keys in code point order, a number that is not
/// a safe integer as Python's `repr` of a float, strings as ECMAScript
/// writes them.
fn python_json(value: &Json) -> String {
    match value {
        Json::Array(items) => {
            let items: Vec<String> = items.iter().map(python_json).collect();
            format!("[{}]", items.join(","))
        }
        Json::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort();
            let members: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        js_string(key),
                        python_json(members.get(key).unwrap_or(&Json::Null))
                    )
                })
                .collect();
            format!("{{{}}}", members.join(","))
        }
        Json::Number(number) => {
            let value = number.as_f64().unwrap_or(f64::NAN);
            if value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "a safe integer converts exactly"
                )]
                let integer = value as i64;
                integer.to_string()
            } else {
                python_float(value)
            }
        }
        Json::String(text) => js_string(text),
        Json::Bool(value) => value.to_string(),
        Json::Null => "null".into(),
    }
}

/// The original's `value.identities_json`: the identities in identity
/// order without repeats, each name as a JSON string and each embedding as
/// a list of Python floats.
#[must_use]
pub fn identities_json(ids: &[Json]) -> String {
    let mut keyed: Vec<(String, &Json)> = ids.iter().map(|id| (sort_key(id), id)).collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.dedup_by(|right, left| left.0 == right.0);
    let texts: Vec<String> = keyed
        .into_iter()
        .map(|(_, id)| match id {
            Json::String(name) => js_string(name),
            Json::Array(components) => python_vector(components),
            other => other.to_string(),
        })
        .collect();
    format!("[{}]", texts.join(","))
}

/// An identity: a string, or an array of numbers (an embedding).
fn is_identity(value: &Json) -> bool {
    match value {
        Json::String(_) => true,
        Json::Array(values) => values.iter().all(Json::is_number),
        _ => false,
    }
}

/// The original's `identity_sort_key` as one text, compared in code point
/// order: `0` and a name's text, or `1` and an embedding's compact Python
/// JSON.
fn sort_key(id: &Json) -> String {
    match id {
        Json::String(name) => format!("0{name}"),
        Json::Array(components) => format!("1{}", python_vector(components)),
        other => format!("2{other}"),
    }
}

/// An embedding as Python's compact `json.dumps` writes a list of floats.
fn python_vector(components: &[Json]) -> String {
    let texts: Vec<String> = components
        .iter()
        .map(|component| python_float(component.as_f64().unwrap_or(f64::NAN)))
        .collect();
    format!("[{}]", texts.join(","))
}

/// Python's `repr` of a finite float (`float_repr_style` `short`): the
/// shortest round-trip digits, positional with at least one fraction digit
/// when the decimal exponent is from -4 to 15, else `d[.ddd]e±XX`.
#[must_use]
pub fn python_float(value: f64) -> String {
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

/// A string as `asciiJson` wrote it: [`js_string`] with every UTF-16 code
/// unit from U+007F up as lower-case `\uxxxx`.
fn ascii_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for character in js_string(text).chars() {
        if u32::from(character) < 0x7f {
            out.push(character);
        } else {
            let mut units = [0_u16; 2];
            for unit in character.encode_utf16(&mut units) {
                push_unit(&mut out, *unit);
            }
        }
    }
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
fn contents_element<H: MemoryHost>(
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

/// The original's `OutputData.contents` of one statement's result: the
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
fn cut_at_word_element<H: MemoryHost>(
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

/// Python's `str.isspace` for one character: Unicode `White_Space`, and
/// the information separators U+001C to U+001F (bidirectional class B or
/// S), which Python counts and Unicode does not.
fn is_python_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// The original's `_cut_at_word`: `words` within [`MAX_TURN_TEXT_BYTES`],
/// cut at the last whole word, and whether it was cut (see the crate
/// documentation, `<memory:cut_at_word>`).
#[must_use]
pub fn cut_at_word(words: &str) -> (String, bool) {
    if words.len() <= MAX_TURN_TEXT_BYTES {
        return (words.to_owned(), false);
    }
    // `encoded[:MAX].decode(errors="ignore")`: the whole characters within
    // the bound.
    let end = (0..=MAX_TURN_TEXT_BYTES)
        .rev()
        .find(|index| words.is_char_boundary(*index))
        .unwrap_or(0);
    let (mut head, tail) = words.split_at(end);
    let ends_in_space = head.chars().next_back().is_some_and(is_python_space);
    let next_is_space = tail.chars().next().is_some_and(is_python_space);
    if !(ends_in_space || next_is_space) {
        // The bound fell inside a word: drop that partial word.
        if let Some(rest) = before_last_word(head) {
            head = rest;
        }
    }
    (head.trim_end_matches(is_python_space).to_owned(), true)
}

/// `text.rsplit(maxsplit=1)[0]` when that split has two parts: `text`
/// without its trailing white space, its last word, and the white space
/// before that word; `None` when it holds at most one word.
fn before_last_word(text: &str) -> Option<&str> {
    let trimmed = text.trim_end_matches(is_python_space);
    let (index, _) = trimmed
        .char_indices()
        .rev()
        .find(|(_, character)| is_python_space(*character))?;
    let rest = trimmed
        .get(..index)
        .unwrap_or_default()
        .trim_end_matches(is_python_space);
    (!rest.is_empty()).then_some(rest)
}

/// Executes `<memory:stm_projection>`.
fn stm_projection_element<H: MemoryHost>(
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
/// place and its last value, as Python's `json.loads` builds a dict.
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

/// The original's `_scalar_projection_value`: a boolean, a number, or a
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

/// The original's `_payload_projection` of one value (see the crate
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

    use super::{Attribute, Content, Data, Executed, Failure, MemoryHost};

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

    impl MemoryHost for Host {
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
