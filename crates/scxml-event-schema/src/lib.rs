//! Compile-time resolution of the `event` attribute vocabulary's JSON Schemas
//! ([`EVENT_NAMESPACE_URI`]).
//!
//! `<transition>`, `<send>`, `<raise>`, and `<donedata>` may carry
//! `event:schema`, a JSON Schema describing the event data the element
//! handles or creates. Its value is either inline JSON (first non-white-space
//! character `{`, or exactly `true` or `false`) or a URI reference to a JSON
//! file relative to the document, optionally with a JSON Pointer fragment
//! (RFC 6901, `schemas/order.json#/$defs/Order`). [`CompiledSchema`] checks
//! event data against a resolved schema; the engine uses it for the data of
//! the events an executed `<send>` or `<raise>` creates.
//!
//! [`resolve`] turns the bytes of one schema source into the canonical JSON
//! text the compiler embeds in Machine IR:
//!
//! 1. the bytes are UTF-8 JSON (a leading byte-order mark is ignored);
//! 2. the fragment, when present, selects the schema (RFC 6901);
//! 3. the dialect is the selected schema's `$schema`, else the document
//!    root's, else JSON Schema 2020-12; only 2020-12, 2019-09, and draft-07
//!    are supported ([`Dialect`]);
//! 4. references are bundled or rejected (below);
//! 5. the result is checked against its dialect's metaschema, and compiled
//!    once so that every reference it keeps resolves and every `pattern` is a
//!    valid regular expression;
//! 6. the result is written as canonical JSON ([`canonical_json`]) with its
//!    SHA-256.
//!
//! # References
//!
//! No schema is ever fetched: a `$ref`, `$dynamicRef`, or `$recursiveRef`
//! whose value is not a fragment (`#` or `#…`) names another resource and is
//! rejected ([`ReferenceReason::External`]). Without a pointer the file's
//! schema is embedded whole, so its fragment references keep their meaning.
//! With a pointer, a JSON Pointer reference (`#/…`) into the selected schema
//! is rebased onto it, and one to anywhere else in the file is bundled: the
//! referenced value is copied into the result's `$defs` (`definitions` for
//! draft-07) under its original pointer as the key, and the reference is
//! rewritten to it. A selected schema that relies on identifiers (`$id`,
//! `$anchor`, `$dynamicAnchor`, `$recursiveAnchor`, plain-name fragments, or
//! dynamic references) cannot be moved and is rejected
//! ([`ReferenceReason::NotRelocatable`]).
//!
//! # Bounds
//!
//! One schema source is at most [`MAX_SCHEMA_BYTES`] bytes, and a pointer
//! extraction copies at most [`MAX_BUNDLED_TARGETS`] referenced values
//! (wasm builds only). On every build, JSON nesting is bounded by the JSON
//! parser's recursion limit (128 levels, which every resolved schema must
//! also fit, so the Machine IR decoder can read it back), and a pointer
//! extraction's result is at most [`MAX_AMPLIFICATION`] times the source
//! bytes (plus 4096): an amplification guard, checked as the copies grow.
//!
//! Every function is pure: no I/O, clock, or randomness, and no telemetry
//! (callers run it inside their own boundary span).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

pub use scxml_types::EVENT_NAMESPACE_URI;
use scxml_types::wasm_bound;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

/// The local name of the attribute holding a schema (inline or by reference).
pub const SCHEMA_ATTRIBUTE: &str = "schema";
/// The local name of the attribute holding an expression whose value is the
/// schema (SCXML's attribute/`expr` convention).
pub const SCHEMA_EXPR_ATTRIBUTE: &str = "schemaexpr";
/// The SCXML elements that may carry the attributes, sorted.
pub const OWNER_ELEMENTS: [&str; 4] = ["donedata", "raise", "send", "transition"];
/// Maximum bytes of one schema source, and of one resolved schema (wasm
/// builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_SCHEMA_BYTES: usize = wasm_bound(1024 * 1024);
/// Maximum distinct referenced values one pointer extraction copies into its
/// result (wasm builds only; unbounded natively, see
/// [`scxml_types::wasm_bound`]). The work of copying is bounded on every
/// build by [`MAX_AMPLIFICATION`].
pub const MAX_BUNDLED_TARGETS: usize = wasm_bound(256);
/// Maximum ratio of a pointer extraction's result bytes to its source bytes
/// (plus 4096). An amplification guard on every build (AGENTS.md: output
/// that grows faster than its input): references to a value and to its
/// nested subschemas copy the same bytes repeatedly.
pub const MAX_AMPLIFICATION: usize = 8;
/// Slack added to the amplification guard so tiny sources can still bundle.
const AMPLIFICATION_SLACK: usize = 4096;
/// Maximum characters kept from a library or value message.
const MAX_MESSAGE_CHARS: usize = 256;

/// Whether `local_name` names an SCXML element that may carry the attributes.
#[must_use]
pub fn is_owner_element(local_name: &str) -> bool {
    OWNER_ELEMENTS.contains(&local_name)
}

/// A supported JSON Schema dialect.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Dialect {
    /// JSON Schema 2020-12, the default when no `$schema` is given.
    Draft202012,
    /// JSON Schema 2019-09.
    Draft201909,
    /// JSON Schema draft-07.
    Draft7,
}

impl Dialect {
    /// The canonical `$schema` URI of the dialect.
    #[must_use]
    pub const fn uri(self) -> &'static str {
        match self {
            Self::Draft202012 => "https://json-schema.org/draft/2020-12/schema",
            Self::Draft201909 => "https://json-schema.org/draft/2019-09/schema",
            Self::Draft7 => "http://json-schema.org/draft-07/schema#",
        }
    }

    /// The dialect a `$schema` value names: the canonical URI, with or
    /// without its trailing empty fragment.
    #[must_use]
    pub fn from_uri(uri: &str) -> Option<Self> {
        let bare = uri.strip_suffix('#').unwrap_or(uri);
        [Self::Draft202012, Self::Draft201909, Self::Draft7]
            .into_iter()
            .find(|dialect| dialect.uri().strip_suffix('#').unwrap_or(dialect.uri()) == bare)
    }

    const fn draft(self) -> boon::Draft {
        match self {
            Self::Draft202012 => boon::Draft::V2020_12,
            Self::Draft201909 => boon::Draft::V2019_09,
            Self::Draft7 => boon::Draft::V7,
        }
    }

    /// The keyword that holds reusable schemas in this dialect.
    const fn defs_keyword(self) -> &'static str {
        match self {
            Self::Draft202012 | Self::Draft201909 => "$defs",
            Self::Draft7 => "definitions",
        }
    }
}

/// Where one `event:schema` value takes its schema from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source<'a> {
    /// The value is the schema's JSON text.
    Inline(&'a str),
    /// The value is a URI reference: `resource`, a path relative to the
    /// document, and the optional `fragment` after `#` (still
    /// percent-encoded).
    Reference {
        /// The resource part, without the fragment, trimmed of XML white
        /// space.
        resource: &'a str,
        /// The fragment after the first `#`, when the value has one.
        fragment: Option<&'a str>,
    },
}

/// Reads an `event:schema` value: inline JSON when its first non-white-space
/// character is `{` or it is exactly `true` or `false` (around XML white
/// space), otherwise a URI reference split at its first `#`.
#[must_use]
pub fn source(value: &str) -> Source<'_> {
    let trimmed = value.trim_matches(scxml_types::is_xml_space);
    if trimmed.starts_with('{') || trimmed == "true" || trimmed == "false" {
        return Source::Inline(value);
    }
    match trimmed.split_once('#') {
        Some((resource, fragment)) => Source::Reference {
            resource,
            fragment: Some(fragment),
        },
        None => Source::Reference {
            resource: trimmed,
            fragment: None,
        },
    }
}

/// Why a reference's resource part cannot name a file relative to the
/// document, or `None` when it can: a non-empty relative path with no scheme,
/// authority, query, backslash, absolute path, or `.`/`..` segment.
/// Repository-namespace resources and other absolute URIs are not supported.
#[must_use]
pub fn resource_problem(resource: &str) -> Option<&'static str> {
    if resource.is_empty() {
        return Some(
            "the reference names no file (a fragment alone would point into the SCXML document)",
        );
    }
    let scheme = resource.find(':').is_some_and(|colon| {
        let name = &resource[..colon];
        !name.contains('/')
            && name
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    });
    if scheme || resource.starts_with('/') {
        return Some(
            "only a path relative to the document is supported (an absolute URI, an absolute path, or a repository-namespace resource is not)",
        );
    }
    if resource.contains('?') {
        return Some("a schema file reference has no query");
    }
    if resource.contains('\\') {
        return Some("a URI path separates segments with `/`, not `\\`");
    }
    if resource
        .split('/')
        .any(|segment| segment == ".." || segment == "." || segment.is_empty())
    {
        return Some(
            "the path has an empty, `.`, or `..` segment: a schema file must be named directly, at or below the document's directory",
        );
    }
    None
}

/// One resolved schema: canonical JSON text and its SHA-256.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolved {
    /// Canonical JSON text ([`canonical_json`]).
    pub schema: String,
    /// Lowercase hex SHA-256 of `schema`'s UTF-8 bytes.
    pub sha256: String,
    /// The dialect the schema was checked against.
    pub dialect: Dialect,
}

/// Why a reference was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceReason {
    /// It names another resource; nothing is fetched.
    External,
    /// It names no value in the file, or the compiled schema could not
    /// resolve it.
    Unresolved,
    /// A pointer extraction cannot move schemas that rely on this keyword.
    NotRelocatable(&'static str),
    /// A pointer extraction would copy more than [`MAX_BUNDLED_TARGETS`]
    /// referenced values, exceed [`MAX_AMPLIFICATION`], or nest deeper than
    /// the JSON parser's recursion limit.
    TooLarge,
}

/// Why a schema source could not be resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaError {
    /// The source or the resolved schema exceeds [`MAX_SCHEMA_BYTES`].
    TooLarge {
        /// The byte bound.
        limit: usize,
    },
    /// The source is not UTF-8.
    InvalidUtf8 {
        /// Byte offset of the first invalid sequence.
        offset: usize,
    },
    /// The source is not JSON.
    InvalidJson {
        /// Byte offset of the error in the source.
        offset: usize,
        /// 1-based line.
        line: usize,
        /// 1-based column (bytes).
        column: usize,
        /// The parser's message, without its position.
        message: String,
    },
    /// The fragment is not a JSON Pointer.
    InvalidPointer {
        /// What is wrong with it.
        message: &'static str,
    },
    /// The pointer names no value.
    PointerNotFound {
        /// The longest prefix of the pointer that names no value.
        pointer: String,
    },
    /// `$schema` names an unsupported dialect.
    UnsupportedDialect {
        /// The `$schema` value (its JSON text when not a string), shortened.
        value: String,
    },
    /// The schema violates its dialect's metaschema, or does not compile.
    InvalidSchema {
        /// JSON Pointer to the offending value in the resolved schema.
        location: String,
        /// The validator's message, shortened, without instance values.
        message: String,
    },
    /// A reference cannot be kept.
    Reference {
        /// JSON Pointer, in the file, of the keyword holding the reference.
        location: String,
        /// The reference (or keyword) as written, shortened.
        reference: String,
        /// Why it was rejected.
        reason: ReferenceReason,
    },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { limit } => write!(formatter, "the schema exceeds {limit} bytes"),
            Self::InvalidUtf8 { offset } => {
                write!(formatter, "the schema is not UTF-8 (byte {offset})")
            }
            Self::InvalidJson {
                line,
                column,
                message,
                ..
            } => write!(
                formatter,
                "invalid JSON at line {line}, column {column}: {message}"
            ),
            Self::InvalidPointer { message } => formatter.write_str(message),
            Self::PointerNotFound { pointer } => {
                write!(formatter, "the JSON Pointer `{pointer}` names no value")
            }
            Self::UnsupportedDialect { value } => {
                write!(formatter, "unsupported `$schema` {value}")
            }
            Self::InvalidSchema { location, message } => {
                write!(formatter, "at `{}`: {message}", display_pointer(location))
            }
            Self::Reference {
                location,
                reference,
                reason,
            } => {
                let at = display_pointer(location);
                match reason {
                    ReferenceReason::External => {
                        write!(formatter, "`{reference}` at `{at}` names another resource")
                    }
                    ReferenceReason::Unresolved => {
                        write!(formatter, "`{reference}` at `{at}` names no value")
                    }
                    ReferenceReason::NotRelocatable(keyword) => write!(
                        formatter,
                        "`{keyword}` at `{at}` ties the schema to its place in the file"
                    ),
                    ReferenceReason::TooLarge => write!(
                        formatter,
                        "bundling the references of the selected schema exceeds the bundling bounds (count, size, or nesting depth; at `{at}`)"
                    ),
                }
            }
        }
    }
}

impl std::error::Error for SchemaError {}

fn display_pointer(pointer: &str) -> &str {
    if pointer.is_empty() { "#" } else { pointer }
}

/// Resolves one schema source.
///
/// `bytes` is the inline attribute value or the referenced file's bytes;
/// `fragment` is the reference's fragment (percent-encoded, without `#`), or
/// `None`. Pure and deterministic.
///
/// # Errors
///
/// Every failure as a [`SchemaError`].
pub fn resolve(bytes: &[u8], fragment: Option<&str>) -> Result<Resolved, SchemaError> {
    if scxml_types::exceeds_wasm_bound(bytes.len(), MAX_SCHEMA_BYTES) {
        return Err(SchemaError::TooLarge {
            limit: MAX_SCHEMA_BYTES,
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|error| SchemaError::InvalidUtf8 {
        offset: error.valid_up_to(),
    })?;
    let bom = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let root = parse_json(text, bom)?;
    let tokens = match fragment {
        Some(fragment) => parse_fragment(fragment)?,
        None => Vec::new(),
    };
    let selected = lookup(&root, &tokens).ok_or_else(|| SchemaError::PointerNotFound {
        pointer: missing_prefix(&root, &tokens),
    })?;
    let dialect = match declared_dialect(selected)? {
        Some(dialect) => dialect,
        None => declared_dialect(&root)?.unwrap_or(Dialect::Draft202012),
    };
    let schema = if tokens.is_empty() {
        check_external(&root)?;
        root
    } else {
        let root_dialect = root.get("$schema").is_some().then_some(dialect);
        extract(&root, &tokens, dialect, root_dialect, bytes.len())?
    };
    check_schema(&schema, dialect)?;
    let text = canonical_json(&schema);
    if scxml_types::exceeds_wasm_bound(text.len(), MAX_SCHEMA_BYTES) {
        return Err(SchemaError::TooLarge {
            limit: MAX_SCHEMA_BYTES,
        });
    }
    // Bundling can nest a value deeper than its file did; the Machine IR
    // decoder reads the embedded text with the same parser and recursion
    // limit, so a result it cannot read back is rejected here.
    if !tokens.is_empty() && serde_json::from_str::<Value>(&text).is_err() {
        return Err(reference_error(
            &tokens,
            "$ref",
            "$ref",
            ReferenceReason::TooLarge,
        ));
    }
    let sha256 = sha256_hex(text.as_bytes());
    Ok(Resolved {
        schema: text,
        sha256,
        dialect,
    })
}

/// The byte range, in the JSON text `text`, of the value the JSON Pointer
/// `pointer` (RFC 6901, unencoded) names: where a finding in an inline
/// schema is. `None` when `text` is not JSON or the pointer names no value.
/// Linear in `text`, without recursion.
#[must_use]
pub fn locate(text: &str, pointer: &str) -> Option<(usize, usize)> {
    let tokens = parse_pointer(pointer).ok()?;
    serde_json::from_str::<Value>(text).ok()?;
    let bytes = text.as_bytes();
    let mut at = skip_space(bytes, 0);
    for token in &tokens {
        at = match bytes.get(at)? {
            b'{' => member(text, at, token)?,
            b'[' => item(bytes, at, array_index(token)?)?,
            _ => return None,
        };
    }
    Some((at, value_end(bytes, at)?))
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes
        .get(at)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
    {
        at += 1;
    }
    at
}

/// The start of member `name` of the object at `at`.
fn member(text: &str, at: usize, name: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = skip_space(bytes, at + 1);
    while *bytes.get(at)? == b'"' {
        let key_end = string_end(bytes, at)?;
        let key: String = serde_json::from_str(text.get(at..key_end)?).ok()?;
        let colon = skip_space(bytes, key_end);
        let value = skip_space(bytes, colon + 1);
        if key == name {
            return Some(value);
        }
        at = skip_space(bytes, value_end(bytes, value)?);
        if *bytes.get(at)? != b',' {
            return None;
        }
        at = skip_space(bytes, at + 1);
    }
    None
}

/// The start of item `index` of the array at `at`.
fn item(bytes: &[u8], at: usize, index: usize) -> Option<usize> {
    let mut at = skip_space(bytes, at + 1);
    for _ in 0..index {
        at = skip_space(bytes, value_end(bytes, at)?);
        if *bytes.get(at)? != b',' {
            return None;
        }
        at = skip_space(bytes, at + 1);
    }
    (*bytes.get(at)? != b']').then_some(at)
}

/// The end of the string token at `at` (just after its closing quote).
fn string_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut at = at + 1;
    loop {
        match *bytes.get(at)? {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
}

/// The end of the value at `at`.
fn value_end(bytes: &[u8], at: usize) -> Option<usize> {
    match *bytes.get(at)? {
        b'"' => string_end(bytes, at),
        b'{' | b'[' => {
            let mut depth = 0_usize;
            let mut index = at;
            loop {
                match *bytes.get(index)? {
                    b'"' => {
                        index = string_end(bytes, index)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(index + 1);
                        }
                    }
                    _ => {}
                }
                index += 1;
            }
        }
        _ => {
            let mut index = at;
            while bytes.get(index).is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            }) {
                index += 1;
            }
            Some(index)
        }
    }
}

/// The member names of the object the JSON Pointer `pointer` (RFC 6901,
/// unencoded) names in the JSON document `bytes`, for suggesting a near
/// miss; empty when it names no object.
#[must_use]
pub fn member_names(bytes: &[u8], pointer: &str) -> Vec<String> {
    let text = std::str::from_utf8(bytes).unwrap_or_default();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (Ok(root), Ok(tokens)) = (serde_json::from_str::<Value>(text), parse_pointer(pointer))
    else {
        return Vec::new();
    };
    match lookup(&root, &tokens) {
        Some(Value::Object(members)) => members.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The JSON Schema simple type names (`type` keyword values).
pub const TYPE_NAMES: [&str; 7] = [
    "array", "boolean", "integer", "null", "number", "object", "string",
];

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Writes `value` as canonical JSON: compact (no white space), object
/// members sorted by the UTF-8 bytes of their keys, strings and numbers as
/// `serde_json` writes them (the shortest round-trip form of a float).
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

/// Whether `text` is canonical JSON ([`canonical_json`] of its own parse).
#[must_use]
pub fn is_canonical_json(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|value| canonical_json(&value) == text)
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                if let Some(member) = members.get(key) {
                    write_canonical(member, out);
                }
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

fn parse_json(text: &str, bom: usize) -> Result<Value, SchemaError> {
    let body = text.get(bom..).unwrap_or_default();
    serde_json::from_str(body).map_err(|error| {
        let line = error.line().max(1);
        let column = error.column();
        let line_start = body
            .match_indices('\n')
            .nth(line.saturating_sub(2))
            .map_or(0, |(index, _)| if line > 1 { index + 1 } else { 0 });
        let mut offset = line_start
            .saturating_add(column.saturating_sub(1))
            .min(body.len());
        while offset > 0 && !body.is_char_boundary(offset) {
            offset -= 1;
        }
        let message = error.to_string();
        let message = message
            .rfind(" at line ")
            .map_or(message.as_str(), |end| &message[..end]);
        SchemaError::InvalidJson {
            offset: offset.saturating_add(bom),
            line,
            column: column.max(1),
            message: shorten(message),
        }
    })
}

/// Parses a percent-encoded JSON Pointer fragment into its reference tokens.
fn parse_fragment(fragment: &str) -> Result<Vec<String>, SchemaError> {
    let decoded = percent_decode(fragment).ok_or(SchemaError::InvalidPointer {
        message: "the fragment's percent-encoding is invalid or not UTF-8",
    })?;
    parse_pointer(&decoded)
}

fn parse_pointer(pointer: &str) -> Result<Vec<String>, SchemaError> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(SchemaError::InvalidPointer {
            message: "a JSON Pointer fragment is empty or starts with `/` (plain-name fragments are not supported)",
        });
    };
    rest.split('/')
        .map(|token| {
            let mut out = String::with_capacity(token.len());
            let mut chars = token.chars();
            while let Some(character) = chars.next() {
                if character == '~' {
                    match chars.next() {
                        Some('0') => out.push('~'),
                        Some('1') => out.push('/'),
                        _ => {
                            return Err(SchemaError::InvalidPointer {
                                message: "in a JSON Pointer `~` is followed by `0` or `1` (RFC 6901)",
                            });
                        }
                    }
                } else {
                    out.push(character);
                }
            }
            Ok(out)
        })
        .collect()
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn lookup<'v>(root: &'v Value, tokens: &[String]) -> Option<&'v Value> {
    tokens.iter().try_fold(root, |value, token| match value {
        Value::Object(members) => members.get(token),
        Value::Array(items) => array_index(token).and_then(|index| items.get(index)),
        _ => None,
    })
}

/// An RFC 6901 array index: `0` or digits without a leading zero.
fn array_index(token: &str) -> Option<usize> {
    if token.is_empty()
        || !token.bytes().all(|byte| byte.is_ascii_digit())
        || (token.len() > 1 && token.starts_with('0'))
    {
        return None;
    }
    token.parse().ok()
}

fn missing_prefix(root: &Value, tokens: &[String]) -> String {
    let found = (0..=tokens.len())
        .take_while(|count| lookup(root, &tokens[..*count]).is_some())
        .last()
        .unwrap_or(0);
    pointer_text(&tokens[..(found + 1).min(tokens.len())])
}

/// The JSON Pointer text of `tokens` (`""` for the root).
fn pointer_text(tokens: &[String]) -> String {
    tokens.iter().fold(String::new(), |mut out, token| {
        out.push('/');
        out.push_str(&token.replace('~', "~0").replace('/', "~1"));
        out
    })
}

fn declared_dialect(value: &Value) -> Result<Option<Dialect>, SchemaError> {
    match value.get("$schema") {
        None => Ok(None),
        Some(Value::String(uri)) => {
            Dialect::from_uri(uri)
                .map(Some)
                .ok_or_else(|| SchemaError::UnsupportedDialect {
                    value: format!("`{}`", shorten(uri)),
                })
        }
        Some(other) => Err(SchemaError::UnsupportedDialect {
            value: format!("`{}` (not a URI string)", shorten(&other.to_string())),
        }),
    }
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

/// Keywords whose value is one subschema.
const SCHEMA_KEYWORDS: &[&str] = &[
    "additionalItems",
    "additionalProperties",
    "contains",
    "contentSchema",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];
/// Keywords whose value is an array of subschemas.
const ARRAY_KEYWORDS: &[&str] = &["allOf", "anyOf", "items", "oneOf", "prefixItems"];
/// Keywords whose value is an object of subschemas.
const MAP_KEYWORDS: &[&str] = &[
    "$defs",
    "definitions",
    "dependencies",
    "dependentSchemas",
    "patternProperties",
    "properties",
];
/// Keywords whose value is a reference.
const REFERENCE_KEYWORDS: &[&str] = &["$dynamicRef", "$recursiveRef", "$ref"];
/// Keywords that identify a schema by something other than its place.
const IDENTIFIER_KEYWORDS: &[&str] = &["$anchor", "$dynamicAnchor", "$id", "$recursiveAnchor"];

/// What a schema walk reports each finding to.
type Visit<'f> = dyn FnMut(&[String], Found<'_>) -> Result<(), SchemaError> + 'f;

/// What a schema walk found.
enum Found<'a> {
    /// A reference keyword with a string value.
    Reference {
        keyword: &'static str,
        value: &'a mut String,
    },
    /// An identifier keyword.
    Identifier { keyword: &'static str },
}

/// Walks every subschema of `schema` (applicator keywords only, so values
/// of `const`, `enum`, `default`, `examples`, and unknown keywords are never
/// read as schemas), reporting reference and identifier keywords with the
/// JSON Pointer tokens of the object that holds them.
fn walk(
    schema: &mut Value,
    path: &mut Vec<String>,
    visit: &mut Visit<'_>,
) -> Result<(), SchemaError> {
    let Value::Object(members) = schema else {
        return Ok(());
    };
    for keyword in IDENTIFIER_KEYWORDS {
        if members.contains_key(*keyword) {
            visit(path, Found::Identifier { keyword })?;
        }
    }
    for keyword in REFERENCE_KEYWORDS {
        if let Some(Value::String(value)) = members.get_mut(*keyword) {
            visit(path, Found::Reference { keyword, value })?;
        }
    }
    for (key, value) in members.iter_mut() {
        let key = key.as_str();
        path.push(key.to_owned());
        let result = if SCHEMA_KEYWORDS.contains(&key) && value.is_object() {
            walk(value, path, visit)
        } else if ARRAY_KEYWORDS.contains(&key) && value.is_array() {
            walk_items(value, path, visit)
        } else if MAP_KEYWORDS.contains(&key) {
            walk_members(value, path, visit)
        } else {
            Ok(())
        };
        path.pop();
        result?;
    }
    Ok(())
}

fn walk_items(
    value: &mut Value,
    path: &mut Vec<String>,
    visit: &mut Visit<'_>,
) -> Result<(), SchemaError> {
    if let Value::Array(items) = value {
        for (index, item) in items.iter_mut().enumerate() {
            path.push(index.to_string());
            let result = walk(item, path, visit);
            path.pop();
            result?;
        }
    }
    Ok(())
}

fn walk_members(
    value: &mut Value,
    path: &mut Vec<String>,
    visit: &mut Visit<'_>,
) -> Result<(), SchemaError> {
    if let Value::Object(members) = value {
        for (key, member) in members.iter_mut() {
            path.push(key.clone());
            let result = walk(member, path, visit);
            path.pop();
            result?;
        }
    }
    Ok(())
}

fn reference_error(
    path: &[String],
    keyword: &str,
    reference: &str,
    reason: ReferenceReason,
) -> SchemaError {
    let mut location = path.to_vec();
    location.push(keyword.to_owned());
    SchemaError::Reference {
        location: pointer_text(&location),
        reference: shorten(reference),
        reason,
    }
}

/// Rejects every reference that is not a fragment of the same file.
fn check_external(root: &Value) -> Result<(), SchemaError> {
    let mut scratch = root.clone();
    walk(
        &mut scratch,
        &mut Vec::new(),
        &mut |path, found| match found {
            Found::Reference { keyword, value } if !value.starts_with('#') => Err(reference_error(
                path,
                keyword,
                value,
                ReferenceReason::External,
            )),
            _ => Ok(()),
        },
    )
}

/// The references of one pointer extraction: rebased onto the selected
/// schema, or bundled from elsewhere in the file.
struct Bundler<'r> {
    root: &'r Value,
    tokens: &'r [String],
    defs_keyword: &'static str,
    /// Keys of copied values (their original pointers) and their tokens, in
    /// the order they were first referenced; processed as a worklist.
    wanted: Vec<(String, Vec<String>)>,
}

impl Bundler<'_> {
    fn rewrite(&mut self, path: &[String], found: Found<'_>) -> Result<(), SchemaError> {
        let (keyword, value) = match found {
            Found::Identifier { keyword } => {
                return Err(reference_error(
                    path,
                    keyword,
                    keyword,
                    ReferenceReason::NotRelocatable(keyword),
                ));
            }
            Found::Reference { keyword, value } => (keyword, value),
        };
        let Some(fragment) = value.strip_prefix('#') else {
            return Err(reference_error(
                path,
                keyword,
                value,
                ReferenceReason::External,
            ));
        };
        if keyword != "$ref" {
            return Err(reference_error(
                path,
                keyword,
                value,
                ReferenceReason::NotRelocatable(keyword),
            ));
        }
        // A fragment that does not percent-decode names no value (never the
        // root).
        let pointer = percent_decode(fragment)
            .ok_or_else(|| reference_error(path, keyword, value, ReferenceReason::Unresolved))?;
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return Err(reference_error(
                path,
                keyword,
                value,
                ReferenceReason::NotRelocatable("plain-name fragment"),
            ));
        }
        let target = parse_pointer(&pointer)
            .ok()
            .filter(|target| lookup(self.root, target).is_some())
            .ok_or_else(|| reference_error(path, keyword, value, ReferenceReason::Unresolved))?;
        let rebased = if let Some(inner) = target.strip_prefix(self.tokens) {
            pointer_text(inner)
        } else {
            let key = pointer_text(&target);
            if !self.wanted.iter().any(|(wanted, _)| *wanted == key) {
                if scxml_types::exceeds_wasm_bound(
                    self.wanted.len().saturating_add(1),
                    MAX_BUNDLED_TARGETS,
                ) {
                    return Err(reference_error(
                        path,
                        keyword,
                        value,
                        ReferenceReason::TooLarge,
                    ));
                }
                self.wanted.push((key.clone(), target));
            }
            pointer_text(&[self.defs_keyword.to_owned(), key])
        };
        *value = format!("#{}", encode_fragment(&rebased));
        Ok(())
    }
}

/// Extracts the schema at `tokens`, rebasing and bundling its references.
fn extract(
    root: &Value,
    tokens: &[String],
    dialect: Dialect,
    root_dialect: Option<Dialect>,
    source_bytes: usize,
) -> Result<Value, SchemaError> {
    let mut selected = lookup(root, tokens).cloned().unwrap_or(Value::Bool(true));
    let defs_keyword = dialect.defs_keyword();
    let mut bundler = Bundler {
        root,
        tokens,
        defs_keyword,
        wanted: Vec::new(),
    };
    walk(&mut selected, &mut tokens.to_vec(), &mut |path, found| {
        bundler.rewrite(path, found)
    })?;
    let limit = source_bytes
        .saturating_mul(MAX_AMPLIFICATION)
        .saturating_add(AMPLIFICATION_SLACK);
    let too_large = || reference_error(tokens, "$ref", "$ref", ReferenceReason::TooLarge);
    // The guard is checked as the copies grow, so a quadratic bundle stops
    // at the bound instead of being built first.
    let mut bundle_bytes = canonical_json(&selected).len();
    let mut copies = Map::new();
    let mut next = 0;
    while let Some((key, target)) = bundler.wanted.get(next).cloned() {
        next += 1;
        let mut copy = lookup(root, &target).cloned().unwrap_or(Value::Bool(true));
        bundle_bytes = bundle_bytes.saturating_add(canonical_json(&copy).len());
        if bundle_bytes > limit {
            return Err(too_large());
        }
        walk(&mut copy, &mut target.clone(), &mut |path, found| {
            bundler.rewrite(path, found)
        })?;
        copies.insert(key, copy);
    }
    insert_bundled(&mut selected, tokens, defs_keyword, copies)?;
    if let (Some(dialect), Value::Object(members)) = (root_dialect, &mut selected) {
        members
            .entry("$schema".to_owned())
            .or_insert_with(|| Value::String(dialect.uri().to_owned()));
    }
    if canonical_json(&selected).len() > limit {
        return Err(too_large());
    }
    Ok(selected)
}

/// Adds the bundled values to the selected schema's definitions.
fn insert_bundled(
    selected: &mut Value,
    tokens: &[String],
    defs_keyword: &'static str,
    bundled: Map<String, Value>,
) -> Result<(), SchemaError> {
    if bundled.is_empty() {
        return Ok(());
    }
    let not_relocatable = |reference: &str| {
        reference_error(
            tokens,
            defs_keyword,
            reference,
            ReferenceReason::NotRelocatable(defs_keyword),
        )
    };
    let Value::Object(members) = selected else {
        return Err(not_relocatable(defs_keyword));
    };
    let Value::Object(defs) = members
        .entry(defs_keyword.to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return Err(not_relocatable(defs_keyword));
    };
    for (key, value) in bundled {
        if defs.insert(key.clone(), value).is_some() {
            return Err(not_relocatable(&key));
        }
    }
    Ok(())
}

/// Percent-encodes a JSON Pointer for use as a URI fragment: every byte
/// outside RFC 3986 `pchar` and `/`.
fn encode_fragment(pointer: &str) -> String {
    let mut out = String::with_capacity(pointer.len());
    for byte in pointer.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
            )
        {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

/// The in-memory location the compiled schema is registered under; nothing is
/// ever loaded from it.
const SCHEMA_LOCATION: &str = "urn:scxml:event-schema";

/// Refuses every URL: a schema is compiled only from its in-memory value, so
/// boon's native `file` loader is never reached.
struct NoLoader;

impl boon::UrlLoader for NoLoader {
    fn load(&self, url: &str) -> Result<Value, Box<dyn std::error::Error>> {
        Err(format!("{url} is not loaded").into())
    }
}

/// Checks `schema` against its dialect's metaschema and compiles it once
/// (boon checks the metaschema while compiling), so every keyword has a
/// valid value, every reference resolves, and every `pattern` is a valid
/// regular expression.
fn check_schema(schema: &Value, dialect: Dialect) -> Result<(), SchemaError> {
    let mut compiler = boon::Compiler::new();
    compiler.use_loader(Box::new(NoLoader));
    compiler.set_default_draft(dialect.draft());
    let mut schemas = boon::Schemas::new();
    if let Err(error) = compiler.add_resource(SCHEMA_LOCATION, schema.clone()) {
        return Err(compile_error(error));
    }
    match compiler.compile(SCHEMA_LOCATION, &mut schemas) {
        Ok(_) => Ok(()),
        Err(error) => Err(compile_error(error)),
    }
}

// ---------------------------------------------------------------------------
// Checking event data at a boundary
// ---------------------------------------------------------------------------

/// One resolved schema compiled for checking event data: the engine checks
/// the data of the events an executed `<send>` or `<raise>` creates against
/// the literal `event:schema`s that apply (`scxml-engine`'s `contracts`).
pub struct CompiledSchema {
    schemas: boon::Schemas,
    index: boon::SchemaIndex,
    /// The schema is the boolean schema `true`, which admits every value,
    /// XML data included.
    admits_all: bool,
}

impl fmt::Debug for CompiledSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledSchema")
            .field("admits_all", &self.admits_all)
            .finish_non_exhaustive()
    }
}

impl CompiledSchema {
    /// Compiles one canonical resolved schema ([`Resolved::schema`]): its
    /// dialect is its `$schema`, else JSON Schema 2020-12, and nothing is
    /// ever loaded. Deterministic.
    ///
    /// # Errors
    ///
    /// [`SchemaError`] for text that is not canonical JSON, an unsupported
    /// dialect, or a schema that does not compile.
    pub fn new(canonical: &str) -> Result<Self, SchemaError> {
        let not_canonical = || SchemaError::InvalidSchema {
            location: String::new(),
            message: "the schema is not canonical JSON".to_owned(),
        };
        let schema: Value = serde_json::from_str(canonical).map_err(|_| not_canonical())?;
        if canonical_json(&schema) != canonical {
            return Err(not_canonical());
        }
        let dialect = declared_dialect(&schema)?.unwrap_or(Dialect::Draft202012);
        let admits_all = schema == Value::Bool(true);
        let mut compiler = boon::Compiler::new();
        compiler.use_loader(Box::new(NoLoader));
        compiler.set_default_draft(dialect.draft());
        let mut schemas = boon::Schemas::new();
        compiler
            .add_resource(SCHEMA_LOCATION, schema)
            .map_err(compile_error)?;
        let index = compiler
            .compile(SCHEMA_LOCATION, &mut schemas)
            .map_err(compile_error)?;
        Ok(Self {
            schemas,
            index,
            admits_all,
        })
    }

    /// Whether `data` satisfies the schema: its [`data_instance`] is valid,
    /// or the schema is the boolean schema `true` (the only one that admits
    /// XML data, which has no JSON instance). Bounded by the data (event
    /// data is bounded at every boundary) and the schema.
    #[must_use]
    pub fn admits(&self, data: &scxml_types::DataValue) -> bool {
        if self.admits_all {
            return true;
        }
        data_instance(data)
            .is_some_and(|instance| self.schemas.validate(&instance, self.index).is_ok())
    }
}

/// The JSON instance a schema checks for one event's data: no data (absent
/// or `undefined`) and `null` are `null`; booleans, numbers, strings, and
/// JSON arrays and objects are themselves (an integral number within the
/// 64-bit integer range is an integer). XML data has no JSON instance
/// (`None`).
#[must_use]
pub fn data_instance(data: &scxml_types::DataValue) -> Option<Value> {
    use scxml_types::DataValue;
    match data {
        DataValue::Absent | DataValue::Undefined | DataValue::Null => Some(Value::Null),
        DataValue::Bool(value) => Some(Value::Bool(*value)),
        DataValue::Number(number) => {
            let value = number.get();
            // -2^63 and 2^63 are exact in f64; an integral value in
            // [-2^63, 2^63) converts to i64 exactly.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the value is integral and inside the i64 range, checked on the same line"
            )]
            let integer = (value.fract() == 0.0
                && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&value))
            .then_some(value as i64);
            match integer {
                Some(integer) => Some(Value::from(integer)),
                None => serde_json::Number::from_f64(value).map(Value::Number),
            }
        }
        DataValue::String(text) => Some(Value::String(text.clone())),
        DataValue::JsonArray(text) | DataValue::JsonObject(text) => serde_json::from_str(text).ok(),
        DataValue::Xml(_) => None,
    }
}

/// The finding for one boon compile failure.
fn compile_error(error: boon::CompileError) -> SchemaError {
    match error {
        boon::CompileError::ValidationError { src, .. } => {
            let leaf = deepest(&src);
            SchemaError::InvalidSchema {
                location: leaf.instance_location.to_string(),
                message: shorten(&leaf.kind.to_string()),
            }
        }
        boon::CompileError::InvalidRegex { url, regex, .. } => SchemaError::InvalidSchema {
            location: fragment_pointer(&url),
            message: shorten(&format!("`{regex}` is not a valid regular expression")),
        },
        boon::CompileError::JsonPointerNotFound(url)
        | boon::CompileError::InvalidJsonPointer(url)
        | boon::CompileError::LoadUrlError { url, .. }
        | boon::CompileError::UnsupportedUrlScheme { url } => SchemaError::Reference {
            location: String::new(),
            reference: shorten(&display_reference(&url)),
            reason: ReferenceReason::Unresolved,
        },
        boon::CompileError::AnchorNotFound { reference, .. } => SchemaError::Reference {
            location: String::new(),
            reference: shorten(&display_reference(&reference)),
            reason: ReferenceReason::Unresolved,
        },
        other => SchemaError::InvalidSchema {
            location: String::new(),
            message: shorten(&display_reference(&other.to_string())),
        },
    }
}

/// The most specific cause of a metaschema failure: the first leaf at the
/// deepest instance location.
fn deepest<'e, 's, 'v>(
    error: &'e boon::ValidationError<'s, 'v>,
) -> &'e boon::ValidationError<'s, 'v> {
    let mut best = error;
    let mut stack = vec![error];
    while let Some(next) = stack.pop() {
        if next.causes.is_empty()
            && (!best.causes.is_empty()
                || next.instance_location.tokens.len() > best.instance_location.tokens.len())
        {
            best = next;
        }
        stack.extend(next.causes.iter().rev());
    }
    best
}

/// The JSON Pointer of a `…#/pointer` URL (empty for the root).
fn fragment_pointer(url: &str) -> String {
    url.split_once('#')
        .map(|(_, fragment)| percent_decode(fragment).unwrap_or_default())
        .unwrap_or_default()
}

/// A URL or message as the author wrote it, without the internal location.
fn display_reference(text: &str) -> String {
    text.replace(SCHEMA_LOCATION, "")
}

fn shorten(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(MAX_MESSAGE_CHARS).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// The resources of every `event:schema` reference, keyed by the authored
/// resource part (see [`Source::Reference`]).
pub type Resources = BTreeMap<String, Vec<u8>>;

/// Maximum resources in one component request (`event-schemas`; an
/// interface bound on every build).
pub const MAX_REQUEST_RESOURCES: usize = 64;
/// Maximum UTF-8 bytes of one resource's URI in a component request.
pub const MAX_REQUEST_RESOURCE_URI_BYTES: usize = 16 * 1024;
/// Maximum bytes of one resource in a component request.
pub const MAX_REQUEST_RESOURCE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum bytes of every resource of one component request together.
pub const MAX_REQUEST_TOTAL_RESOURCE_BYTES: usize = 16 * 1024 * 1024;

/// Collects a component request's `event-schemas` into [`Resources`], or
/// `None` when they break a request bound (count, URI length, bytes, total)
/// or repeat a URI.
#[must_use]
pub fn request_resources(
    resources: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Option<Resources> {
    let mut collected = Resources::new();
    let mut total = 0_usize;
    for (uri, bytes) in resources {
        total = total.checked_add(bytes.len())?;
        if collected.len() >= MAX_REQUEST_RESOURCES
            || uri.is_empty()
            || uri.len() > MAX_REQUEST_RESOURCE_URI_BYTES
            || bytes.len() > MAX_REQUEST_RESOURCE_BYTES
            || total > MAX_REQUEST_TOTAL_RESOURCE_BYTES
            || collected.insert(uri, bytes).is_some()
        {
            return None;
        }
    }
    Some(collected)
}

#[cfg(test)]
mod tests;
