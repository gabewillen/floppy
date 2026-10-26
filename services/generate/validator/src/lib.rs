//! The package validator of the `generate` service
//! (`stateforward:scxml-validator/package-validator`): scxml.rs calls it once
//! per document, after its XSD and built-in checks, with every `<invoke>`
//! whose literal `type` is [`SERVICE_URI`], and reports what it finds as
//! `generate::<code>`. It imports nothing: it reads only the parsed document
//! it is given.
//!
//! What it checks is what the service itself would reject when the
//! `<invoke>` runs (`error.execution`), or would return that the document
//! says it does not accept:
//!
//! * `unknown_param` (error): a `<param>` (or `namelist` entry) the service
//!   takes no parameter of, with a machine-applicable rename to the
//!   parameter strictly closest to it within two edits ([`PARAMETERS`]).
//! * `duplicate_param` (error): a parameter given twice.
//! * `missing_param` (error): no `model`, or not exactly one of `prompt` and
//!   `messages`; `schemaname` or `tool_choice` without `schema`; with
//!   `input` (an embeddings call), any parameter but `model` and
//!   `dimensions`; `dimensions` without `input`.
//! * `invalid_value` (error, ECMAScript documents, literal `expr` only): a
//!   `reasoning_effort` that is not one of [`REASONING_EFFORTS`] (a
//!   machine-applicable fix to the one strictly closest within two edits)
//!   or, with a `schema`, is not [`TOOLS_REASONING_EFFORT`] (a fix to it), a
//!   `tool_choice` that is not one of [`TOOL_CHOICES`] (with the same fix),
//!   a `stream` that is not `true` or `false`, a `temperature` that is not
//!   a number from 0 to 2, a `max_completion_tokens` or
//!   `thinking_token_budget` that is not a whole number from 1 to 1048576,
//!   a `schemaname` outside `[A-Za-z0-9_-]{1,64}`, a `dimensions` that is
//!   not a whole number from 1 to 4096, or an empty `model`, `prompt`,
//!   `system`, or `input`.
//! * `schema_mismatch` (warning): a transition that can take the
//!   invocation's events (in the invoking state, an ancestor, or a
//!   descendant) handles `generate.failed` or `generate.result` with a
//!   literal `event:schema` that rejects data the service returns (each
//!   closed failure code, a refused call's `rejection`, or a `{"content",
//!   "tool_calls", "request_bytes", "usage"}` reply: a text reply without a
//!   `schema` parameter, a tool call with one, and no call with a
//!   `tool_choice` that may be `auto`, any but a literal `required`; with
//!   `input`, a `{"vectors", "request_bytes", "usage"}` reply); the
//!   help names the schema of what
//!   it returns.
//!
//! The decisions are the portable [`check`] over a plain [`Document`],
//! tested natively; the guest maps the WIT types onto it.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use scxml_event_schema::{CompiledSchema, canonical_json};
use scxml_types::DataValue;
use serde_json::Value as Json;

/// The service's exact type URI.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/generate";
/// The parameters the service takes.
pub const PARAMETERS: [&str; 14] = [
    "model",
    "prompt",
    "messages",
    "system",
    "schema",
    "schemaname",
    "reasoning_effort",
    "stream",
    "max_completion_tokens",
    "tool_choice",
    "temperature",
    "thinking_token_budget",
    "input",
    "dimensions",
];
/// The `tool_choice` values the service accepts.
pub const TOOL_CHOICES: [&str; 2] = ["required", "auto"];
/// The only `reasoning_effort` the service accepts with a `schema`.
pub const TOOLS_REASONING_EFFORT: &str = "none";
/// The `reasoning_effort` values the service accepts.
pub const REASONING_EFFORTS: [&str; 6] = ["none", "minimal", "low", "medium", "high", "xhigh"];
/// The closed failure codes of `generate.failed`.
pub const FAILURE_CODES: [&str; 9] = [
    "unconfigured",
    "transport",
    "status",
    "malformed",
    "truncated",
    "filtered",
    "refusal",
    "not-json",
    "schema-mismatch",
];
/// The event that carries the reply.
pub const RESULT_EVENT: &str = "generate.result";
/// The event that reports a failed generation.
pub const FAILED_EVENT: &str = "generate.failed";
/// Upper bound on the bytes of a schema name.
const MAX_SCHEMA_NAME_BYTES: usize = 64;
/// Upper bound on `temperature`.
const MAX_TEMPERATURE: f64 = 2.0;
/// Upper bound on `max_completion_tokens` and `thinking_token_budget`.
const MAX_COMPLETION_TOKENS: u32 = 1_048_576;

const MAX_DIMENSIONS: u32 = 4096;

const SCXML: &str = "http://www.w3.org/2005/07/scxml";

/// A half-open UTF-8 byte range of the document.
pub type Span = (u32, u32);

/// One attribute.
#[derive(Clone, Debug)]
pub struct Attribute {
    /// Namespace URI; `None` for no namespace.
    pub namespace: Option<String>,
    /// Local name.
    pub local: String,
    /// Normalized value.
    pub value: String,
    /// The whole attribute.
    pub span: Span,
}

/// One element.
#[derive(Clone, Debug)]
pub struct Element {
    /// Namespace URI.
    pub namespace: Option<String>,
    /// Local name.
    pub local: String,
    /// The whole element.
    pub span: Span,
    /// Attributes in source order.
    pub attributes: Vec<Attribute>,
    /// Child element indices.
    pub children: Vec<u32>,
    /// Parent element index.
    pub parent: Option<u32>,
}

impl Element {
    fn attribute(&self, local: &str) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|attribute| attribute.namespace.is_none() && attribute.local == local)
    }

    fn is(&self, local: &str) -> bool {
        self.namespace.as_deref() == Some(SCXML) && self.local == local
    }
}

/// One `<transition>`, in document order.
#[derive(Clone, Debug)]
pub struct Transition {
    /// Its `event` attribute.
    pub event: Option<String>,
    /// Its resolved literal `event:schema`.
    pub schema: Option<String>,
}

/// The parsed document.
#[derive(Clone, Debug, Default)]
pub struct Document {
    /// Every element in document order.
    pub elements: Vec<Element>,
    /// Every `<transition>`, in document order.
    pub transitions: Vec<Transition>,
    /// The root's `datamodel` attribute.
    pub datamodel: Option<String>,
}

/// A finding's severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    /// Error.
    Error,
    /// Warning.
    Warning,
}

/// One machine-applicable repair: replace `span` with `replacement`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fix {
    /// Summary.
    pub message: String,
    /// The replaced range (inside the `<invoke>`).
    pub span: Span,
    /// Replacement text.
    pub replacement: String,
}

/// One finding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    /// The code, reported as `generate::<code>`.
    pub code: &'static str,
    /// Severity.
    pub severity: Severity,
    /// Summary.
    pub message: String,
    /// The primary range and its label.
    pub primary: (Span, String),
    /// Related ranges.
    pub secondary: Vec<(Span, String)>,
    /// Help.
    pub help: Option<String>,
    /// The repair, when one is certain.
    pub fix: Option<Fix>,
}

/// Every finding for the `<invoke>` elements `targets` of `document`.
#[must_use]
pub fn check(document: &Document, targets: &[u32]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for invoke in targets
        .iter()
        .filter_map(|target| document.elements.get(usize::try_from(*target).ok()?))
        .filter(|element| element.is("invoke"))
    {
        let given = parameters(document, invoke, &mut findings);
        schema_mismatches(document, invoke, given, &mut findings);
    }
    findings
}

/// The children of `element`.
fn children<'a>(document: &'a Document, element: &'a Element) -> impl Iterator<Item = &'a Element> {
    element
        .children
        .iter()
        .filter_map(|child| document.elements.get(usize::try_from(*child).ok()?))
}

/// The candidate closest to `typed`, within two edits, when no other is as
/// close.
fn closest<'a>(typed: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let mut scored: Vec<(usize, &str)> = candidates
        .iter()
        .map(|candidate| (distance(typed, candidate), *candidate))
        .filter(|(score, _)| *score <= 2)
        .collect();
    scored.sort_unstable();
    match scored.as_slice() {
        [(best, only)] => Some((*best, *only)),
        [(best, first), (next, _), ..] if best < next => Some((*best, *first)),
        _ => None,
    }
    .map(|(_, candidate)| candidate)
}

/// Levenshtein distance over characters.
fn distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.chars().enumerate() {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(row + 1);
        for (column, right_char) in right.iter().enumerate() {
            let substitute = previous.get(column).map_or(usize::MAX, |cost| {
                cost + usize::from(left_char != *right_char)
            });
            let delete = previous.get(column + 1).map_or(usize::MAX, |cost| cost + 1);
            let insert = current.last().map_or(usize::MAX, |cost| cost + 1);
            current.push(substitute.min(delete).min(insert));
        }
        previous = current;
    }
    previous.last().copied().unwrap_or_default()
}

/// A literal ECMAScript value of an `expr`: a quoted string without escapes,
/// `true` or `false`, or a decimal number (negated or not).
#[derive(Debug, PartialEq)]
enum Literal<'a> {
    String(&'a str),
    Bool,
    Number(f64),
    Other,
}

fn literal(expr: &str) -> Literal<'_> {
    let expr = expr.trim();
    for quote in ['\'', '"'] {
        if let Some(inner) = expr
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
            && !inner.contains(quote)
            && !inner.contains('\\')
        {
            return Literal::String(inner);
        }
    }
    let digits = expr.strip_prefix(['-', '+']).unwrap_or(expr);
    match expr {
        "true" | "false" => Literal::Bool,
        _ if digits == "Infinity" || digits == "NaN" => Literal::Number(f64::NAN),
        _ if digits.starts_with(|first: char| first.is_ascii_digit() || first == '.')
            && digits.bytes().all(|byte| {
                byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-')
            }) =>
        {
            expr.parse::<f64>().map_or(Literal::Other, Literal::Number)
        }
        _ => Literal::Other,
    }
}

/// What the parameters of an `<invoke>` say the service may return.
#[derive(Clone, Copy)]
struct Given {
    /// It gives `schema`: the reply holds a tool call.
    schema: bool,
    /// It gives a `tool_choice` that may be `auto` (any but a literal
    /// `required`): the reply may hold no call.
    tool_choice: bool,
    /// It gives `input`: the reply is vectors.
    input: bool,
}

/// Checks the parameters of `invoke`; what it gives.
fn parameters(document: &Document, invoke: &Element, findings: &mut Vec<Finding>) -> Given {
    let ecmascript = document
        .datamodel
        .as_deref()
        .is_none_or(|datamodel| datamodel == "ecmascript");
    let mut seen: Vec<&str> = Vec::new();
    let mut required_choice = false;
    let mut effort = None;
    for param in children(document, invoke).filter(|child| child.is("param")) {
        let Some(name) = param.attribute("name") else {
            continue;
        };
        named(&mut seen, &name.value, name.span, Some(name), findings);
        if ecmascript && let Some(expr) = param.attribute("expr") {
            value(&name.value, expr, findings);
            required_choice |=
                name.value == "tool_choice" && literal(&expr.value) == Literal::String("required");
            if name.value == "reasoning_effort"
                && let Literal::String(given) = literal(&expr.value)
                && given != TOOLS_REASONING_EFFORT
                && REASONING_EFFORTS.contains(&given)
            {
                effort = Some((given, expr.span));
            }
        }
    }
    if let Some(namelist) = invoke.attribute("namelist") {
        for name in namelist.value.split_ascii_whitespace() {
            named(&mut seen, name, namelist.span, None, findings);
        }
    }
    let has = |name: &str| seen.contains(&name);
    let missing = |message: &str, help: &str| Finding {
        code: "missing_param",
        severity: Severity::Error,
        message: message.to_owned(),
        primary: (invoke.span, "this invocation".to_owned()),
        secondary: Vec::new(),
        help: Some(help.to_owned()),
        fix: None,
    };
    if !has("model") {
        findings.push(missing(
            "the generate service requires a `model` parameter",
            "add `<param name=\"model\" expr=\"…\"/>`",
        ));
    }
    if has("input") {
        if let Some(name) = seen
            .iter()
            .find(|name| !matches!(**name, "model" | "input" | "dimensions"))
        {
            findings.push(missing(
                &format!("`input` takes only `model` and `dimensions`, not `{name}`"),
                &format!("remove `{name}`, or embed in another invocation"),
            ));
        }
    } else if has("prompt") == has("messages") {
        findings.push(missing(
            "the generate service takes exactly one of `prompt` and `messages`",
            "give either a `prompt` or a `messages` parameter",
        ));
    }
    if has("dimensions") && !has("input") {
        findings.push(missing(
            "`dimensions` requires an `input` parameter",
            "add the `input` parameter, or remove `dimensions`",
        ));
    }
    for name in ["schemaname", "tool_choice"] {
        if !has("schema") && has(name) {
            findings.push(missing(
                &format!("`{name}` requires a `schema` parameter"),
                &format!("add the `schema` parameter, or remove `{name}`"),
            ));
        }
    }
    if has("schema")
        && let Some((given, span)) = effort
    {
        findings.push(Finding {
            code: "invalid_value",
            severity: Severity::Error,
            message: format!("reasoning_effort `{given}` is refused with a `schema`"),
            primary: (span, "the service rejects this value".to_owned()),
            secondary: Vec::new(),
            help: Some(format!(
                "a call is asked for with reasoning_effort `{TOOLS_REASONING_EFFORT}`"
            )),
            fix: Some(Fix {
                message: format!("use `{TOOLS_REASONING_EFFORT}`"),
                span,
                replacement: format!("expr=\"'{TOOLS_REASONING_EFFORT}'\""),
            }),
        });
    }
    Given {
        schema: has("schema"),
        tool_choice: has("tool_choice") && !required_choice,
        input: has("input"),
    }
}

/// Records one parameter `name` (at `span`; `attribute` is its `name`
/// attribute, which a fix may rename) and reports it when unknown or
/// repeated.
fn named<'a>(
    seen: &mut Vec<&'a str>,
    name: &'a str,
    span: Span,
    attribute: Option<&Attribute>,
    findings: &mut Vec<Finding>,
) {
    if !PARAMETERS.contains(&name) {
        let suggestion = closest(name, &PARAMETERS);
        findings.push(Finding {
            code: "unknown_param",
            severity: Severity::Error,
            message: format!("the generate service takes no parameter `{name}`"),
            primary: (span, "unknown parameter".to_owned()),
            secondary: Vec::new(),
            help: Some(suggestion.map_or_else(
                || format!("use one of {}", PARAMETERS.join(", ")),
                |known| format!("did you mean `{known}`?"),
            )),
            fix: suggestion.zip(attribute).map(|(known, attribute)| Fix {
                message: format!("use `{known}`"),
                span: attribute.span,
                replacement: format!("name=\"{known}\""),
            }),
        });
        return;
    }
    if seen.contains(&name) {
        findings.push(Finding {
            code: "duplicate_param",
            severity: Severity::Error,
            message: format!("the parameter `{name}` is given twice"),
            primary: (span, "given again here".to_owned()),
            secondary: Vec::new(),
            help: Some("give each parameter once".to_owned()),
            fix: None,
        });
    }
    seen.push(name);
}

/// Checks one literal `expr` of the parameter `name`.
fn value(name: &str, expr: &Attribute, findings: &mut Vec<Finding>) {
    let literal = literal(&expr.value);
    let invalid = |message: String, help: String, fix: Option<Fix>| Finding {
        code: "invalid_value",
        severity: Severity::Error,
        message,
        primary: (expr.span, "the service rejects this value".to_owned()),
        secondary: Vec::new(),
        help: Some(help),
        fix,
    };
    match (name, literal) {
        ("reasoning_effort", Literal::String(effort)) if !REASONING_EFFORTS.contains(&effort) => {
            let suggestion = closest(effort, &REASONING_EFFORTS);
            findings.push(invalid(
                format!("`{effort}` is not a reasoning_effort the generate service accepts"),
                suggestion.map_or_else(
                    || format!("use one of {}", REASONING_EFFORTS.join(", ")),
                    |known| format!("did you mean `{known}`?"),
                ),
                suggestion.map(|known| Fix {
                    message: format!("use `{known}`"),
                    span: expr.span,
                    replacement: format!("expr=\"'{known}'\""),
                }),
            ));
        }
        ("reasoning_effort", Literal::Bool) => findings.push(invalid(
            "reasoning_effort must be a string".to_owned(),
            format!("use one of {}", REASONING_EFFORTS.join(", ")),
            None,
        )),
        ("tool_choice", Literal::String(choice)) if !TOOL_CHOICES.contains(&choice) => {
            let suggestion = closest(choice, &TOOL_CHOICES);
            findings.push(invalid(
                format!("`{choice}` is not a tool_choice the generate service accepts"),
                suggestion.map_or_else(
                    || format!("use one of {}", TOOL_CHOICES.join(", ")),
                    |known| format!("did you mean `{known}`?"),
                ),
                suggestion.map(|known| Fix {
                    message: format!("use `{known}`"),
                    span: expr.span,
                    replacement: format!("expr=\"'{known}'\""),
                }),
            ));
        }
        ("tool_choice", Literal::Bool | Literal::Number(_)) => findings.push(invalid(
            "tool_choice must be a string".to_owned(),
            format!("use one of {}", TOOL_CHOICES.join(", ")),
            None,
        )),
        (
            "temperature" | "max_completion_tokens" | "thinking_token_budget" | "dimensions",
            Literal::String(_) | Literal::Bool,
        ) => findings.push(invalid(
            format!("{name} must be a number"),
            "write the number without quotes".to_owned(),
            None,
        )),
        (_, Literal::Number(number)) => {
            if let Some(bound) = out_of_bounds(name, number) {
                findings.push(invalid(
                    format!("{name} must be {bound}"),
                    format!("write {bound}"),
                    None,
                ));
            }
        }
        ("stream", Literal::String(_)) => findings.push(invalid(
            "stream must be a boolean".to_owned(),
            "write `true` or `false`".to_owned(),
            None,
        )),
        ("schemaname", Literal::String(schema_name))
            if schema_name.is_empty()
                || schema_name.len() > MAX_SCHEMA_NAME_BYTES
                || !schema_name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) =>
        {
            findings.push(invalid(
                format!("`{schema_name}` is not a schema name"),
                "a schema name matches [A-Za-z0-9_-]{1,64}".to_owned(),
                None,
            ));
        }
        ("model" | "prompt" | "system" | "input", Literal::String(text))
            if text.trim().is_empty() =>
        {
            findings.push(invalid(
                format!("{name} must be a non-empty string"),
                format!("give the {name} text"),
                None,
            ));
        }
        _ => {}
    }
}

/// The bound a literal `number` of the parameter `name` is outside, as the
/// service checks it; `None` within it, or for a parameter that takes no
/// number (its other checks report that).
fn out_of_bounds(name: &str, number: f64) -> Option<String> {
    match name {
        "temperature" if !(0.0..=MAX_TEMPERATURE).contains(&number) => {
            Some(format!("a number from 0 to {MAX_TEMPERATURE}"))
        }
        "max_completion_tokens" | "thinking_token_budget"
            if number.fract() != 0.0
                || !(1.0..=f64::from(MAX_COMPLETION_TOKENS)).contains(&number) =>
        {
            Some(format!("a whole number from 1 to {MAX_COMPLETION_TOKENS}"))
        }
        "dimensions"
            if number.fract() != 0.0 || !(1.0..=f64::from(MAX_DIMENSIONS)).contains(&number) =>
        {
            Some(format!("a whole number from 1 to {MAX_DIMENSIONS}"))
        }
        _ => None,
    }
}

/// SCXML 3.12.1 event descriptor matching.
fn handles(descriptors: &str, event: &str) -> bool {
    descriptors.split_ascii_whitespace().any(|descriptor| {
        let prefix = descriptor
            .strip_suffix(".*")
            .or_else(|| descriptor.strip_suffix('.'))
            .unwrap_or(descriptor);
        descriptor == "*"
            || prefix.is_empty()
            || prefix == event
            || event
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('.'))
    })
}

/// Whether `ancestor` is `element` or one of its ancestors.
fn contains(document: &Document, ancestor: usize, element: usize) -> bool {
    let mut current = Some(element);
    while let Some(index) = current {
        if index == ancestor {
            return true;
        }
        current = document
            .elements
            .get(index)
            .and_then(|element| element.parent)
            .and_then(|parent| usize::try_from(parent).ok());
    }
    false
}

/// What the service returns: `(event, the schema of its data, instances)`.
fn returned(given: Given) -> Vec<(&'static str, Json, Vec<Json>)> {
    let failed = serde_json::json!({
        "type": "object",
        "properties": {
            "code": {"enum": FAILURE_CODES},
            "rejection": {
                "type": "object",
                "properties": {"key": {"type": "string"}, "message": {"type": "string"}},
                "required": ["key", "message"],
                "additionalProperties": false,
            },
        },
        "required": ["code"],
        "additionalProperties": false,
    });
    // Each code, and a refused call's rejection.
    let mut failures: Vec<Json> = FAILURE_CODES
        .iter()
        .map(|code| serde_json::json!({ "code": code }))
        .collect();
    failures.extend([
        serde_json::json!({
            "code": "not-json",
            "rejection": {"key": "not-json", "message": "the arguments are not JSON: EOF while parsing a value at line 1 column 0"},
        }),
        serde_json::json!({
            "code": "schema-mismatch",
            "rejection": {"key": "schema-mismatch:", "message": "(root): missing properties 'events'"},
        }),
    ]);
    let mut events = vec![(FAILED_EVENT, failed, failures)];
    // The reply is the generate result: without a schema
    // its content and no call, with one at least one call; its request's
    // bytes, and its usage when the backend counted tokens.
    let usage = serde_json::json!({
        "input_tokens": 12, "output_tokens": 3, "reasoning_tokens": 1, "total_tokens": 15,
    });
    let mut samples = if given.schema {
        vec![
            serde_json::json!({
                "content": "",
                "request_bytes": 120,
                "tool_calls": [{"id": "call-1", "name": "result", "args": {}}],
            }),
            serde_json::json!({
                "content": "",
                "request_bytes": 120,
                "tool_calls": [{"id": "call-1", "name": "result", "args": {}}],
                "usage": usage,
            }),
        ]
    } else {
        vec![
            serde_json::json!({ "content": "a reply", "request_bytes": 60, "tool_calls": [] }),
            serde_json::json!({ "content": "a reply", "request_bytes": 60, "tool_calls": [], "usage": usage }),
        ]
    };
    if given.tool_choice {
        // Under `auto` blank content selects nothing.
        samples.push(serde_json::json!({ "content": "", "request_bytes": 120, "tool_calls": [] }));
    }
    let count = serde_json::json!({"type": "integer", "minimum": 0});
    let usage_schema = serde_json::json!({
        "type": "object",
        "properties": {
            "input_tokens": count,
            "output_tokens": count,
            "reasoning_tokens": count,
            "total_tokens": count,
        },
        "additionalProperties": false,
    });
    if given.input {
        // An embeddings reply: one unit vector per input.
        events.push((
            RESULT_EVENT,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "vectors": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}},
                    "request_bytes": count,
                    "usage": usage_schema,
                },
                "required": ["vectors", "request_bytes"],
                "additionalProperties": false,
            }),
            vec![
                serde_json::json!({ "vectors": [[0.6, 0.8]], "request_bytes": 40 }),
                serde_json::json!({
                    "vectors": [[0.6, 0.8]],
                    "request_bytes": 40,
                    "usage": {"input_tokens": 4, "total_tokens": 4},
                }),
            ],
        ));
        return events;
    }
    events.push((
        RESULT_EVENT,
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {"type": "string"},
                "tool_calls": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "args": {"type": "object"}},
                    "required": ["id", "name", "args"],
                    "additionalProperties": false,
                }},
                "request_bytes": count,
                "usage": usage_schema,
            },
            "required": ["content", "tool_calls", "request_bytes"],
            "additionalProperties": false,
        }),
        samples,
    ));
    events
}

/// The `schema_mismatch` findings of `invoke`.
fn schema_mismatches(
    document: &Document,
    invoke: &Element,
    given: Given,
    findings: &mut Vec<Finding>,
) {
    let Some(state) = invoke
        .parent
        .and_then(|parent| usize::try_from(parent).ok())
    else {
        return;
    };
    let transitions = document
        .elements
        .iter()
        .filter(|element| element.is("transition"))
        .zip(&document.transitions);
    for (element, transition) in transitions {
        let (Some(event), Some(schema)) = (&transition.event, &transition.schema) else {
            continue;
        };
        let Some(source) = element
            .parent
            .and_then(|parent| usize::try_from(parent).ok())
        else {
            continue;
        };
        // A transition can take the invocation's events while the invoking
        // state is active: its source is the state, an ancestor, or a
        // descendant.
        if !contains(document, source, state) && !contains(document, state, source) {
            continue;
        }
        let Ok(compiled) = CompiledSchema::new(schema) else {
            continue;
        };
        for (name, returned_schema, instances) in returned(given) {
            if !handles(event, name) {
                continue;
            }
            let Some(rejected) = instances.iter().find(|instance| {
                !compiled.admits(&DataValue::JsonObject(canonical_json(instance)))
            }) else {
                continue;
            };
            let attribute = element.attributes.iter().find(|attribute| {
                attribute.namespace.as_deref()
                    == Some("https://github.com/stateforward/scxml.rs/namespaces/event")
                    && attribute.local == "schema"
            });
            findings.push(Finding {
                code: "schema_mismatch",
                severity: Severity::Warning,
                message: format!(
                    "this transition's `event:schema` rejects `{name}` data the generate service returns"
                ),
                primary: (
                    attribute.map_or(element.span, |attribute| attribute.span),
                    format!("rejects `{}`", canonical_json(rejected)),
                ),
                secondary: vec![(invoke.span, "the invocation that returns it".to_owned())],
                help: Some(format!(
                    "the generate service returns `{name}` data satisfying `{}`",
                    canonical_json(&returned_schema)
                )),
                fix: None,
            });
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "package-validator",
        path: "../../../scxml.rs/crates/scxml-wit/wit/validator",
    });

    use exports::stateforward::scxml_validator::package_validator_api::Guest;
    use stateforward::scxml_validator::document::Context;
    use stateforward::scxml_validator::types::{
        Applicability, ComponentError, Diagnostic, Edit, Fix, Label, Severity, Span,
    };

    struct Component;

    const fn span((start, end): super::Span) -> Span {
        Span { start, end }
    }

    impl Guest for Component {
        fn validate(
            document: Context,
            targets: Vec<u32>,
        ) -> Result<Vec<Diagnostic>, ComponentError> {
            let plain = super::Document {
                elements: document
                    .elements
                    .into_iter()
                    .map(|element| super::Element {
                        namespace: element.name.namespace_uri,
                        local: element.name.local_name,
                        span: (element.span.start, element.span.end),
                        attributes: element
                            .attributes
                            .into_iter()
                            .map(|attribute| super::Attribute {
                                namespace: attribute.name.namespace_uri,
                                local: attribute.name.local_name,
                                value: attribute.value,
                                span: (attribute.span.start, attribute.span.end),
                            })
                            .collect(),
                        children: element.children,
                        parent: element.parent,
                    })
                    .collect(),
                transitions: document
                    .transitions
                    .into_iter()
                    .map(|transition| super::Transition {
                        event: transition.event,
                        schema: transition.event_schema,
                    })
                    .collect(),
                datamodel: document.datamodel,
            };
            Ok(super::check(&plain, &targets)
                .into_iter()
                .map(|finding| Diagnostic {
                    code: finding.code.to_owned(),
                    severity: match finding.severity {
                        super::Severity::Error => Severity::Error,
                        super::Severity::Warning => Severity::Warning,
                    },
                    message: finding.message,
                    primary: Label {
                        span: span(finding.primary.0),
                        message: Some(finding.primary.1),
                    },
                    secondary: finding
                        .secondary
                        .into_iter()
                        .map(|(range, message)| Label {
                            span: span(range),
                            message: Some(message),
                        })
                        .collect(),
                    help: finding.help,
                    fixes: finding
                        .fix
                        .into_iter()
                        .map(|fix| Fix {
                            message: fix.message,
                            applicability: Applicability::MachineApplicable,
                            edits: vec![Edit {
                                span: span(fix.span),
                                replacement: fix.replacement,
                            }],
                        })
                        .collect(),
                })
                .collect())
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests;
