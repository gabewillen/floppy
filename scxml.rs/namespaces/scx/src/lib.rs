//! The `scx` extension namespace: `<scx:validate>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! The SCXML authoring tools of the `scx` toolchain, for machines that
//! check documents they write (a builder bot, for example). Documents bind
//! the namespace, [`NAMESPACE_URI`], with the prefix `scx`; `scx` embeds its
//! package and component, so a document uses it with no flag and no fetch.
//!
//! # `<scx:validate>`
//!
//! Validates one SCXML document held as text with the scxml.rs validator,
//! linked into this component (`scxml-validator-core` with the bundled W3C
//! SCXML 1.0 XSD set of `scxml-validator-xsd`, strict schema policy), and
//! assigns the canonical validator report to a data-model location.
//!
//! * `location="L"` (required): receives the report, a JSON object: the
//!   `scxml.report/v3` validator artifact (`version`, `kind: "validator"`,
//!   and `validator` with `source_name`, `source_text`, `counts`, and every
//!   diagnostic with its code, severity, category, spans, notes, help, spec
//!   references, and fixes with their applicability). Its JSON value is
//!   exactly that of the canonical report bytes `scx validate --format
//!   json` writes for the same text, written with members sorted by key (the
//!   form a data-model `json-object` takes).
//! * the document text, exactly one of:
//!   * `expr="E"`: an expression that evaluates to a string, used exactly; or
//!   * one `<scx:content>` child holding the document as text (typically a
//!     CDATA section), with leading and trailing white space removed. Inline
//!     elements are not supported: the child holds text only.
//! * `name="N"` (optional, [`DEFAULT_SOURCE_NAME`] without it): the report's
//!   `source_name`. `event:schema` file references resolve relative to it.
//! * `<scx:eventschema uri="U">` children (optional, at most
//!   [`MAX_EVENT_SCHEMAS`]): the bytes of each file an `event:schema`
//!   attribute of the document references, keyed by the reference's resource
//!   part exactly as authored (`events.json` for `event:schema="events.json#/$defs/Go"`).
//!   Each holds the schema as text, or names it with `expr` (a string or a
//!   JSON value). Nothing is ever read from a file or the network: a
//!   reference whose bytes are not supplied is reported by the validator as
//!   `SCXML088` (the file is not available), like any other diagnostic.
//! * `applyfixes="machine-applicable"` with `fixedlocation="F"` (optional,
//!   both or neither): every `machine-applicable` fix of the report is
//!   applied to the text (fixes whose edits overlap an already accepted fix
//!   are skipped, rustfix's rule) and the fixed text is assigned to `F` as a
//!   string; the report still describes the original text. Validate the
//!   fixed text again to see what remains.
//! * `profile="draft"` (optional, [`PROFILE_DRAFT`]): also applies the
//!   validator's draft profile ([`scxml_validator_core::Profile::Draft`],
//!   `SCXML097` to `SCXML101`), for a document the machine runs as its own
//!   invoked child: only the SCXML and `event` namespaces, no `<invoke>` or
//!   `<script>`, no `src`/`srcexpr`, no `typeexpr`, no `<send type>` but the
//!   SCXML Event I/O Processor's, and no DTD. Its findings are ordinary
//!   diagnostics of the report.
//!
//! Without packages the validator binds no extension namespace and no
//! invoked-service type, exactly as `scx validate` without
//! `--extension-package` or `--service-package`: a draft that uses either is
//! reported (no schema for its namespace; `SCXML094` for a literal service
//! type). The data model's expression-syntax checker is a separate component
//! a extension component cannot call, so expressions are not
//! syntax-checked.
//!
//! The local name `validate` matches exactly; any other local name is
//! `not-handled` (the machine raises `error.execution`).
//!
//! # Failures
//!
//! An invalid document *is not* a failure: it is a report with errors, for
//! the machine to branch on. Every malformed element (an unknown attribute or
//! child, a missing or doubled source, a non-string text value, a bound
//! exceeded), evaluation, assignment, or reporting failure is an `execution`
//! error with a static detail (SCXML `error.execution`); the document, its
//! schemas, and the report are never echoed.
//!
//! # Bounds
//!
//! The document's bytes, the number and size of the event schemas, the
//! source name, and resource URIs are bounded on wasm builds only
//! ([`scxml_types::wasm_bound`]); the validator applies its own bounds (and
//! `scxml.report/v3` caps the canonical report at 32 MiB).
//!
//! One `execute` call runs the whole validator, the bundled XSD set
//! included: about 166 million instructions for a two-state draft and 214
//! million for a 1.2 KiB model-written one, most of it compiling the W3C
//! SCXML 1.0 XSD set. scxml.rs's default extension call cap
//! (`DEFAULT_EXTENSION_CALL_FUEL`, one billion) covers it, so a run needs no
//! fuel flag; an exhausted call is `error.execution` and retires the
//! instance for the rest of the session.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries the document, a schema, the report, a
//! location, or a name, so the cardinality is the product of the three
//! closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::collections::BTreeMap;

use scxml_reporting::{
    Applicability, ArtifactEnvelope, ByteSpan, Diagnostic, Fix, Label, TextEdit, encode_canonical,
};
use scxml_types::{exceeds_wasm_bound, wasm_bound};
use scxml_validator_core::{
    EventSchemaResources, MemorySchemaResolver, Profile, SchemaMode, Validator,
};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The namespace's name: the last path segment of its URI.
pub const NAMESPACE_NAME: &str = "scx";

/// The exact namespace URI documents bind: tagless, like the `env` and
/// `event` vocabularies, because it ships with (and is versioned with) the
/// `scx` toolchain, which embeds its package and component.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/scx";

/// The report's `source_name` when the element names none.
pub const DEFAULT_SOURCE_NAME: &str = "draft.scx";

/// The only `applyfixes` value: the applicability of the fixes applied.
pub const APPLY_MACHINE_APPLICABLE: &str = "machine-applicable";

/// The only `profile` value: the validator's draft profile.
pub const PROFILE_DRAFT: &str = "draft";

/// Upper bound on the document's bytes (wasm builds only).
pub const MAX_DOCUMENT_BYTES: usize = wasm_bound(4 * 1024 * 1024);
/// Upper bound on the `<scx:eventschema>` children (wasm builds only).
pub const MAX_EVENT_SCHEMAS: usize = wasm_bound(64);
/// Upper bound on one event schema's bytes (wasm builds only).
pub const MAX_EVENT_SCHEMA_BYTES: usize = wasm_bound(1024 * 1024);
/// Upper bound on every event schema's bytes together (wasm builds only).
pub const MAX_TOTAL_EVENT_SCHEMA_BYTES: usize = wasm_bound(8 * 1024 * 1024);
/// Upper bound on the bytes of `name` and of each `uri` (wasm builds only).
pub const MAX_NAME_BYTES: usize = wasm_bound(4096);

/// The source id of the one document a validation reads: every span points
/// into it.
const DOCUMENT_SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

/// One attribute of an element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    /// Whether the attribute has a namespace (only unqualified attributes are
    /// element parameters).
    pub qualified: bool,
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
    Start {
        /// Its namespace URI, when it has one.
        namespace_uri: Option<String>,
        /// Its local name.
        local_name: String,
        /// Its attributes.
        attributes: Vec<Attribute>,
    },
    /// The end tag of the innermost open nested element.
    End,
}

/// A value the data model evaluated.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A string.
    Text(String),
    /// A JSON object or array, as JSON text.
    Json(String),
    /// Any other value.
    Other,
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
pub trait ScxmlHost {
    /// A non-recoverable host failure.
    type Error;

    /// Evaluates one data-model expression; `Ok(Err(()))` is a recoverable
    /// evaluation failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, Self::Error>;

    /// Assigns one canonical JSON object to one data-model location;
    /// `Ok(Err(()))` is a recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign_json_object(
        &mut self,
        location: &str,
        json: &str,
    ) -> Result<Result<(), ()>, Self::Error>;

    /// Assigns one string to one data-model location; `Ok(Err(()))` is a
    /// recoverable assignment failure.
    ///
    /// # Errors
    ///
    /// A non-recoverable host failure.
    fn assign_text(&mut self, location: &str, value: &str) -> Result<Result<(), ()>, Self::Error>;
}

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, evaluation,
/// assignment, or reporting failure; [`Failure::Host`] for a non-recoverable
/// host failure.
pub fn execute<H: ScxmlHost>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "validate" => validate_element(attributes, content, host).map(|()| Executed::Handled),
        _ => Ok(Executed::NotHandled),
    }
}

/// The parsed parameters of one `<scx:validate>`.
#[derive(Debug, Default)]
struct Parameters<'a> {
    location: &'a str,
    expr: Option<&'a str>,
    name: Option<&'a str>,
    fixed_location: Option<&'a str>,
    profile: Option<Profile>,
}

/// One `<scx:eventschema>` child.
#[derive(Debug, Default)]
struct EventSchemaChild {
    uri: Option<String>,
    expr: Option<String>,
    text: String,
}

/// The parsed children of one `<scx:validate>`.
#[derive(Debug, Default)]
struct Children {
    content: Option<String>,
    event_schemas: Vec<EventSchemaChild>,
}

fn parameters(attributes: &[Attribute]) -> Result<Parameters<'_>, &'static str> {
    let mut location = None;
    let mut parameters = Parameters::default();
    let mut apply_fixes = None;
    for attribute in attributes {
        if attribute.qualified {
            return Err("scx:validate takes no namespaced attributes");
        }
        let value = attribute.value.as_str();
        match attribute.local_name.as_str() {
            "location" => location = Some(value),
            "expr" => parameters.expr = Some(value),
            "name" => parameters.name = Some(value),
            "applyfixes" => apply_fixes = Some(value),
            "fixedlocation" => parameters.fixed_location = Some(value),
            "profile" if value == PROFILE_DRAFT => parameters.profile = Some(Profile::Draft),
            "profile" => return Err("scx:validate profile accepts only draft"),
            _ => return Err("scx:validate has an attribute this package version does not define"),
        }
    }
    parameters.location = location
        .map(str::trim)
        .filter(|location| !location.is_empty())
        .ok_or("scx:validate requires a non-empty location")?;
    match (apply_fixes, parameters.fixed_location.map(str::trim)) {
        (None, None) => {}
        (Some(APPLY_MACHINE_APPLICABLE), Some(fixed)) if !fixed.is_empty() => {
            parameters.fixed_location = Some(fixed);
        }
        (Some(APPLY_MACHINE_APPLICABLE), _) => {
            return Err("scx:validate applyfixes requires a non-empty fixedlocation");
        }
        (Some(_), _) => {
            return Err("scx:validate applyfixes accepts only machine-applicable");
        }
        (None, Some(_)) => return Err("scx:validate fixedlocation requires applyfixes"),
    }
    if let Some(name) = parameters.name
        && (name.is_empty() || exceeds_wasm_bound(name.len(), MAX_NAME_BYTES))
    {
        return Err("scx:validate name must be non-empty and within the name bound");
    }
    Ok(parameters)
}

fn children(content: &[Content]) -> Result<Children, &'static str> {
    let mut children = Children::default();
    let mut items = content.iter();
    while let Some(item) = items.next() {
        let (namespace_uri, local_name, attributes) = match item {
            Content::Text(text) if text.trim().is_empty() => continue,
            Content::Text(_) => {
                return Err("scx:validate holds only scx:content and scx:eventschema children");
            }
            Content::End => return Err("scx:validate content is unbalanced"),
            Content::Start {
                namespace_uri,
                local_name,
                attributes,
            } => (namespace_uri, local_name, attributes),
        };
        if namespace_uri.as_deref() != Some(NAMESPACE_URI) {
            return Err("scx:validate holds only scx:content and scx:eventschema children");
        }
        // The child's text, up to its end tag; a nested element is refused.
        let mut text = String::new();
        loop {
            match items.next() {
                Some(Content::Text(part)) => text.push_str(part),
                Some(Content::End) => break,
                Some(Content::Start { .. }) => {
                    return Err("scx:content and scx:eventschema hold text only, not elements");
                }
                None => return Err("scx:validate content is unbalanced"),
            }
        }
        match local_name.as_str() {
            "content" => {
                if !attributes.is_empty() {
                    return Err("scx:content takes no attributes");
                }
                if children.content.is_some() {
                    return Err("scx:validate holds at most one scx:content");
                }
                children.content = Some(text);
            }
            "eventschema" => {
                let mut child = EventSchemaChild {
                    text,
                    ..EventSchemaChild::default()
                };
                for attribute in attributes {
                    match (attribute.qualified, attribute.local_name.as_str()) {
                        (false, "uri") => child.uri = Some(attribute.value.clone()),
                        (false, "expr") => child.expr = Some(attribute.value.clone()),
                        _ => {
                            return Err(
                                "scx:eventschema has an attribute this package version does not define",
                            );
                        }
                    }
                }
                if exceeds_wasm_bound(
                    children.event_schemas.len().saturating_add(1),
                    MAX_EVENT_SCHEMAS,
                ) {
                    return Err("scx:validate has more scx:eventschema children than the bound");
                }
                children.event_schemas.push(child);
            }
            _ => return Err("scx:validate holds only scx:content and scx:eventschema children"),
        }
    }
    Ok(children)
}

/// Evaluates `expression` to the text it names (a string, or a JSON object
/// or array when `json` is admitted).
fn evaluate_text<H: ScxmlHost>(
    host: &mut H,
    expression: &str,
    json: bool,
    failed: &'static str,
    not_text: &'static str,
) -> Result<String, Failure<H::Error>> {
    match host.evaluate(expression).map_err(Failure::Host)? {
        Ok(Value::Text(text)) => Ok(text),
        Ok(Value::Json(text)) if json => Ok(text),
        Ok(_) => Err(Failure::Execution(not_text)),
        Err(()) => Err(Failure::Execution(failed)),
    }
}

fn event_schemas<H: ScxmlHost>(
    children: Vec<EventSchemaChild>,
    host: &mut H,
) -> Result<EventSchemaResources, Failure<H::Error>> {
    let mut resources = EventSchemaResources::new();
    let mut total = 0_usize;
    for child in children {
        let uri = child
            .uri
            .filter(|uri| !uri.is_empty())
            .ok_or(Failure::Execution(
                "scx:eventschema requires a non-empty uri",
            ))?;
        if exceeds_wasm_bound(uri.len(), MAX_NAME_BYTES) {
            return Err(Failure::Execution(
                "scx:eventschema uri is longer than the name bound",
            ));
        }
        let literal = child.text.trim();
        let bytes = match (child.expr, literal.is_empty()) {
            (Some(expression), true) => evaluate_text(
                host,
                &expression,
                true,
                "scx:eventschema expr could not be evaluated",
                "scx:eventschema expr must evaluate to a string or a JSON value",
            )?
            .into_bytes(),
            (None, false) => literal.as_bytes().to_vec(),
            _ => {
                return Err(Failure::Execution(
                    "scx:eventschema holds exactly one of expr or text",
                ));
            }
        };
        total = total.saturating_add(bytes.len());
        if exceeds_wasm_bound(bytes.len(), MAX_EVENT_SCHEMA_BYTES)
            || exceeds_wasm_bound(total, MAX_TOTAL_EVENT_SCHEMA_BYTES)
        {
            return Err(Failure::Execution(
                "scx:eventschema schemas are larger than the bound",
            ));
        }
        if resources.insert(uri, bytes).is_some() {
            return Err(Failure::Execution("scx:eventschema names one uri twice"));
        }
    }
    Ok(resources)
}

fn validate_element<H: ScxmlHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<(), Failure<H::Error>> {
    let parameters = parameters(attributes).map_err(Failure::Execution)?;
    let children = children(content).map_err(Failure::Execution)?;
    let text = match (parameters.expr, children.content) {
        (Some(expression), None) => evaluate_text(
            host,
            expression,
            false,
            "scx:validate expr could not be evaluated",
            "scx:validate expr must evaluate to a string",
        )?,
        (None, Some(text)) => text.trim().to_owned(),
        _ => {
            return Err(Failure::Execution(
                "scx:validate takes exactly one of expr or scx:content",
            ));
        }
    };
    if exceeds_wasm_bound(text.len(), MAX_DOCUMENT_BYTES) {
        return Err(Failure::Execution(
            "scx:validate document is larger than the bound",
        ));
    }
    let resources = event_schemas(children.event_schemas, host)?;
    let validation = validate(
        parameters.name.unwrap_or(DEFAULT_SOURCE_NAME),
        &text,
        resources,
        parameters.profile,
    )
    .map_err(Failure::Execution)?;
    let fixed = match parameters.fixed_location {
        Some(_) => {
            Some(apply_machine_applicable(&validation.artifact).map_err(Failure::Execution)?)
        }
        None => None,
    };
    host.assign_json_object(parameters.location, &validation.report)
        .map_err(Failure::Host)?
        .map_err(|()| Failure::Execution("scx:validate could not assign the report to location"))?;
    if let (Some(location), Some(fixed)) = (parameters.fixed_location, fixed) {
        host.assign_text(location, &fixed)
            .map_err(Failure::Host)?
            .map_err(|()| {
                Failure::Execution("scx:validate could not assign the fixed text to fixedlocation")
            })?;
    }
    Ok(())
}

/// One validation: the canonical artifact and its canonical JSON text.
#[derive(Clone, Debug)]
pub struct Validation {
    /// The `scxml.report/v3` validator artifact.
    pub artifact: ArtifactEnvelope,
    /// The artifact as canonical JSON (members sorted by key).
    pub report: String,
}

/// Validates `text` (named `source_name`) with the given `event:schema`
/// resources, exactly as the scxml.rs validator component does without
/// packages, plus `profile`'s rules when one is given, and returns the
/// canonical report.
///
/// # Errors
///
/// A static detail when the report violates the canonical contract or its
/// bound (`scxml.report/v3`).
pub fn validate(
    source_name: &str,
    text: &str,
    event_schemas: EventSchemaResources,
    profile: Option<Profile>,
) -> Result<Validation, &'static str> {
    // The bundled official W3C SCXML 1.0 schema set governs the core
    // namespace; no package binds any other namespace or service type.
    let validator = Validator::new(
        BundledSchemaResolver::new(MemorySchemaResolver::new()),
        XsdSchemaBackend,
    )
    .with_schema_mode(SchemaMode::Strict)
    .with_event_schema_resources(event_schemas)
    .with_services(BTreeMap::new());
    let validator = match profile {
        Some(profile) => validator.with_profile(profile),
        None => validator,
    };
    let outcome = validator.validate_source(DOCUMENT_SOURCE, source_name, text);
    let diagnostics = outcome
        .report
        .diagnostics()
        .iter()
        .map(canonical_diagnostic)
        .collect::<Option<Vec<_>>>()
        .ok_or("a validator span does not fit the report")?;
    let artifact = ArtifactEnvelope::validator(source_name, text, diagnostics)
        .map_err(|_| "the validator report violates the canonical report contract")?;
    let bytes = encode_canonical(&artifact)
        .map_err(|_| "the validator report could not be encoded canonically")?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "the validator report could not be encoded canonically")?;
    Ok(Validation {
        artifact,
        report: scxml_event_schema::canonical_json(&value),
    })
}

fn canonical_span(span: scxml_syntax::Span) -> Option<ByteSpan> {
    Some(ByteSpan {
        start: u64::try_from(span.start).ok()?,
        end: u64::try_from(span.end).ok()?,
    })
}

fn canonical_label(label: &scxml_validator_core::Label) -> Option<Label> {
    Some(Label {
        span: canonical_span(label.span)?,
        message: label.message.clone(),
    })
}

/// One validator diagnostic in the canonical reporting model (the mapping
/// the scxml.rs validator component applies).
fn canonical_diagnostic(diagnostic: &scxml_validator_core::Diagnostic) -> Option<Diagnostic> {
    Some(Diagnostic {
        severity: diagnostic.severity,
        code: diagnostic.code.clone(),
        category: diagnostic.category,
        message: diagnostic.message.clone(),
        primary: canonical_label(&diagnostic.primary)?,
        secondary: diagnostic
            .secondary
            .iter()
            .map(canonical_label)
            .collect::<Option<_>>()?,
        notes: diagnostic.notes.clone(),
        help: diagnostic.help.clone(),
        spec_references: diagnostic.spec_references.clone(),
        fixes: diagnostic
            .fixes
            .iter()
            .map(|fix| {
                Some(Fix {
                    message: fix.message.clone(),
                    applicability: fix.applicability,
                    edits: fix
                        .edits
                        .iter()
                        .map(|edit| {
                            Some(TextEdit {
                                span: canonical_span(edit.span)?,
                                replacement: edit.replacement.clone(),
                            })
                        })
                        .collect::<Option<_>>()?,
                })
            })
            .collect::<Option<_>>()?,
    })
}

/// Applies every `machine-applicable` fix of a validator artifact to its
/// source text, in diagnostic order, skipping a fix whose edits overlap (or
/// replace the same empty span as) an already accepted edit, rustfix's rule.
///
/// # Errors
///
/// A static detail when the artifact is not a validator artifact or an edit
/// does not address the source text (the canonical contract rules both out).
pub fn apply_machine_applicable(artifact: &ArtifactEnvelope) -> Result<String, &'static str> {
    let payload = artifact
        .validator_payload()
        .ok_or("the report is not a validator report")?;
    let mut accepted: Vec<(usize, usize, &str)> = Vec::new();
    for fix in payload
        .diagnostics
        .iter()
        .flat_map(|diagnostic| &diagnostic.fixes)
        .filter(|fix| fix.applicability == Applicability::MachineApplicable)
    {
        let mut edits = Vec::with_capacity(fix.edits.len());
        for edit in &fix.edits {
            let start = usize::try_from(edit.span.start)
                .map_err(|_| "a fix edit does not address the source text")?;
            let end = usize::try_from(edit.span.end)
                .map_err(|_| "a fix edit does not address the source text")?;
            edits.push((start, end, edit.replacement.as_str()));
        }
        let overlaps = edits.iter().any(|&(start, end, _)| {
            accepted.iter().any(|&(other_start, other_end, _)| {
                (start < other_end && other_start < end)
                    || (start == other_start && end == other_end)
            })
        });
        if !overlaps {
            accepted.extend(edits);
        }
    }
    accepted.sort_unstable();
    let source = payload.source_text.as_str();
    let mut fixed = String::with_capacity(source.len());
    let mut cursor = 0_usize;
    for (start, end, replacement) in accepted {
        let kept = (start >= cursor && start <= end)
            .then(|| source.get(cursor..start))
            .flatten()
            .ok_or("a fix edit does not address the source text")?;
        if source.get(start..end).is_none() {
            return Err("a fix edit does not address the source text");
        }
        fixed.push_str(kept);
        fixed.push_str(replacement);
        cursor = end;
    }
    fixed.push_str(
        source
            .get(cursor..)
            .ok_or("a fix edit does not address the source text")?,
    );
    Ok(fixed)
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "executable-content-extension",
        path: "../../crates/scxml-wit/wit/scxml",
        // The world's `async func fetch` is lowered synchronously (this
        // component never calls it): `execute` is synchronous executable
        // content.
        async: ["-import:stateforward:scxml/outbound-http@0.2.0#fetch"],
    });

    use std::cell::Cell;

    use exports::stateforward::scxml::executable_content_extension_api::Guest;
    use stateforward::scxml::data_model_host;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, ExecutableContentExtension,
        ExtensionAttribute, ExtensionContent, ExtensionOutcome, SlotReference, TraceContext,
    };

    use super::{Attribute, Content, Executed, Failure, ScxmlHost, Value};

    /// The lifecycle of this instance (`initialize`, `execute` calls,
    /// `shutdown`).
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

    fn assign(location: &str, value: &DataValue) -> Result<Result<(), ()>, ComponentError> {
        match data_model_host::assign(location, value) {
            Ok(()) => Ok(Ok(())),
            Err(error) if recoverable(error.code) => Ok(Err(())),
            Err(error) => Err(error),
        }
    }

    impl ScxmlHost for Host {
        type Error = ComponentError;

        fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, ComponentError> {
            match data_model_host::evaluate(expression) {
                Ok(DataValue::Text(text)) => Ok(Ok(Value::Text(text))),
                Ok(DataValue::JsonObject(text) | DataValue::JsonArray(text)) => {
                    Ok(Ok(Value::Json(text)))
                }
                Ok(_) => Ok(Ok(Value::Other)),
                Err(error) if recoverable(error.code) => Ok(Err(())),
                Err(error) => Err(error),
            }
        }

        fn assign_json_object(
            &mut self,
            location: &str,
            json: &str,
        ) -> Result<Result<(), ()>, ComponentError> {
            assign(location, &DataValue::JsonObject(json.to_owned()))
        }

        fn assign_text(
            &mut self,
            location: &str,
            value: &str,
        ) -> Result<Result<(), ()>, ComponentError> {
            assign(location, &DataValue::Text(value.to_owned()))
        }
    }

    fn attributes(attributes: Vec<ExtensionAttribute>) -> Vec<Attribute> {
        attributes
            .into_iter()
            .map(|attribute| Attribute {
                qualified: attribute.name.namespace_uri.is_some(),
                local_name: attribute.name.local_name,
                value: attribute.value,
            })
            .collect()
    }

    fn content(items: Vec<ExtensionContent>) -> Vec<Content> {
        items
            .into_iter()
            .map(|item| match item {
                ExtensionContent::Text(text) => Content::Text(text),
                ExtensionContent::StartElement(start) => Content::Start {
                    namespace_uri: start.name.namespace_uri,
                    local_name: start.name.local_name,
                    attributes: attributes(start.attributes),
                },
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
                let attributes = attributes(element.attributes);
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
