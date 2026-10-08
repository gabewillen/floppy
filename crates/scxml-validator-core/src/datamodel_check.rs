//! Data-model expression checks.
//!
//! # Contract
//!
//! * `datamodel="null"` checks are data-model-neutral and always run: the
//!   null data model (SCXML 1.0 §B.1) has no value, location, or script
//!   expressions, and its only condition is the `In('stateID')` predicate.
//! * Syntax checks for any other data model run only when the caller supplies
//!   an explicit [`ExpressionSyntaxChecker`] capability whose
//!   [`ExpressionSyntaxChecker::data_model`] equals the machine root's
//!   `datamodel` attribute exactly. The validator never guesses expression
//!   syntax, and it never skips expressions silently: an unrecognized
//!   `datamodel` value is an `LIMIT005` error, and a machine using a
//!   recognized non-null data model gets an `LINT010` note when no checker
//!   for that data model is linked (another data model's checker, or none).
//!   A machine without `datamodel` uses [`DEFAULT_DATA_MODEL`] (SCXML 1.0
//!   §3.2.1: the attribute's default is "platform-specific"; §3.2.2:
//!   "Platforms SHOULD document their default data model").
//! * The capability is parse-only: implementations MUST NOT evaluate source,
//!   read ambient state, or retain it. Issue offsets are UTF-8 byte offsets
//!   into the exact decoded source passed to [`ExpressionSyntaxChecker::check`];
//!   the validator maps them back onto exact source bytes (entity references
//!   and CDATA included) and falls back to the whole value when a mapping is
//!   not exact.
//! * Bounds: at most [`MAX_CHECKED_EXPRESSIONS`] fragments and
//!   [`MAX_CHECKED_EXPRESSION_BYTES`] decoded bytes are sent to the checker
//!   per document, and at most [`MAX_ISSUES_PER_EXPRESSION`] issues are
//!   reported per fragment. Exceeding a document bound emits one fail-closed
//!   `LIMIT004` error and stops further checks.
//! * Records no telemetry of its own; it runs inside the validator's boundary
//!   span, and source text never enters telemetry.

use std::fmt;

use scxml_syntax::{Attribute, Document, Element, Node, Span, is_xml_space, xml_tokens};
use scxml_types::wasm_bound;

use crate::graph::{attribute_removal_span, removal_span};
use crate::semantic::{Rule, is_scxml, machine_roots, nearest, visit_machine};
use crate::{Applicability, Diagnostic, Fix, FixEdit, Label, Report, SCXML_NAMESPACE};

/// Maximum expression fragments sent to a checker per document (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_CHECKED_EXPRESSIONS: usize = wasm_bound(16_384);
/// Maximum decoded expression bytes sent to a checker per document
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_CHECKED_EXPRESSION_BYTES: usize = wasm_bound(4 * 1024 * 1024);
/// Maximum issues reported for one expression fragment.
pub const MAX_ISSUES_PER_EXPRESSION: usize = 4;

/// `datamodel` values this project recognizes: `null` and `ecmascript`
/// (SCXML 1.0 Appendix B.1 and B.2) and `xpath` (the `XPath` data model of the
/// SCXML 1.0 Candidate Recommendation of 2014-03-13, Appendix C.3, which the
/// final Recommendation omits). `ecmascript` and `xpath` have syntax checkers and
/// executable components, and `null` has always-on rules and a component.
pub const KNOWN_DATA_MODELS: [&str; 3] = ["ecmascript", "null", "xpath"];

/// The data model of a document without a `datamodel` attribute. SCXML 1.0
/// §3.2.1 gives the attribute the default "platform-specific", and §3.2.2
/// says "Platforms SHOULD document their default data model". This project
/// uses ECMAScript: the one official IRP document that declares none (test
/// 301) is, by its own comment, "valid only for datamodels that support
/// scripting".
pub const DEFAULT_DATA_MODEL: &str = "ecmascript";

/// The data model `machine` uses and the span that names it: its `datamodel`
/// attribute's value, or [`DEFAULT_DATA_MODEL`] at the start tag when it has
/// none.
pub(crate) fn machine_data_model(machine: &Element) -> (&str, Span) {
    match machine.attribute("datamodel") {
        Some(datamodel) => (datamodel.value.as_str(), datamodel.value_span),
        None => (DEFAULT_DATA_MODEL, machine.start_tag_span),
    }
}

const SPEC_NULL: &str = "SCXML 1.0 §B.1 The Null Data Model";
const SPEC_DATAMODEL: &str = "SCXML 1.0 §5.1 Data Model";
const SPEC_ECMASCRIPT: &str = "SCXML 1.0 §B.2 The ECMAScript Data Model";

/// Syntactic category of one data-model source fragment.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExpressionKind {
    /// A boolean condition (`cond`).
    Condition,
    /// A value expression (`expr`, `*expr`, `<foreach array>`).
    Value,
    /// An assignable location (`location`, `idlocation`, `namelist` items,
    /// `<foreach item/index>`).
    Location,
    /// A `<script>` body.
    Script,
}

/// One syntax problem reported by a checker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxIssue {
    /// Inclusive UTF-8 byte offset into the checked source.
    pub start: usize,
    /// Exclusive UTF-8 byte offset into the checked source (`start <= end`).
    pub end: usize,
    /// Human-readable parser message.
    pub message: String,
}

/// Result of checking one fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionCheck {
    /// The fragment parses as the requested kind.
    Valid,
    /// The fragment does not parse; at least one issue is present.
    Invalid(Vec<SyntaxIssue>),
    /// The checker declined the fragment. A checker declines only what its
    /// data model also rejects at evaluation (for example, a fragment over
    /// the data model's size or nesting bound) or when its own answer cannot
    /// be trusted; either way the validator fails closed with the `LIMIT003`
    /// error, so `compile` never lowers such a fragment.
    NotChecked,
}

pub use effects::{
    EffectCause, EffectCheck, EffectFinding, EffectFragment, EffectKind, EffectRole, EffectSpan,
};
pub use scxml_types::ExternalScripts;

/// Explicit, optional, parse-only expression syntax capability.
///
/// Implementations are pure: no evaluation, no ambient I/O, no retained
/// source. The WIT contract for component providers is
/// `stateforward:scxml/data-model-api.check-syntax` and
/// `stateforward:scxml/data-model-api.check-effects`.
pub trait ExpressionSyntaxChecker: fmt::Debug + Send + Sync {
    /// Exact `datamodel` attribute value this checker understands.
    fn data_model(&self) -> &str;

    /// Checks one decoded source fragment.
    fn check(&self, kind: ExpressionKind, source: &str) -> ExpressionCheck;

    /// Analyzes the side effects of one machine's `cond` and `<log expr>`
    /// fragments over every fragment that parses (the WIT
    /// `check-effects`); `scripts` is [`ExternalScripts::Present`] when the
    /// machine has a `<script src>`. A data model whose expressions have no
    /// side effects returns no findings.
    fn check_effects(
        &self,
        fragments: &[EffectFragment<'_>],
        scripts: ExternalScripts,
    ) -> EffectCheck;
}

// ---------------------------------------------------------------------------
// Expression sites
// ---------------------------------------------------------------------------

/// One expression-bearing source location.
struct Site<'a> {
    kind: ExpressionKind,
    /// Human label for messages (`cond`, `expr`, `<script>`, ...).
    what: String,
    /// Decoded source text.
    source: String,
    /// Mapping from decoded byte offsets (0..=len) to source byte offsets,
    /// or `None` when no exact mapping exists.
    offsets: Option<Vec<usize>>,
    /// Whole lexical value span.
    value_span: Span,
    /// Attribute, when the site is an attribute.
    attribute: Option<&'a Attribute>,
    /// Element owning the site.
    element: &'a Element,
}

fn attribute_kind(element: &Element, name: &str) -> Option<ExpressionKind> {
    let local = element.name.local_name.as_str();
    match name {
        "cond" if matches!(local, "transition" | "if" | "elseif") => {
            Some(ExpressionKind::Condition)
        }
        // Only attributes SCXML 1.0 defines as value expressions; an unknown
        // element (a schema error such as `<lgo expr>`) is not a site, so
        // its misspelling is not also reported as an expression finding.
        "expr" if matches!(local, "data" | "assign" | "log" | "param" | "content") => {
            Some(ExpressionKind::Value)
        }
        "eventexpr" | "targetexpr" | "delayexpr" if local == "send" => Some(ExpressionKind::Value),
        "typeexpr" if matches!(local, "send" | "invoke") => Some(ExpressionKind::Value),
        "srcexpr" if local == "invoke" => Some(ExpressionKind::Value),
        "sendidexpr" if local == "cancel" => Some(ExpressionKind::Value),
        "array" if local == "foreach" => Some(ExpressionKind::Value),
        "location" if matches!(local, "assign" | "param") => Some(ExpressionKind::Location),
        "idlocation" if matches!(local, "send" | "invoke") => Some(ExpressionKind::Location),
        "item" | "index" if local == "foreach" => Some(ExpressionKind::Location),
        "namelist" if matches!(local, "send" | "invoke") => Some(ExpressionKind::Location),
        _ => None,
    }
}

fn collect_sites<'a>(document: &Document, machine: &'a Element) -> Vec<Site<'a>> {
    let mut sites = Vec::new();
    visit_machine(machine, &mut |element| {
        if element.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
            return;
        }
        for attribute in &element.attributes {
            let name = attribute.name.local_name.as_str();
            let kind = match attribute.name.namespace_uri.as_deref() {
                None => attribute_kind(element, name),
                // `event:schemaexpr` is a value expression on the elements
                // that may carry it (`EVENT001` reports it elsewhere).
                Some(scxml_types::EVENT_NAMESPACE_URI)
                    if name == scxml_event_schema::SCHEMA_EXPR_ATTRIBUTE
                        && scxml_event_schema::is_owner_element(&element.name.local_name) =>
                {
                    Some(ExpressionKind::Value)
                }
                Some(_) => None,
            };
            let Some(kind) = kind else {
                continue;
            };
            if name == "namelist" {
                push_namelist_sites(document, attribute, element, &mut sites);
                continue;
            }
            let offsets = document
                .source
                .slice(attribute.value_span)
                .and_then(|raw| decode_offsets(raw, attribute.value_span.start))
                .filter(|(decoded, _)| *decoded == attribute.value)
                .map(|(_, offsets)| offsets);
            sites.push(Site {
                kind,
                what: name.to_owned(),
                source: attribute.value.clone(),
                offsets,
                value_span: attribute.value_span,
                attribute: Some(attribute),
                element,
            });
        }
        if is_scxml(element, "script")
            && element.attribute("src").is_none()
            && let Some(site) = script_site(document, element)
        {
            sites.push(site);
        }
    });
    sites
}

/// Splits a decoded `namelist` into one location site per token, keeping
/// exact source offsets through character references.
fn push_namelist_sites<'a>(
    document: &Document,
    attribute: &'a Attribute,
    element: &'a Element,
    sites: &mut Vec<Site<'a>>,
) {
    let decoded = document
        .source
        .slice(attribute.value_span)
        .and_then(|raw| decode_offsets(raw, attribute.value_span.start))
        .filter(|(decoded, _)| *decoded == attribute.value);
    let Some((decoded, offsets)) = decoded else {
        for token in xml_tokens(&attribute.value) {
            sites.push(Site {
                kind: ExpressionKind::Location,
                what: "namelist".to_owned(),
                source: token.to_owned(),
                offsets: None,
                value_span: attribute.value_span,
                attribute: Some(attribute),
                element,
            });
        }
        return;
    };
    let mut start: Option<usize> = None;
    let mut boundaries: Vec<(usize, usize)> = Vec::new();
    for (offset, character) in decoded.char_indices() {
        if is_xml_space(character) {
            if let Some(begin) = start.take() {
                boundaries.push((begin, offset));
            }
        } else if start.is_none() {
            start = Some(offset);
        }
    }
    if let Some(begin) = start {
        boundaries.push((begin, decoded.len()));
    }
    for (begin, end) in boundaries {
        let (Some(token), Some(map)) = (decoded.get(begin..end), offsets.get(begin..=end)) else {
            continue;
        };
        let (Some(first), Some(last)) = (map.first(), map.last()) else {
            continue;
        };
        sites.push(Site {
            kind: ExpressionKind::Location,
            what: "namelist".to_owned(),
            source: token.to_owned(),
            offsets: Some(map.to_vec()),
            value_span: Span::new(attribute.value_span.source, *first, *last),
            attribute: Some(attribute),
            element,
        });
    }
}

fn script_site<'a>(document: &Document, element: &'a Element) -> Option<Site<'a>> {
    let mut source = String::new();
    let mut expected = String::new();
    let mut offsets: Option<Vec<usize>> = Some(Vec::new());
    let mut value_span: Option<Span> = None;
    for child in &element.children {
        let Node::Text(text) = child else {
            continue;
        };
        expected.push_str(&text.value);
        value_span = Some(value_span.map_or(text.span, |span| {
            Span::new(span.source, span.start, text.span.end)
        }));
        let raw = document.source.slice(text.span).unwrap_or("");
        let decoded = raw
            .strip_prefix("<![CDATA[")
            .and_then(|inner| inner.strip_suffix("]]>"))
            .map_or_else(
                || decode_offsets(raw, text.span.start),
                |inner| {
                    let base = text.span.start + "<![CDATA[".len();
                    Some((inner.to_owned(), (base..=base + inner.len()).collect()))
                },
            );
        match (decoded, offsets.as_mut()) {
            (Some((decoded, mut map)), Some(all)) => {
                source.push_str(&decoded);
                all.pop();
                all.append(&mut map);
            }
            _ => offsets = None,
        }
    }
    if scxml_syntax::is_xml_blank(&expected) {
        return None;
    }
    if source != expected {
        offsets = None;
    }
    Some(Site {
        kind: ExpressionKind::Script,
        what: "<script>".to_owned(),
        source: expected,
        offsets,
        value_span: value_span.unwrap_or(element.span),
        attribute: None,
        element,
    })
}

/// Decodes XML predefined and character references in a raw attribute or
/// text value, returning the decoded text and the source offset of every
/// decoded byte boundary (length `decoded.len() + 1`).
pub(crate) fn decode_offsets(raw: &str, base: usize) -> Option<(String, Vec<usize>)> {
    let mut decoded = String::with_capacity(raw.len());
    let mut offsets = Vec::with_capacity(raw.len() + 1);
    let mut index = 0;
    while index < raw.len() {
        let rest = raw.get(index..)?;
        let (character, consumed) = if rest.starts_with('&') {
            let end = rest.find(';')?;
            let name = rest.get(1..end)?;
            let character = match name {
                "lt" => '<',
                "gt" => '>',
                "amp" => '&',
                "quot" => '"',
                "apos" => '\'',
                _ => {
                    let number = name.strip_prefix('#')?;
                    let value = if let Some(hex) = number
                        .strip_prefix('x')
                        .or_else(|| number.strip_prefix('X'))
                    {
                        u32::from_str_radix(hex, 16).ok()?
                    } else {
                        number.parse::<u32>().ok()?
                    };
                    char::from_u32(value)?
                }
            };
            (character, end + 1)
        } else {
            let character = rest.chars().next()?;
            (character, character.len_utf8())
        };
        for _ in 0..character.len_utf8() {
            offsets.push(base + index);
        }
        decoded.push(character);
        index += consumed;
    }
    offsets.push(base + raw.len());
    Some((decoded, offsets))
}

impl Site<'_> {
    /// Maps the decoded range `start..end` onto exact source bytes (the
    /// whole value when no exact mapping exists).
    fn span_of(&self, start: usize, end: usize) -> Span {
        self.issue_span(&SyntaxIssue {
            start,
            end,
            message: String::new(),
        })
    }

    /// Maps a checker issue onto exact source bytes.
    fn issue_span(&self, issue: &SyntaxIssue) -> Span {
        let Some(offsets) = &self.offsets else {
            return self.value_span;
        };
        let length = self.source.len();
        let floor = |offset: usize| {
            let mut offset = offset.min(length);
            while offset > 0 && !self.source.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        let mut start = floor(issue.start);
        let mut end = floor(issue.end.max(issue.start));
        if start == end {
            if let Some(character) = self
                .source
                .get(start..)
                .and_then(|rest| rest.chars().next())
            {
                end = start + character.len_utf8();
            } else if let Some(character) = self
                .source
                .get(..start)
                .and_then(|head| head.chars().next_back())
            {
                start -= character.len_utf8();
            }
        }
        match (offsets.get(start), offsets.get(end)) {
            (Some(start), Some(end)) if start <= end => {
                Span::new(self.value_span.source, *start, *end)
            }
            _ => self.value_span,
        }
    }
}

// ---------------------------------------------------------------------------
// Data-model declaration (always on)
// ---------------------------------------------------------------------------

/// LIMIT005 (unrecognized `datamodel`).
pub(crate) struct DataModelDeclarationRule;

impl Rule for DataModelDeclarationRule {
    fn code(&self) -> &'static str {
        "LIMIT005"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            if let Some(datamodel) = machine.attribute("datamodel")
                && !KNOWN_DATA_MODELS.contains(&datamodel.value.as_str())
            {
                report.push(unknown_data_model_diagnostic(self.code(), datamodel));
            }
        }
    }
}

fn unknown_data_model_diagnostic(code: &str, datamodel: &Attribute) -> Diagnostic {
    let suggestion = KNOWN_DATA_MODELS
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(&datamodel.value))
        .or_else(|| nearest(&datamodel.value, KNOWN_DATA_MODELS.iter().copied()));
    let diagnostic = Diagnostic::error(
        code,
        format!("unknown data model `{}`", datamodel.value),
        datamodel.value_span,
    )
    .with_primary_message("not a data model this validator recognizes")
    .with_note("SCXML 1.0 §5.1 defines the `ecmascript`, `null`, and `xpath` data models; an unrecognized data model fails closed because none of this machine's expressions can be evaluated or syntax-checked")
    .with_spec_reference(SPEC_DATAMODEL);
    match suggestion {
        Some(suggestion) => diagnostic
            .with_help(format!("did you mean `{suggestion}`?"))
            .with_fix(Fix::replace(
                format!("use the `{suggestion}` data model"),
                Applicability::MaybeIncorrect,
                datamodel.value_span,
                suggestion,
            )),
        None => diagnostic.with_help(
            "use `ecmascript` for JavaScript expressions, or `null` for a machine without data",
        ),
    }
}

fn unchecked_data_model_note(
    datamodel: &str,
    span: Span,
    checker: Option<&str>,
    first: &Site<'_>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::warning(
        "LINT010",
        format!("expressions of this `{datamodel}` machine were not syntax-checked"),
        span,
    )
    .with_primary_message(format!(
        "no syntax checker for the `{datamodel}` data model is linked"
    ))
    .with_secondary(Label::new(first.value_span).with_message(format!(
        "first unchecked expression: `{}`",
        first.what
    )))
    .with_note(match checker {
        Some(checker) => format!(
            "this validator run links a syntax checker only for the `{checker}` data model; nothing is claimed about these expressions"
        ),
        None => "this validator run links no expression syntax checker; nothing is claimed about these expressions".to_owned(),
    })
    .with_help("review the expressions manually, or validate with a host that links a syntax checker for this data model")
    .with_spec_reference(SPEC_DATAMODEL);
    diagnostic.severity = crate::Severity::Note;
    diagnostic.category = crate::DiagnosticCategory::Lint;
    diagnostic
}

// ---------------------------------------------------------------------------
// Null data model (always on)
// ---------------------------------------------------------------------------

/// SCXML037 (error): an element SCXML 1.0 §B.1.7 does not support under
/// `datamodel="null"`; SCXML079 (warning): an expression there, which has no
/// expression language to be written in and raises `error.execution` when
/// evaluated (§5.9.4).
pub(crate) struct NullDataModelRule;

impl Rule for NullDataModelRule {
    fn code(&self) -> &'static str {
        "SCXML037"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            let Some(datamodel) = machine.attribute("datamodel") else {
                continue;
            };
            if datamodel.value != "null" {
                continue;
            }
            let unsupported = unsupported_null_elements(machine);
            for element in &unsupported {
                report.push(unsupported_null_element_diagnostic(
                    self.code(),
                    document,
                    datamodel,
                    element,
                ));
            }
            let mut reported_namelists: Vec<Span> = Vec::new();
            let mut states: Option<in_predicate::StateIds> = None;
            // One element is one mistake: its expression sites are grouped
            // (in document order) into a single finding.
            let mut groups: Vec<Vec<Site<'_>>> = Vec::new();
            for site in collect_sites(document, machine) {
                // An unsupported element is already the one finding for
                // everything it holds.
                if unsupported
                    .iter()
                    .any(|element| contains(element.span, site.element.start_tag_span))
                {
                    continue;
                }
                if let Some(attribute) = site.attribute
                    && attribute.name.local_name == "namelist"
                {
                    if reported_namelists.contains(&attribute.span) {
                        continue;
                    }
                    reported_namelists.push(attribute.span);
                }
                if site.kind == ExpressionKind::Condition {
                    if is_null_in_predicate(&site.source) {
                        continue;
                    }
                    if let Some(diagnostic) = in_predicate::unquoted_null_in(
                        document,
                        datamodel,
                        states.get_or_insert_with(|| in_predicate::StateIds::of(machine)),
                        &site,
                    ) {
                        report.push(diagnostic);
                        continue;
                    }
                }
                match groups.last_mut() {
                    Some(group)
                        if group.first().is_some_and(|first| {
                            first.element.start_tag_span == site.element.start_tag_span
                        }) =>
                    {
                        group.push(site);
                    }
                    _ => groups.push(vec![site]),
                }
            }
            // An expression has a run-time outcome (SCXML 1.0 §5.9.4:
            // `error.execution` when an ill-formed expression is evaluated),
            // so it is the `SCXML079` warning; only the elements §B.1.7 does
            // not support are the `SCXML037` error.
            for group in &groups {
                if let Some(diagnostic) = null_diagnostic("SCXML079", document, datamodel, group) {
                    report.push(diagnostic);
                }
            }
        }
    }
}

/// SCXML 1.0 §B.1.7: "The <foreach> element and the elements defined in 5
/// Data Model and Data Manipulation are not supported in the Null Data
/// Model." Section 5 defines `<datamodel>`, `<data>`, `<assign>`,
/// `<donedata>`, `<content>`, `<param>`, and `<script>`.
const NULL_UNSUPPORTED_ELEMENTS: [&str; 8] = [
    "datamodel",
    "data",
    "assign",
    "donedata",
    "content",
    "param",
    "script",
    "foreach",
];

/// The outermost elements of `machine` (not of the machines nested in it)
/// that the null data model does not support, in document order.
fn unsupported_null_elements(machine: &Element) -> Vec<&Element> {
    let mut found = Vec::new();
    let mut pending = vec![machine];
    while let Some(element) = pending.pop() {
        for child in element.children.iter().rev() {
            let Node::Element(child) = child else {
                continue;
            };
            if is_scxml(child, "scxml") {
                continue;
            }
            if NULL_UNSUPPORTED_ELEMENTS
                .iter()
                .any(|name| is_scxml(child, name))
            {
                found.push(child);
            } else {
                pending.push(child);
            }
        }
    }
    found.sort_by_key(|element| element.span.start);
    found
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.source == inner.source && outer.start <= inner.start && inner.end <= outer.end
}

/// `SCXML037` for an element SCXML 1.0 §B.1.7 does not support in the null
/// data model. Appendix A.1: a conformant document "MUST conform to any
/// additional syntactic constraints that are defined for the data model that
/// it has chosen." A `<content>` that is an `<invoke>`'s only source is not
/// offered for removal, which would leave the `<invoke>` without one.
fn unsupported_null_element_diagnostic(
    code: &str,
    document: &Document,
    datamodel: &Attribute,
    element: &Element,
) -> Diagnostic {
    let name = element.name.local_name.as_str();
    let mut diagnostic = Diagnostic::error(
        code,
        format!("the null data model does not support `<{name}>`"),
        element.start_tag_span,
    )
    .with_primary_message("not supported in the null data model")
    .with_secondary(
        Label::new(datamodel.value_span).with_message("the null data model is selected here"),
    )
    .with_note("SCXML 1.0 §B.1.7: the `<foreach>` element and the elements of section 5 (`<datamodel>`, `<data>`, `<assign>`, `<donedata>`, `<content>`, `<param>`, `<script>`) are not supported in the null data model")
    .with_help(format!(
        "remove the `<{name}>`, or select a data model that supports it, such as `datamodel=\"ecmascript\"`"
    ))
    .with_spec_reference(SPEC_NULL);
    if name != "content" {
        diagnostic = diagnostic.with_fix(Fix::replace(
            format!("remove the `<{name}>` element"),
            Applicability::MaybeIncorrect,
            removal_span(document, element.span),
            "",
        ));
    }
    diagnostic.with_fix(Fix::replace(
        "select the ECMAScript data model",
        Applicability::MaybeIncorrect,
        datamodel.value_span,
        "ecmascript",
    ))
}

/// Whether `source` is exactly the null data model's `In('stateID')`
/// predicate, in the grammar the null data-model component evaluates
/// (`components/scxml-data-model-null`): optional XML white space, `In`, `(`,
/// one single- or double-quoted, non-empty state ID with no quote of the
/// same kind, backslash, or XML white space inside it, and `)`. The component rejects
/// an unquoted ID at run time, so `In(s)` is not the predicate.
fn is_null_in_predicate(source: &str) -> bool {
    let Some(argument) = source
        .trim_matches(is_xml_space)
        .strip_prefix("In")
        .map(|rest| rest.trim_start_matches(is_xml_space))
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let argument = argument.trim_matches(is_xml_space);
    let state_id = argument
        .strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
        .filter(|inner| !inner.contains('\''))
        .or_else(|| {
            argument
                .strip_prefix('"')
                .and_then(|inner| inner.strip_suffix('"'))
                .filter(|inner| !inner.contains('"'))
        });
    state_id.is_some_and(|state_id| {
        !state_id.is_empty() && !state_id.contains('\\') && !state_id.contains(is_xml_space)
    })
}

/// Whether deleting `attribute` from `element` leaves the element valid
/// against the SCXML schema and the SCXML007/SCXML008/SCXML052 rules. Required
/// attributes, and the only member of a required choice, are never removed on
/// their own: removing them would trade one diagnostic for another.
fn attribute_removal_is_safe(element: &Element, attribute: &str) -> bool {
    let has = |name: &str| element.attribute(name).is_some();
    !match (element.name.local_name.as_str(), attribute) {
        ("assign", "location" | "expr")
        | ("foreach", "array" | "item")
        | ("if" | "elseif", "cond") => true,
        ("param", "expr") => !has("location"),
        ("param", "location") => !has("expr"),
        ("cancel", "sendidexpr") => !has("sendid"),
        ("send", "eventexpr") => !has("event"),
        _ => false,
    }
}

/// Message and primary span for the primary null-data-model site.
fn null_message(site: &Site<'_>) -> (String, Span) {
    let element = site.element;
    match site.attribute {
        Some(attribute) => (
            if site.kind == ExpressionKind::Condition {
                format!(
                    "`cond` on `<{}>` is not an `In('stateID')` predicate, the only condition the null data model supports",
                    element.name.local_name
                )
            } else {
                format!(
                    "the null data model defines no {} language, but `<{}>` uses `{}`",
                    if site.kind == ExpressionKind::Value {
                        "value-expression"
                    } else {
                        "location-expression"
                    },
                    element.name.local_name,
                    attribute.name.local_name
                )
            },
            attribute.span,
        ),
        None => (
            "the null data model cannot run `<script>`".to_owned(),
            element.start_tag_span,
        ),
    }
}

/// Removes the flagged attributes when that leaves a valid element, and the
/// whole element otherwise.
fn null_removal_fix(
    document: &Document,
    element: &Element,
    what: &str,
    removable_attributes: Option<Vec<&Attribute>>,
) -> Fix {
    match removable_attributes {
        Some(attributes) => Fix {
            message: if attributes.len() == 1 {
                format!("remove `{what}`")
            } else {
                format!(
                    "remove the expressions from `<{}>`",
                    element.name.local_name
                )
            },
            applicability: Applicability::MaybeIncorrect,
            edits: attributes
                .iter()
                .map(|attribute| FixEdit {
                    span: attribute_removal_span(document, attribute.span),
                    replacement: String::new(),
                })
                .collect(),
        },
        None => Fix::replace(
            format!("remove the `<{}>` element", element.name.local_name),
            Applicability::MaybeIncorrect,
            removal_span(document, element.span),
            "",
        ),
    }
}

fn null_diagnostic(
    code: &str,
    document: &Document,
    datamodel: &Attribute,
    group: &[Site<'_>],
) -> Option<Diagnostic> {
    // A location or condition site is primary over a value expression.
    let site = group
        .iter()
        .find(|site| site.kind != ExpressionKind::Value)
        .or_else(|| group.first())?;
    let element = site.element;
    let (message, primary) = null_message(site);
    let what = site.what.as_str();
    // SCXML 1.0 §B.1.3-§B.1.5: "There is no location expression language.",
    // "There is no value expression language.", "There is no scripting
    // language."; §B.1.2: conditions are only the In predicate.
    let mut diagnostic = Diagnostic::warning(code, message, primary)
        .with_primary_message(format!("`{what}` requires a data model"))
        .with_secondary(
            Label::new(datamodel.value_span).with_message("the null data model is selected here"),
        );
    for other in group {
        if std::ptr::eq(other, site) {
            continue;
        }
        let span = other
            .attribute
            .map_or(other.value_span, |attribute| attribute.span);
        diagnostic = diagnostic.with_secondary(
            Label::new(span).with_message(format!("`{}` also requires a data model", other.what)),
        );
    }
    let conditions_only = group
        .iter()
        .all(|site| site.kind == ExpressionKind::Condition);
    let removable_attributes: Option<Vec<&Attribute>> = group
        .iter()
        .map(|site| {
            site.attribute
                .filter(|attribute| attribute_removal_is_safe(element, &attribute.name.local_name))
        })
        .collect();
    // Removing every flagged attribute must still leave a valid element, so a
    // `<send>` losing both `eventexpr` and its only event source is removed whole.
    let removable_attributes = removable_attributes.filter(|attributes| {
        attributes.len() < 2
            || attributes
                .iter()
                .all(|attribute| attribute.name.local_name != "eventexpr")
    });
    diagnostic = diagnostic
        .with_note("the null data model has no data, value expressions, location expressions, or scripts; conditions may only be `In('stateID')`, with the state ID quoted")
        .with_help(if conditions_only {
            "rewrite the condition as `In('stateID')`, or select a data model that supports it, such as `datamodel=\"ecmascript\"`".to_owned()
        } else if removable_attributes.is_some() {
            "remove the expression, or select a data model that supports expressions, such as `datamodel=\"ecmascript\"`".to_owned()
        } else {
            format!(
                "`<{}>` cannot work without a data model: remove the element, or select a data model that supports expressions, such as `datamodel=\"ecmascript\"`",
                element.name.local_name
            )
        })
        .with_spec_reference(SPEC_NULL);
    if !conditions_only {
        diagnostic = diagnostic.with_fix(null_removal_fix(
            document,
            element,
            what,
            removable_attributes,
        ));
    }
    Some(diagnostic.with_fix(Fix::replace(
        "select the ECMAScript data model",
        Applicability::MaybeIncorrect,
        datamodel.value_span,
        "ecmascript",
    )))
}

// ---------------------------------------------------------------------------
// Capability-backed syntax checks
// ---------------------------------------------------------------------------

/// Checks every expression of each machine whose `datamodel` matches the
/// supplied checker.
///
/// Machines without `datamodel` are left to the always-on `LINT010`
/// warning; a machine using another recognized non-null data model gets an
/// `LINT010` note that its expressions went unchecked.
#[must_use]
pub fn validate_expressions(document: &Document, checker: &dyn ExpressionSyntaxChecker) -> Report {
    validate_expressions_with(document, &[checker])
}

/// Checks every expression of each machine with the checker whose
/// [`ExpressionSyntaxChecker::data_model`] equals that machine's
/// `datamodel` exactly (each machine, including inline invoked documents,
/// selects its own); a machine whose recognized non-null data model has no
/// linked checker gets an `LINT010` note. The first checker listed for a
/// data model wins.
#[must_use]
pub fn validate_expressions_with(
    document: &Document,
    checkers: &[&dyn ExpressionSyntaxChecker],
) -> Report {
    let document = &*crate::schema_values::schema_values(document);
    let mut report = Report::new();
    let mut fragments = 0_usize;
    let mut bytes = 0_usize;
    'machines: for machine in machine_roots(&document.root) {
        let (datamodel, span) = machine_data_model(machine);
        let Some(checker) = checkers
            .iter()
            .find(|checker| checker.data_model() == datamodel)
        else {
            if datamodel != "null"
                && KNOWN_DATA_MODELS.contains(&datamodel)
                && let Some(first) = collect_sites(document, machine).first()
            {
                let linked = checkers
                    .iter()
                    .map(|checker| checker.data_model())
                    .collect::<Vec<_>>()
                    .join("`, `");
                report.push(unchecked_data_model_note(
                    datamodel,
                    span,
                    (!linked.is_empty()).then_some(linked.as_str()),
                    first,
                ));
            }
            continue;
        };
        let xpath_scope = (checker.data_model() == xpath::DATA_MODEL)
            .then(|| xpath::references::Scope::of(machine));
        let sites = collect_sites(document, machine);
        // The fragments that parse (as written, or with their reported
        // habit repairs applied), for the machine's side-effect analysis.
        let mut valid = std::collections::BTreeSet::new();
        let mut repaired = std::collections::BTreeMap::new();
        for (index, site) in sites.iter().enumerate() {
            fragments = fragments.saturating_add(1);
            bytes = bytes.saturating_add(site.source.len());
            if scxml_types::exceeds_wasm_bound(fragments, MAX_CHECKED_EXPRESSIONS)
                || scxml_types::exceeds_wasm_bound(bytes, MAX_CHECKED_EXPRESSION_BYTES)
            {
                report.push(too_many_expressions(&site.value_span, datamodel));
                break 'machines;
            }
            if site.kind != ExpressionKind::Script && scxml_syntax::is_xml_blank(&site.source) {
                // An empty required attribute the compiler rejects is the
                // `SCXML066` error instead.
                if !lowering_rejects_empty(site) {
                    report.push(empty_expression_diagnostic(document, site));
                }
                continue;
            }
            match checker.check(site.kind, &site.source) {
                ExpressionCheck::Valid => {
                    valid.insert(index);
                    for finding in
                        valid_fragment_lints(document, *checker, site, xpath_scope.as_ref())
                    {
                        report.push(finding);
                    }
                }
                ExpressionCheck::NotChecked => {
                    report.push(not_checked_diagnostic(checker.data_model(), site));
                }
                ExpressionCheck::Invalid(issues) => {
                    if let Some((findings, composed)) =
                        habit_diagnostics(document, *checker, site, xpath_scope.as_ref())
                    {
                        for finding in findings {
                            report.push(finding);
                        }
                        repaired.insert(index, composed);
                        continue;
                    }
                    let mut issues = issues;
                    issues.sort_by(|left, right| {
                        (left.start, left.end, &left.message).cmp(&(
                            right.start,
                            right.end,
                            &right.message,
                        ))
                    });
                    issues.dedup();
                    for issue in issues.iter().take(MAX_ISSUES_PER_EXPRESSION) {
                        report.push(issue_diagnostic(document, *checker, site, issue));
                    }
                }
            }
        }
        let parsed: Vec<&Site<'_>> = sites
            .iter()
            .enumerate()
            .filter_map(|(index, site)| {
                if valid.contains(&index) {
                    Some(site)
                } else {
                    repaired.get(&index)
                }
            })
            .collect();
        effects::report_effects(document, *checker, machine, &parsed, &mut report);
    }
    report.sort();
    report
}

/// `LIMIT004`: the document exceeds the expression-checking bound at `span`.
fn too_many_expressions(span: &Span, datamodel: &str) -> Diagnostic {
    Diagnostic::error(
                        "LIMIT004",
                        "too many expressions for bounded syntax checking",
                        *span,
                    )
                    .with_primary_message("expression checking stopped here")
                    .with_note(format!(
                        "the validator syntax-checks at most {MAX_CHECKED_EXPRESSIONS} expressions and {MAX_CHECKED_EXPRESSION_BYTES} expression bytes per document; this and later expressions were not checked"
                    ))
                    .with_help("split the document into smaller machines, or move large scripts into fewer `<script>` elements")
                    .with_spec_reference(if datamodel == "ecmascript" {
                        SPEC_ECMASCRIPT
                    } else {
                        "SCXML 1.0 §5.9 Expressions"
                    })
}

/// Lints of a fragment that parses: `LINT017` (ECMAScript) and, with the
/// `XPath` scope, the reference and habit lints; every fragment repair is
/// kept only when the repaired fragment still parses. (An assignment in an
/// ECMAScript condition is the side-effect error `SCXML074`.)
fn valid_fragment_lints(
    document: &Document,
    checker: &dyn ExpressionSyntaxChecker,
    site: &Site<'_>,
    xpath_scope: Option<&xpath::references::Scope>,
) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    if site.kind == ExpressionKind::Condition
        && checker.data_model() == "ecmascript"
        && let Some(offset) = arrow_in_condition(&site.source)
    {
        let mut finding = arrow_in_condition_diagnostic(document, site, offset);
        finding
            .fixes
            .retain(|fix| repair_parses(document, checker, site, fix));
        findings.push(finding);
    }
    if let Some(scope) = xpath_scope {
        for mut finding in xpath::references::lint(document, site, scope) {
            finding
                .fixes
                .retain(|fix| repair_parses(document, checker, site, fix));
            findings.push(finding);
        }
    }
    findings
}

/// Reports, for a validator run with no expression checker linked, every
/// machine whose recognized non-null data model has expressions: they were
/// not syntax-checked, and the report must say so rather than stay silent.
#[must_use]
pub fn report_unchecked_expressions(document: &Document) -> Report {
    let document = &*crate::schema_values::schema_values(document);
    let mut report = Report::new();
    for machine in machine_roots(&document.root) {
        let (datamodel, span) = machine_data_model(machine);
        if datamodel != "null"
            && KNOWN_DATA_MODELS.contains(&datamodel)
            && let Some(first) = collect_sites(document, machine).first()
        {
            report.push(unchecked_data_model_note(datamodel, span, None, first));
        }
    }
    report
}

/// Project-owned wording for one checker message, with repair help and an
/// optional targeted repair.
#[derive(Default)]
struct IssueWording {
    message: String,
    help: String,
    fix: Option<Fix>,
    /// Further alternative repairs, offered after [`Self::fix`].
    alternatives: Vec<Fix>,
    /// Exact decoded range of the responsible token, when the checker's
    /// position covers only part of it.
    token: Option<(usize, usize)>,
    /// Related decoded ranges with their label (for example the opening
    /// delimiter an error leaves unclosed).
    related: Vec<(usize, usize, String)>,
    /// Replaces the default note for the expression kind.
    note: Option<String>,
    /// Replaces the default primary label.
    label: Option<String>,
}

/// Rewrites parser-internal wording (for example Boa's "primary expression"
/// grammar names) into diagnostic-grade text with concrete help.
fn word_issue(
    document: &Document,
    data_model: &str,
    site: &Site<'_>,
    issue: &SyntaxIssue,
) -> IssueWording {
    if data_model == xpath::DATA_MODEL {
        return xpath::word(document, site, issue);
    }
    let raw = issue.message.as_str();
    let generic = format!("fix the {data_model} syntax at the marked position");
    if let Some(rest) = raw.strip_prefix("unexpected `")
        && let Some((token, context)) = rest.split_once('`')
    {
        return unexpected_token_wording(site, issue, token, context, &generic);
    }
    if let Some(rest) = raw.strip_prefix("expected ")
        && let Some((expected, found)) = rest.rsplit_once(", found `")
    {
        return expected_token_wording(
            document,
            data_model,
            site,
            issue,
            expected,
            found.trim_end_matches('`'),
        );
    }
    if raw == "unexpected end of input" {
        return end_of_input_wording(site);
    }
    if raw.starts_with("Invalid left-hand side") {
        return invalid_assignment_wording(site, issue);
    }
    if raw.eq_ignore_ascii_case("unterminated string literal")
        || raw.eq_ignore_ascii_case("unterminated template literal")
    {
        return unterminated_string_wording(document, site, issue);
    }
    if let Some(wording) = hash_wording(document, site, issue, raw) {
        return wording;
    }
    if let Some(character) = raw
        .strip_prefix("unexpected '")
        .and_then(|rest| rest.strip_suffix('\''))
        .and_then(|quoted| {
            let mut characters = quoted.chars();
            characters.next().filter(|_| characters.next().is_none())
        })
    {
        return unexpected_character_wording(document, site, issue, character);
    }
    other_message_wording(document, site, issue, raw, generic)
}

/// Boa's "unexpected `tok`: context" message.
fn unexpected_token_wording(
    site: &Site<'_>,
    issue: &SyntaxIssue,
    token: &str,
    context: &str,
    generic: &str,
) -> IssueWording {
    let context = context.trim_start_matches(':').trim();
    let message = if context.is_empty() {
        format!("unexpected `{token}`")
    } else if context.contains("expression") {
        format!("unexpected `{token}` where an operand was expected")
    } else if let Some(expected) = context.strip_prefix("expected ") {
        format!("unexpected `{token}`: expected {expected}")
    } else {
        format!("unexpected `{token}` in {context}")
    };
    if token == "="
        && let Some(repair) = stray_equals_repair(site, issue.start)
    {
        return IssueWording {
            message,
            help: repair.help,
            fix: repair.fix,
            token: None,
            ..IssueWording::default()
        };
    }
    let help = match token {
        "=" => "`=` assigns; write `==` or `===` to compare".to_owned(),
        ")" | "]" | "}" => format!("remove the unmatched `{token}` or add its opening bracket"),
        "if" | "for" | "while" | "do" | "switch" | "try" | "return" | "throw" | "break"
        | "continue"
            if site.kind != ExpressionKind::Script =>
        {
            format!(
                "`{token}` starts a statement, but {} `{}` must be an expression: use the conditional operator `test ? a : b`, or move the statement into a `<script>`",
                article(&site.what),
                site.what
            )
        }
        _ if context.contains("comma") => {
            format!("separate the elements with `,` before `{token}`")
        }
        _ => format!("{generic}: `{token}` cannot appear there"),
    };
    IssueWording {
        message,
        help,
        fix: None,
        token: None,
        ..IssueWording::default()
    }
}

/// "unexpected end of input": a comment-only value, a `?` without its
/// `:` branch, or a missing operand, bracket, or quote.
fn end_of_input_wording(site: &Site<'_>) -> IssueWording {
    let trimmed = site.source.trim_start();
    if site.kind != ExpressionKind::Script
        && (trimmed.starts_with("//") || trimmed.starts_with("/*"))
    {
        let leading = site.source.len() - trimmed.len();
        return IssueWording {
            message: "the value is only a comment, not an expression".to_owned(),
            help: format!(
                "{} `{}` must produce a value: write the expression, or move the comment into a `<script>` or an XML comment `<!-- … -->`",
                article(&site.what),
                site.what
            ),
            token: Some((leading, leading + 2)),
            label: Some("a comment, not a value".to_owned()),
            ..IssueWording::default()
        };
    }
    let conditional = site.source.contains('?') && !site.source.contains(':');
    IssueWording {
        message: "the expression ends before it is complete".to_owned(),
        help: if conditional {
            "a `?` conditional needs both branches: `test ? whenTrue : whenFalse`".to_owned()
        } else {
            "add the missing operand, closing bracket, or closing quote at the end".to_owned()
        },
        fix: None,
        token: None,
        ..IssueWording::default()
    }
}

/// Every other checker message: the numeric-literal and the ECMAScript
/// checker's own messages keep their wording; anything else is never shown
/// verbatim.
fn other_message_wording(
    document: &Document,
    site: &Site<'_>,
    issue: &SyntaxIssue,
    raw: &str,
    generic: String,
) -> IssueWording {
    if raw.starts_with("a numeric literal must not be followed by") {
        return IssueWording {
            message: "a number is immediately followed by a letter, `$`, or `_`".to_owned(),
            help:
                "separate the number from the name that follows it, or start the name with a letter"
                    .to_owned(),
            fix: None,
            token: None,
            ..IssueWording::default()
        };
    }
    // The ECMAScript checker's own (not Boa's) messages are project wording.
    if matches!(
        raw,
        "expected a variable name or property access"
            | "expected a location, found nothing"
            | "expected an expression, found nothing"
    ) {
        return IssueWording {
            message: raw.to_owned(),
            help: generic,
            ..IssueWording::default()
        };
    }
    // Any other checker message stays internal: the finding echoes the
    // source text at the position in project wording.
    let token = source_word(&site.source, issue.start);
    let quote = quote_text_fix(document, site);
    IssueWording {
        message: if token.trim().is_empty() {
            "the expression cannot be parsed at this position".to_owned()
        } else {
            format!("`{token}` cannot be parsed here")
        },
        help: if quote.is_some() {
            format!(
                "{generic}; if the value is text, quote it as a string literal: write `{}`",
                quoted_text(&site.source).unwrap_or_default()
            )
        } else {
            format!("{generic}; write text as a quoted string literal and comments with `//`")
        },
        fix: quote,
        token: None,
        ..IssueWording::default()
    }
}

/// `site.source` as one quoted string literal (single quotes unless the
/// text contains one), or `None` when it cannot be quoted without escapes.
fn quoted_text(source: &str) -> Option<String> {
    let text = source.trim();
    if text.is_empty() || text.contains(['\\', '\n', '\r']) {
        return None;
    }
    if !text.contains('\'') {
        Some(format!("'{text}'"))
    } else if !text.contains('"') {
        Some(format!("\"{text}\""))
    } else {
        None
    }
}

/// Quoting a value expression as text (`Starting…` to `'Starting…'`): a
/// maybe-incorrect fix, offered only for a value expression whose text has
/// no quotes of its own and whose leading character cannot start an
/// expression (so it is prose, not a broken formula); the caller verifies
/// that the quoted fragment parses.
fn quote_text_fix(document: &Document, site: &Site<'_>) -> Option<Fix> {
    if site.kind != ExpressionKind::Value {
        return None;
    }
    let source = site.source.as_str();
    let trimmed = source.trim();
    // Operators and brackets make it a broken formula, not text.
    if trimmed.contains('`')
        || trimmed.contains([
            '=', '<', '>', '&', '|', '(', ')', '[', ']', '{', '}', '+', '*', '/',
        ])
    {
        return None;
    }
    let prose =
        trimmed.contains(char::is_whitespace) || !trimmed.is_ascii() || trimmed.starts_with('#');
    if !prose {
        return None;
    }
    let quoted = quoted_text(source)?;
    let leading = source.len() - source.trim_start().len();
    xpath::decoded_fix(
        document,
        site,
        leading,
        leading + trimmed.len(),
        "quote the text as a string literal".to_owned(),
        &quoted,
    )
}

/// `#`: Markdown headings and shell-style comments. ECMAScript has neither;
/// `#` only starts a private class member name (`#name`), which is what
/// the checker's "private identifier" messages report.
fn hash_wording(
    document: &Document,
    site: &Site<'_>,
    issue: &SyntaxIssue,
    raw: &str,
) -> Option<IssueWording> {
    let source = site.source.as_str();
    let at_hash = source
        .get(issue.start..)
        .is_some_and(|rest| rest.starts_with('#'));
    if !at_hash && !raw.contains("private identifier") {
        return None;
    }
    let hash = if at_hash {
        issue.start
    } else {
        first_outside_strings(source, '#')?
    };
    let line_start = source
        .get(..hash)?
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let starts_line = source.get(line_start..hash)?.trim().is_empty();
    let run = source
        .get(hash..)?
        .chars()
        .take_while(|character| *character == '#')
        .count();
    let after = source.get(hash + run..).unwrap_or_default();
    let name_follows = after
        .chars()
        .next()
        .is_some_and(|character| character.is_alphabetic() || matches!(character, '_' | '$'));
    let preceded_by_dot = source.get(..hash)?.trim_end().ends_with('.');
    if preceded_by_dot && name_follows && run == 1 {
        let name = source_word(after, 0);
        return Some(IssueWording {
            message: format!(
                "`#{name}` is a private class member name, which only code inside its class can use"
            ),
            help: format!("use a public property name: `.{name}`"),
            fix: exact_source_fix(
                site,
                hash,
                hash + 1,
                format!("use the public property `{name}`"),
                Applicability::MaybeIncorrect,
                "",
            ),
            token: Some((hash, hash + 1 + name.len())),
            label: Some("private name".to_owned()),
            ..IssueWording::default()
        });
    }
    let hashes = "#".repeat(run);
    if site.kind == ExpressionKind::Script && starts_line {
        return Some(IssueWording {
            message: format!("`{hashes}` does not start a comment in ECMAScript"),
            help: "write the comment with `//` (a Markdown heading does not belong in a script)"
                .to_owned(),
            fix: exact_source_fix(
                site,
                hash,
                hash + run,
                "start the comment with `//`",
                Applicability::MaybeIncorrect,
                "//",
            ),
            token: Some((hash, hash + run)),
            label: Some("not a comment".to_owned()),
            ..IssueWording::default()
        });
    }
    let quote = quote_text_fix(document, site);
    let text = quoted_text(source).unwrap_or_default();
    Some(IssueWording {
        message: if starts_line {
            format!("`{hashes}` does not start a comment or a heading in an ECMAScript expression")
        } else {
            format!("`{hashes}` is not an ECMAScript operator")
        },
        help: if quote.is_some() {
            format!("text in an expression must be a quoted string literal: write `{text}`")
        } else {
            "remove the `#` text: an expression cannot contain a comment or a Markdown heading, and `#` only starts a private class member name".to_owned()
        },
        fix: quote,
        token: Some((hash, hash + run)),
        label: Some("not valid ECMAScript here".to_owned()),
        ..IssueWording::default()
    })
}

/// Byte offset of the first `needle` outside string and template literals.
fn first_outside_strings(source: &str, needle: char) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (offset, character) in source.char_indices() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == open {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
        } else if character == needle {
            return Some(offset);
        }
    }
    None
}

/// The wording for an expression that is only `…`, the placeholder a
/// `has-placeholders` fix inserts, not yet replaced; shared by every data
/// model's syntax checker.
fn placeholder_wording(site: &Site<'_>) -> Option<IssueWording> {
    if site.source.trim() != "…" {
        return None;
    }
    let leading = site.source.len() - site.source.trim_start().len();
    Some(IssueWording {
        message: "`…` is a placeholder, not an expression".to_owned(),
        help: format!(
            "replace `…` with the {} this `{}` needs",
            match site.kind {
                ExpressionKind::Condition => "condition",
                ExpressionKind::Location => "location",
                ExpressionKind::Value | ExpressionKind::Script => "expression",
            },
            site.what
        ),
        token: Some((leading, leading + '…'.len_utf8())),
        label: Some("placeholder".to_owned()),
        ..IssueWording::default()
    })
}

/// A character the ECMAScript lexer rejects (`@`, `…`, an emoji, `\`).
fn unexpected_character_wording(
    document: &Document,
    site: &Site<'_>,
    issue: &SyntaxIssue,
    character: char,
) -> IssueWording {
    if let Some(wording) = placeholder_wording(site) {
        return wording;
    }
    let quote = quote_text_fix(document, site);
    let text = quoted_text(&site.source).unwrap_or_default();
    let at = site
        .source
        .get(issue.start..)
        .filter(|rest| rest.starts_with(character))
        .map(|_| issue.start)
        .or_else(|| first_outside_strings(&site.source, character));
    IssueWording {
        message: if character.is_ascii() {
            format!("`{character}` is not an ECMAScript operator or name character")
        } else {
            format!("`{character}` can appear only inside a string literal in ECMAScript")
        },
        help: if quote.is_some() {
            format!("text in an expression must be a quoted string literal: write `{text}`")
        } else if character == '\\' {
            "a backslash escapes a character only inside a string literal; remove it, or move the text into a quoted string".to_owned()
        } else {
            format!("remove `{character}`, or quote the text around it as a string literal")
        },
        fix: quote,
        token: at.map(|at| (at, at + character.len_utf8())),
        label: Some("unexpected character".to_owned()),
        ..IssueWording::default()
    }
}

/// A repair of `start..end` in the decoded source, offered only when those
/// bytes map one-to-one onto the document (no entity or CDATA in between).
fn exact_source_fix(
    site: &Site<'_>,
    start: usize,
    end: usize,
    message: impl Into<String>,
    applicability: Applicability,
    replacement: impl Into<String>,
) -> Option<Fix> {
    site.offsets.as_ref()?;
    let span = site.issue_span(&SyntaxIssue {
        start,
        end,
        message: String::new(),
    });
    (span.len() == end.checked_sub(start)? && end > start)
        .then(|| Fix::replace(message, applicability, span, replacement))
}

/// "expected X, found TOKEN": drops the checker's synthetic closing `)` (it
/// parses expressions wrapped in parentheses) and recognizes the mistakes
/// that produce "a complete expression followed by more text": word
/// operators, prose without quotes, and an apostrophe that ends a string.
/// "expected X, found TOKEN" where X is not only the synthetic closing
/// punctuation: names what was expected.
fn expected_tokens_wording(
    data_model: &str,
    source: &str,
    tokens: &[&str],
    found: &str,
) -> IssueWording {
    let balanced = source.matches('(').count() == source.matches(')').count();
    // Grammar names (`expression`, `identifier`) read as prose; tokens stay
    // in backticks.
    let kept: Vec<String> = tokens
        .iter()
        .copied()
        .filter(|token| !(balanced && *token == "`)`"))
        .map(|token| {
            let name = token.trim_matches('`');
            if name.len() > 1
                && name
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character == ' ')
            {
                format!("{} {name}", article(name))
            } else {
                token.to_owned()
            }
        })
        .collect();
    let expected = kept.join(" or ");
    IssueWording {
        message: format!("expected {expected}, found `{found}`"),
        help: if found == "," {
            "remove the extra `,`, or write the missing value before it".to_owned()
        } else {
            format!(
                "write {expected} where `{found}` is (fix the {data_model} syntax at the marked position)"
            )
        },
        fix: None,
        token: None,
        ..IssueWording::default()
    }
}

fn expected_token_wording(
    document: &Document,
    data_model: &str,
    site: &Site<'_>,
    issue: &SyntaxIssue,
    expected: &str,
    found: &str,
) -> IssueWording {
    let source = site.source.as_str();
    let tokens: Vec<&str> = expected.split(", ").collect();
    let after_complete = tokens
        .iter()
        .all(|token| matches!(*token, "`)`" | "`,`" | "`;`"));
    if !after_complete {
        return expected_tokens_wording(data_model, source, &tokens, found);
    }
    // The checker names tokens in normalized form (`x.1` reports `0.1`), so
    // the message echoes the source text at the position instead.
    // A `.` followed by digits lexes as one number (`list.0` reports `0`),
    // so the source text `.0` is echoed rather than a later `0`.
    let dotted_number = source.get(issue.start..).and_then(|rest| {
        let digits = rest
            .strip_prefix('.')?
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        (digits > 0).then(|| rest.get(..=digits).map(str::to_owned))?
    });
    let (found, token_start) = if let Some(dotted) = dotted_number {
        (dotted, issue.start)
    } else if source
        .get(issue.start..)
        .is_some_and(|rest| rest.starts_with(found))
    {
        (found.to_owned(), issue.start)
    } else if let Some(offset) = source.find(found) {
        (found.to_owned(), offset)
    } else {
        (source_word(source, issue.start).to_owned(), issue.start)
    };
    let found = found.as_str();
    let token_end = token_start.saturating_add(found.len());
    if let Some(wording) = word_operator_wording(site, found, token_start, token_end)
        .or_else(|| declaration_wording(site))
        .or_else(|| digit_property_wording(site, found, token_start, token_end))
    {
        return wording;
    }
    let before = source.get(..token_start).unwrap_or_default();
    let trimmed = source.trim_start();
    if let Some(quote) = before
        .trim_end()
        .chars()
        .next_back()
        .filter(|quote| matches!(quote, '\'' | '"'))
        && trimmed.starts_with(quote)
        && before.trim_end().len() > source.len() - trimmed.len() + 1
    {
        let quote_at = before.trim_end().len() - 1;
        return IssueWording {
            message: format!("the string literal ends early at this `{quote}`"),
            help: format!(
                "escape the `{quote}` inside the string as `\\{quote}`, or delimit the string with the other quote character"
            ),
            fix: exact_source_fix(
                site,
                quote_at,
                quote_at + 1,
                format!("escape the inner `{quote}`"),
                Applicability::MaybeIncorrect,
                format!("\\{quote}"),
            ),
            token: Some((quote_at, quote_at + 1)),
            ..IssueWording::default()
        };
    }
    let prose = site.kind == ExpressionKind::Value
        && trimmed.contains(' ')
        && trimmed.chars().all(|character| {
            character.is_alphanumeric()
                || !character.is_ascii()
                || matches!(character, ' ' | '.' | ',' | '!' | '?' | '-' | ':' | ';')
        });
    if prose {
        let quoted = quoted_text(source).unwrap_or_default();
        return IssueWording {
            message: format!("unexpected `{found}` after a complete expression"),
            help: format!(
                "text in an expression must be a quoted string literal: write `{quoted}`"
            ),
            fix: quote_text_fix(document, site),
            token: Some((token_start, token_end)),
            ..IssueWording::default()
        };
    }
    IssueWording {
        message: format!("unexpected `{found}` after a complete expression"),
        help: "two expressions follow each other with no operator between them: add the missing operator (such as `&&`, `+`, or `,`), or quote text as a string".to_owned(),
        fix: None,
        token: Some((token_start, token_end)),
        ..IssueWording::default()
    }
}

/// The identifier, number, or property-access text at `start` (at least one
/// character).
fn source_word(source: &str, start: usize) -> &str {
    let rest = source.get(start..).unwrap_or_default();
    let length = rest
        .char_indices()
        .find(|(_, character)| {
            !(character.is_alphanumeric() || matches!(character, '.' | '_' | '$'))
        })
        .map_or(rest.len(), |(index, _)| index);
    let length = if length == 0 {
        rest.chars().next().map_or(0, char::len_utf8)
    } else {
        length
    };
    rest.get(..length).unwrap_or_default()
}

/// `x.1`: a property name cannot start with a digit, so an index was meant.
fn digit_property_wording(
    site: &Site<'_>,
    found: &str,
    token_start: usize,
    token_end: usize,
) -> Option<IssueWording> {
    let digits = found.strip_prefix('.')?;
    let follows_operand = site
        .source
        .get(..token_start)
        .and_then(|head| head.chars().next_back())
        .is_some_and(|character| {
            character.is_alphanumeric() || matches!(character, '_' | '$' | ')' | ']')
        });
    (!digits.is_empty()
        && digits.chars().all(|character| character.is_ascii_digit())
        && follows_operand)
        .then(|| IssueWording {
            message: format!("`{found}` is not a property access"),
            help: format!(
                "a property name cannot start with a digit; index with brackets: `[{digits}]`"
            ),
            fix: exact_source_fix(
                site,
                token_start,
                token_end,
                format!("index with `[{digits}]`"),
                Applicability::MaybeIncorrect,
                format!("[{digits}]"),
            ),
            token: Some((token_start, token_end)),
            ..IssueWording::default()
        })
}

/// The indefinite article for an attribute or element name as it is
/// rendered: "an" before `expr`, "a" before `cond` and `<script>`.
fn article(word: &str) -> &'static str {
    let first = word
        .chars()
        .find(char::is_ascii_alphabetic)
        .map(|first| first.to_ascii_lowercase());
    if matches!(first, Some('a' | 'e' | 'i' | 'o' | 'u')) && !word.starts_with('<') {
        "an"
    } else {
        "a"
    }
}

/// `let y = 1` (or `const`/`var`) in a condition or value: a declaration is
/// a statement, not an expression.
fn declaration_wording(site: &Site<'_>) -> Option<IssueWording> {
    if site.kind == ExpressionKind::Script {
        return None;
    }
    let source = site.source.as_str();
    let leading = source.len() - source.trim_start().len();
    let keyword = ["let", "const", "var"].into_iter().find(|keyword| {
        source
            .trim_start()
            .strip_prefix(keyword)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    })?;
    let declared = source_word(source.trim_start().get(keyword.len()..)?.trim_start(), 0);
    let name = if declared.is_empty() {
        "name"
    } else {
        declared
    };
    let article = article(&site.what);
    Some(IssueWording {
        message: format!(
            "`{keyword}` declares a variable, which {article} `{}` cannot do",
            site.what
        ),
        help: format!(
            "{article} `{}` must be an expression: declare the variable with `<data id=\"{name}\" expr=\"…\"/>` in the `<datamodel>`, or set it with `<assign location=\"{name}\" expr=\"…\"/>`",
            site.what
        ),
        token: Some((leading, leading + keyword.len())),
        label: Some("a declaration, not an expression".to_owned()),
        ..IssueWording::default()
    })
}

/// `typeof x = 'object'` or `x++ = 2`: Boa rejects the assignment target.
/// In a condition or value the `=` was almost always meant to compare.
fn invalid_assignment_wording(site: &Site<'_>, issue: &SyntaxIssue) -> IssueWording {
    let at = issue.start;
    let single_equals = site
        .source
        .get(at..)
        .is_some_and(|rest| rest.starts_with('=') && !rest.starts_with("=="));
    let comparing =
        single_equals && matches!(site.kind, ExpressionKind::Condition | ExpressionKind::Value);
    IssueWording {
        message: "the left side of `=` cannot be assigned to".to_owned(),
        help: if comparing {
            "`=` assigns, and only a variable or a property such as `a.b` or `a[0]` can be assigned; to compare, write `===` (or `==`)".to_owned()
        } else {
            "assign only to a variable or a property such as `a.b` or `a[0]`".to_owned()
        },
        fix: comparing
            .then(|| {
                exact_source_fix(
                    site,
                    at,
                    at + 1,
                    "compare with `===`",
                    Applicability::MaybeIncorrect,
                    "===",
                )
            })
            .flatten(),
        label: Some("not assignable".to_owned()),
        ..IssueWording::default()
    }
}

/// An unterminated string: label the opening quote and name the delimiter.
fn unterminated_string_wording(
    document: &Document,
    site: &Site<'_>,
    issue: &SyntaxIssue,
) -> IssueWording {
    // The checker may place the issue at the start of the fragment; the
    // opening quote is the one still open at the end.
    let at_issue = site
        .source
        .get(issue.start..)
        .and_then(|rest| rest.chars().next())
        .filter(|quote| matches!(quote, '\'' | '"' | '`'))
        .map(|quote| (issue.start, quote));
    let (at, quote) = at_issue
        .or_else(|| unclosed_quote(&site.source))
        .unwrap_or((issue.start, '\''));
    let text_fix = quote_text_fix(document, site);
    let text = quoted_text(&site.source).unwrap_or_default();
    let (message, close) = if quote == '`' {
        (
            "unterminated template literal",
            "close the template literal with a matching backtick; to include a backtick inside it, escape it with `\\`".to_owned(),
        )
    } else {
        (
            "unterminated string literal",
            format!(
                "close the string with a matching `{quote}`; to include a `{quote}` inside it, escape it as `\\{quote}`"
            ),
        )
    };
    IssueWording {
        message: message.to_owned(),
        help: if text_fix.is_some() {
            format!("{close}; if the whole value is text, quote all of it: write `{text}`")
        } else {
            close
        },
        fix: text_fix,
        token: Some((at, at + quote.len_utf8())),
        label: Some("string starts here".to_owned()),
        ..IssueWording::default()
    }
}

/// The offset and character of the quote still open at the end of
/// `source`, if any.
fn unclosed_quote(source: &str) -> Option<(usize, char)> {
    let mut open: Option<(usize, char)> = None;
    let mut escaped = false;
    for (offset, character) in source.char_indices() {
        match open {
            Some((_, quote)) => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == quote {
                    open = None;
                }
            }
            None if matches!(character, '\'' | '"' | '`') => open = Some((offset, character)),
            None => {}
        }
    }
    open
}

/// `and`/`or`/`not` written as words: ECMAScript spells them `&&`/`||`/`!`.
fn word_operator_wording(
    site: &Site<'_>,
    found: &str,
    token_start: usize,
    token_end: usize,
) -> Option<IssueWording> {
    let in_attribute = site.attribute.is_some();
    let word_operator = match found {
        "and" => Some((
            "&&",
            if in_attribute { "&amp;&amp;" } else { "&&" },
            "write `&&` (escaped as `&amp;&amp;` inside an XML attribute)",
        )),
        "or" => Some(("||", "||", "write `||`")),
        "not" => Some(("!", "!", "write `!` before the operand")),
        _ => None,
    };
    word_operator.map(|(operator, replacement, help)| IssueWording {
        message: format!("`{found}` is not an ECMAScript operator"),
        help: help.to_owned(),
        fix: (in_attribute || operator == replacement)
            .then(|| {
                exact_source_fix(
                    site,
                    token_start,
                    token_end,
                    format!("use `{operator}`"),
                    Applicability::MaybeIncorrect,
                    replacement,
                )
            })
            .flatten(),
        token: Some((token_start, token_end)),
        ..IssueWording::default()
    })
}

/// Help and repair for a stray `=` that follows a comparison operator.
struct StrayEquals {
    help: String,
    fix: Option<Fix>,
}

/// ECMAScript comparison operators a stray `=` can complete.
const ECMASCRIPT_COMPARISONS: [&str; 6] = ["==", "===", "!=", "!==", "<=", ">="];

/// `x = = 3`, `x == = 3`, `x === = 3`, `x !== = 3`, `x < = 3`: the `=` at
/// `at` follows an operator made of `=`, `!`, `<`, and `>`, optionally
/// separated by whitespace. When the operator and the `=` together spell a
/// comparison, the repair joins them (removes the whitespace); otherwise the
/// operator is already complete and the repair removes the stray `=` with
/// the whitespace before it. Either way exactly one comparison operator
/// remains, so the repair never yields `====`.
fn stray_equals_repair(site: &Site<'_>, at: usize) -> Option<StrayEquals> {
    let source = site.source.as_str();
    if source.get(at..at.checked_add(1)?)? != "=" {
        return None;
    }
    let before = source.get(..at)?;
    let operator_end = before.trim_end().len();
    let head = before.get(..operator_end)?;
    let operator_start = head
        .char_indices()
        .rev()
        .take_while(|(_, character)| matches!(character, '=' | '!' | '<' | '>'))
        .last()
        .map(|(index, _)| index)?;
    let operator = head.get(operator_start..)?;
    let spaced = source.get(operator_start..=at)?;
    let joined = format!("{operator}=");
    if ECMASCRIPT_COMPARISONS.contains(&joined.as_str()) {
        return Some(StrayEquals {
            help: format!(
                "`{spaced}` is not an operator; write `{joined}` without the space to compare"
            ),
            fix: exact_source_fix(
                site,
                operator_end,
                at,
                format!("compare with `{joined}`"),
                Applicability::MaybeIncorrect,
                "",
            ),
        });
    }
    ECMASCRIPT_COMPARISONS
        .contains(&operator)
        .then(|| StrayEquals {
            help: format!(
                "`{operator}` is already a complete operator; remove the stray `=` after it"
            ),
            fix: exact_source_fix(
                site,
                operator_end,
                at + 1,
                format!("remove the stray `=` and compare with `{operator}`"),
                Applicability::MaybeIncorrect,
                "",
            ),
        })
}

/// What a fix does to the fragment it repairs.
enum Repaired {
    /// The fix edits outside the fragment (for example it removes the
    /// element), so it is not a syntax repair.
    Outside,
    /// The decoded fragment the fix leaves behind.
    Fragment(String),
    /// The fix leaves text that does not decode (it cannot be verified).
    Unverifiable,
}

/// The decoded fragment `fix` would leave in `site`.
fn repaired_fragment(document: &Document, site: &Site<'_>, fix: &Fix) -> Repaired {
    let span = site.value_span;
    if fix
        .edits
        .iter()
        .any(|edit| edit.span.start < span.start || edit.span.end > span.end)
    {
        return Repaired::Outside;
    }
    let Some(mut raw) = document.source.slice(span).map(str::to_owned) else {
        return Repaired::Unverifiable;
    };
    let mut edits: Vec<&FixEdit> = fix.edits.iter().collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
    for edit in edits {
        let start = edit.span.start - span.start;
        let end = edit.span.end - span.start;
        if !raw.is_char_boundary(start) || !raw.is_char_boundary(end) {
            return Repaired::Unverifiable;
        }
        raw.replace_range(start..end, &edit.replacement);
    }
    let decoded = match raw
        .strip_prefix("<![CDATA[")
        .and_then(|inner| inner.strip_suffix("]]>"))
    {
        Some(inner) if site.attribute.is_none() => Some(inner.to_owned()),
        _ => decode_offsets(&raw, 0).map(|(decoded, _)| decoded),
    };
    decoded.map_or(Repaired::Unverifiable, Repaired::Fragment)
}

/// Whether `fix` leaves `site` parsing for `checker` (always true for a fix
/// that edits outside the fragment, such as removing its element).
fn repair_parses(
    document: &Document,
    checker: &dyn ExpressionSyntaxChecker,
    site: &Site<'_>,
    fix: &Fix,
) -> bool {
    match repaired_fragment(document, site, fix) {
        Repaired::Outside => true,
        Repaired::Fragment(repaired) => {
            checker.check(site.kind, &repaired) == ExpressionCheck::Valid
        }
        Repaired::Unverifiable => false,
    }
}

fn issue_diagnostic(
    document: &Document,
    checker: &dyn ExpressionSyntaxChecker,
    site: &Site<'_>,
    issue: &SyntaxIssue,
) -> Diagnostic {
    let wording = word_issue(document, checker.data_model(), site, issue);
    worded_diagnostic(
        document,
        checker,
        site,
        site.issue_span(issue),
        wording,
        true,
    )
}

/// Every habit of `site` as its own finding, when the fragment with all of
/// their repairs applied parses for `checker`; `None` otherwise (the
/// checker's own issue is then worded instead). Each repair is verified by
/// the composed re-check, so a single repair may leave the other reported
/// habits standing, but never an unreported error. The lints of a fragment
/// that parses (with `scope`, the `XPath` reference lints) run on the
/// repaired fragment too, mapped back onto the source, and the repaired
/// fragment is returned for the machine's side-effect analysis, so applying
/// every repair reveals no finding that was not already reported.
fn habit_diagnostics<'a>(
    document: &Document,
    checker: &dyn ExpressionSyntaxChecker,
    site: &Site<'a>,
    scope: Option<&xpath::references::Scope>,
) -> Option<(Vec<Diagnostic>, Site<'a>)> {
    let loop_variable =
        site.element.name.local_name == "foreach" && matches!(site.what.as_str(), "item" | "index");
    if loop_variable || site.what == "namelist" {
        return None;
    }
    let found = habits::scan(checker.data_model(), &site.source);
    if found.is_empty() {
        return None;
    }
    let (repaired, map) = habits::apply_mapped(&site.source, &found)?;
    if checker.check(site.kind, &repaired) != ExpressionCheck::Valid {
        return None;
    }
    let mut findings: Vec<Diagnostic> = found
        .iter()
        .take(MAX_ISSUES_PER_EXPRESSION)
        .map(|habit| {
            let edits: Option<Vec<FixEdit>> = habit
                .edits
                .iter()
                .map(|edit| {
                    xpath::decoded_fix(
                        document,
                        site,
                        edit.start,
                        edit.end,
                        String::new(),
                        &edit.replacement,
                    )
                    .and_then(|fix| fix.edits.into_iter().next())
                })
                .collect();
            let fix = edits.map(|edits| Fix {
                message: habit.fix_message.clone(),
                applicability: habit.applicability,
                edits,
            });
            let wording = IssueWording {
                message: habit.message.clone(),
                help: habit.help.clone(),
                fix,
                token: Some((habit.start, habit.end)),
                ..IssueWording::default()
            };
            worded_diagnostic(
                document,
                checker,
                site,
                site.span_of(habit.start, habit.end),
                wording,
                false,
            )
        })
        .collect();
    let composed = Site {
        kind: site.kind,
        what: site.what.clone(),
        offsets: site.offsets.as_ref().and_then(|offsets| {
            map.iter()
                .map(|decoded| offsets.get(*decoded).copied())
                .collect()
        }),
        source: repaired,
        value_span: site.value_span,
        attribute: site.attribute,
        element: site.element,
    };
    if let Some(scope) = scope {
        findings.extend(xpath::references::lint(document, &composed, scope));
    }
    Some((findings, composed))
}

/// `SCXML039` for a location that is not assignable. Under ECMAScript, a
/// name declared by a `<data id>` the data model cannot declare
/// (`location="retry-count"`) names that declaration as the root cause.
fn location_diagnostic(
    document: &Document,
    data_model: &str,
    site: &Site<'_>,
    wording: &IssueWording,
    primary: Span,
) -> Diagnostic {
    let xpath = data_model == xpath::DATA_MODEL;
    let (note, help) = if xpath {
        (
            "an XPath location must select existing nodes: a declared variable `$name` or a path below it such as `$name/child` or `$name/@attribute`; `a.b` is one variable name that contains a dot, and `a[0]` is a predicate, not an index".to_owned(),
            wording.help.clone(),
        )
    } else {
        (
            "a location expression must name a place in the data model that can be assigned, such as a variable name or a property access like `a.b` or `a[0]`".to_owned(),
            // A targeted repair (`x.0` to `x[0]`) keeps its own help.
            if wording.fix.is_some() {
                wording.help.clone()
            } else {
                "use a declared variable name or a member expression".to_owned()
            },
        )
    };
    let name = site.source.trim();
    let declaration = (!xpath)
        .then(|| data_ids::undeclarable_data(document, site.element, name))
        .flatten();
    let (reason, primary) = if !xpath && ECMASCRIPT_RESERVED_WORDS.contains(&name) {
        (
            format!("`{name}` is a reserved word, not a variable"),
            site.value_span,
        )
    } else if declaration.is_some() {
        (
            format!("`{name}` is not an ECMAScript variable name"),
            site.value_span,
        )
    } else {
        (wording.message.clone(), primary)
    };
    let help = match &declaration {
        Some((_, Some(renamed))) => format!(
            "`<data id=\"{name}\">` declares a variable no ECMAScript expression can name (`LINT015`): rename it, for example to `{renamed}`, in its declaration and every reference"
        ),
        Some((_, None)) => format!(
            "`<data id=\"{name}\">` declares a variable no ECMAScript expression can name (`LINT015`): rename it in its declaration and every reference"
        ),
        None => help,
    };
    let mut diagnostic = Diagnostic::warning(
        "SCXML039",
        format!(
            "`{}` is not an assignable {data_model} location: {reason}",
            site.what
        ),
        primary,
    )
    .with_primary_message("not a location")
    .with_note(note)
    .with_help(help)
    .with_spec_reference("SCXML 1.0 §5.9.2 Location Expressions");
    if let Some((id, _)) = declaration {
        diagnostic = diagnostic.with_secondary(
            Label::new(id.value_span)
                .with_message("declared here with a name that is not a variable name"),
        );
    }
    diagnostic
}

/// Builds the `SCXML038`/`SCXML039` finding for one worded issue. With
/// `verify_fixes`, a fragment repair is offered only when the checker
/// accepts the repaired fragment.
fn worded_diagnostic(
    document: &Document,
    checker: &dyn ExpressionSyntaxChecker,
    site: &Site<'_>,
    fallback: Span,
    wording: IssueWording,
    verify_fixes: bool,
) -> Diagnostic {
    let data_model = checker.data_model();
    let decoded_span = |start: usize, end: usize| {
        site.issue_span(&SyntaxIssue {
            start,
            end,
            message: String::new(),
        })
    };
    // Widen a one-character checker position to the whole responsible token.
    let primary = match wording.token {
        Some((start, end)) if end > start && end <= site.source.len() => decoded_span(start, end),
        _ => fallback,
    };
    let what = site.what.as_str();
    let spec = match data_model {
        "ecmascript" => SPEC_ECMASCRIPT,
        xpath::DATA_MODEL => xpath::SPEC_XPATH,
        _ => "SCXML 1.0 §5.9 Expressions",
    };
    let mut diagnostic = if site.kind == ExpressionKind::Location
        && site.element.name.local_name == "foreach"
    {
        foreach_variable_diagnostic(data_model, site, &wording.message, primary)
    } else if site.kind == ExpressionKind::Location
        && site.what == "namelist"
        && site.source.contains(',')
    {
        namelist_comma_diagnostic(site, primary)
    } else if site.kind == ExpressionKind::Location {
        location_diagnostic(document, data_model, site, &wording, primary)
    } else {
        Diagnostic::warning(
            "SCXML038",
            format!("{data_model} syntax error in `{what}`: {}", wording.message),
            primary,
        )
        .with_primary_message(wording.label.as_deref().unwrap_or("syntax error"))
        .with_note(wording.note.clone().unwrap_or_else(|| match site.kind {
            ExpressionKind::Condition => "a condition that cannot be parsed raises `error.execution` and evaluates as false, so this condition can never hold",
            ExpressionKind::Script => "a script that cannot be parsed raises `error.execution` and aborts the rest of its executable-content block",
            ExpressionKind::Value | ExpressionKind::Location => "an expression that cannot be parsed raises `error.execution` whenever it is evaluated",
        }.to_owned()))
        .with_help(wording.help.clone())
        .with_spec_reference("SCXML 1.0 §5.9 Expressions")
    }
    .with_spec_reference(spec);
    // A repair of the fragment is offered only when the same checker accepts
    // the repaired fragment, so no suggestion can leave (or move) a syntax
    // error; a fix that edits outside the fragment (removing its element)
    // is not a syntax repair and is kept.
    for fix in wording.fix.into_iter().chain(wording.alternatives) {
        if !verify_fixes || repair_parses(document, checker, site, &fix) {
            diagnostic = diagnostic.with_fix(fix);
        }
    }
    for (start, end, message) in wording.related {
        let span = decoded_span(start, end);
        if span != site.value_span && span != diagnostic.primary.span {
            diagnostic = diagnostic.with_secondary(Label::new(span).with_message(message));
        }
    }
    if diagnostic.primary.span != site.value_span {
        diagnostic = diagnostic.with_secondary(
            Label::new(site.value_span).with_message(format!("while parsing this `{what}`")),
        );
    }
    diagnostic
}

/// `namelist="a,b"`: names are separated by whitespace, so the comma makes
/// one invalid location out of two valid ones.
fn namelist_comma_diagnostic(site: &Site<'_>, primary: Span) -> Diagnostic {
    let names: Vec<&str> = site
        .source
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    let diagnostic = Diagnostic::warning(
        "SCXML039",
        format!(
            "`namelist` separates names with spaces, not commas: `{}` is not one location",
            site.source
        ),
        primary,
    )
    .with_primary_message("comma-separated names")
    .with_note("`namelist` is a space-separated list of data-model locations; a comma makes the list one name that is not a valid location")
    .with_help(format!("separate the names with spaces: `{}`", names.join(" ")))
    .with_spec_reference("SCXML 1.0 §6.2 <send>");
    match exact_source_fix(
        site,
        0,
        site.source.len(),
        "separate the names with spaces",
        Applicability::MachineApplicable,
        names.join(" "),
    ) {
        Some(fix) if !names.is_empty() => diagnostic.with_fix(fix),
        _ => diagnostic,
    }
}

/// Offset of a top-level `=>` in an ECMAScript condition: outside string,
/// template, and regular-expression literals and outside every `( … )`,
/// `[ … ]`, and `{ … }`, so the condition itself is an arrow function
/// (`x => 1`, `(x) => 1`), not a call that takes one (`list.some(x => x)`).
/// One bounded pass.
fn arrow_in_condition(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut quote: Option<u8> = None;
    let mut depth = 0_usize;
    let mut previous: Option<u8> = None;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if let Some(open) = quote {
            if byte == b'\\' {
                index = index.saturating_add(2);
                continue;
            }
            if byte == open {
                quote = None;
                previous = Some(byte);
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'/' if previous.is_none_or(|previous| b"(,=:[!&|?{};+-*%<>~^".contains(&previous)) => {
                let mut class = false;
                index += 1;
                while let Some(&inner) = bytes.get(index) {
                    match inner {
                        b'\\' => index += 1,
                        b'[' => class = true,
                        b']' => class = false,
                        b'/' if !class => break,
                        _ => {}
                    }
                    index += 1;
                }
                previous = Some(b'/');
                index += 1;
                continue;
            }
            b'(' | b'[' | b'{' => depth = depth.saturating_add(1),
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'=' if depth == 0 && bytes.get(index + 1) == Some(&b'>') => return Some(index),
            _ => {}
        }
        if !byte.is_ascii_whitespace() {
            previous = Some(byte);
        }
        index += 1;
    }
    None
}

/// LINT017: `cond="count => 3"` parses as an arrow function, which is
/// always truthy, so the condition always holds; `>=` was meant.
fn arrow_in_condition_diagnostic(
    document: &Document,
    site: &Site<'_>,
    offset: usize,
) -> Diagnostic {
    let primary = site.issue_span(&SyntaxIssue {
        start: offset,
        end: offset + 2,
        message: String::new(),
    });
    let parameter = site.source.get(..offset).unwrap_or_default().trim();
    let parameter = parameter
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .map_or(parameter, str::trim);
    let single_identifier = !parameter.is_empty()
        && !parameter.starts_with(|first: char| first.is_ascii_digit())
        && parameter
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'));
    let mut diagnostic = Diagnostic::warning(
        "LINT017",
        format!(
            "`cond` on `<{}>` is an arrow function, so it is always true",
            site.element.name.local_name
        ),
        primary,
    )
    .with_category(crate::DiagnosticCategory::Lint)
    .with_primary_message("`=>` makes an arrow function")
    .with_note("in ECMAScript `x => 1` is a function whose parameter is `x`; a condition is converted to a boolean without calling it, and every function is truthy, so the condition always holds")
    .with_help(if single_identifier {
        format!("to test whether `{parameter}` is at least the value, compare with `>=`")
    } else {
        "a condition must compute a boolean: compare with `>=`, or call the function".to_owned()
    })
    .with_spec_reference("SCXML 1.0 §5.9.1 Conditional Expressions")
    .with_spec_reference(SPEC_ECMASCRIPT);
    if single_identifier
        && let Some(fix) = xpath::decoded_fix(
            document,
            site,
            offset,
            offset + 2,
            "compare with `>=`".to_owned(),
            ">=",
        )
    {
        diagnostic = diagnostic.with_fix(fix);
    }
    diagnostic
}

/// ECMA-262 `ReservedWord` outside strict mode code (keywords, future
/// reserved words, and the `null`/`true`/`false` literals): the names that
/// can never name a variable in a script, exactly the ECMAScript data
/// model's list (`components/scxml-data-model-ecmascript/src/syntax.rs`).
/// `let`, `yield`, and `await` are identifiers in a non-strict script.
const ECMASCRIPT_RESERVED_WORDS: [&str; 36] = [
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
];

/// SCXML039 for a `<foreach>` `item` or `index`: these name loop variables
/// (SCXML 1.0 §4.6), so the wording and repair are about variable names,
/// not assignable locations in general.
fn foreach_variable_diagnostic(
    data_model: &str,
    site: &Site<'_>,
    issue_message: &str,
    primary: Span,
) -> Diagnostic {
    let what = site.what.as_str();
    let name = site.source.trim();
    let starts_with_digit = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit());
    let identifier_tail = name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'));
    let reserved = data_model == "ecmascript" && ECMASCRIPT_RESERVED_WORDS.contains(&name);
    let message = if starts_with_digit {
        format!("`<foreach {what}>` must be a variable name, but `{name}` starts with a digit")
    } else if reserved {
        format!(
            "`<foreach {what}>` must be a variable name, but `{name}` is a reserved word in ECMAScript"
        )
    } else {
        format!("`<foreach {what}>` must be a variable name: {issue_message}")
    };
    // A leading digit or a reserved word disqualifies the whole name, so
    // label all of it.
    let primary = if starts_with_digit || reserved {
        site.value_span
    } else {
        primary
    };
    if reserved && !name.is_empty() {
        return Diagnostic::warning("SCXML039", message, primary)
            .with_primary_message("reserved word")
            .with_note(format!("`<foreach>` assigns each array element (`item`) and its position (`index`) to a variable with this name; an ECMAScript reserved word such as `{name}` can never name a variable"))
            .with_help(format!("rename the loop variable, for example `{name}Item`, and use that name inside the loop"))
            .with_spec_reference("SCXML 1.0 §4.6 <foreach>")
            .with_fix(Fix::replace(
                format!("rename the loop variable to `{name}Item`"),
                Applicability::MaybeIncorrect,
                site.value_span,
                format!("{name}Item"),
            ));
    }
    let diagnostic = Diagnostic::warning("SCXML039", message, primary)
        .with_primary_message("not a variable name")
        .with_note(format!("`<foreach>` assigns each array element (`item`) and its position (`index`) to a variable with this name, creating it when it is not declared; the name must be a legal identifier of the `{data_model}` data model"))
        .with_help(if data_model == xpath::DATA_MODEL {
            "use an XPath variable name: an NCName that starts with a letter or `_`, without `$` (`<foreach>` binds the variable, and expressions then refer to it as `$name`)"
        } else {
            "use a name that starts with a letter or `$` and contains only ASCII letters, digits, `_`, and `$` (the ECMAScript data model reserves names that start with `_` for system variables)"
        })
        .with_spec_reference("SCXML 1.0 §4.6 <foreach>");
    // The ECMAScript data model reserves a leading `_`, so its repair
    // prefixes the attribute name instead (`1x` to `item1x`).
    let (prefix, tail_ok) = if data_model == xpath::DATA_MODEL {
        (
            "_",
            name.chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_'),
        )
    } else {
        (what, identifier_tail)
    };
    if starts_with_digit && tail_ok && !name.is_empty() {
        diagnostic.with_fix(Fix::replace(
            format!("prefix the name with `{prefix}`: `{prefix}{name}`"),
            Applicability::MaybeIncorrect,
            site.value_span,
            format!("{prefix}{name}"),
        ))
    } else {
        diagnostic
    }
}

/// Whether `site` is an empty (not merely blank) required attribute that
/// lowering rejects, which `SCXML066` reports.
fn lowering_rejects_empty(site: &Site<'_>) -> bool {
    site.attribute.is_some_and(|attribute| {
        attribute.value.is_empty()
            && matches!(
                (
                    site.element.name.local_name.as_str(),
                    attribute.name.local_name.as_str()
                ),
                ("if" | "elseif", "cond") | ("foreach", "array" | "item") | ("assign", "location")
            )
    })
}

fn empty_expression_diagnostic(document: &Document, site: &Site<'_>) -> Diagnostic {
    let what = site.what.as_str();
    let element = site.element.name.local_name.as_str();
    let primary = site
        .attribute
        .map_or(site.value_span, |attribute| attribute.span);
    // Lowering maps a blank `<log expr>` to the absent attribute (W3C IRP
    // 307 authors it), so removing it is exact. Every other blank
    // expression, including `<transition cond>`, is evaluated and raises
    // `error.execution` (SCXML 5.9), so removing it changes behavior.
    let absent_meaning =
        site.attribute.and_then(
            |attribute| match (element, attribute.name.local_name.as_str()) {
                ("log", "expr") => {
                    Some("a blank `expr` logs no value, exactly as if `expr` were absent")
                }
                _ => None,
            },
        );
    let (message, note, applicability) = match absent_meaning {
        Some(note) => (
            format!("empty `{what}` on `<{element}>` is treated as absent"),
            note,
            Applicability::MachineApplicable,
        ),
        None => (
            format!(
                "empty `{what}` on `<{element}>` is still evaluated and cannot produce a value"
            ),
            "an attribute that is present is evaluated even when empty; an empty expression raises `error.execution` instead of behaving as if the attribute were absent",
            Applicability::MaybeIncorrect,
        ),
    };
    let mut diagnostic = Diagnostic::warning("LINT009", message, primary)
        .with_primary_message("empty expression")
        .with_note(note)
        .with_spec_reference("SCXML 1.0 §5.9 Expressions")
        .with_spec_reference("SCXML 1.0 §5.10 Errors");
    let removable = site
        .attribute
        .filter(|attribute| attribute_removal_is_safe(site.element, &attribute.name.local_name));
    diagnostic = diagnostic.with_help(match (site.kind, removable.is_some()) {
        (ExpressionKind::Condition, true) => {
            "write the intended condition, or remove `cond` if the transition should be unconditional".to_owned()
        }
        (ExpressionKind::Condition, false) => format!(
            "write the condition this `<{element}>` branch tests; `cond` is required on `<{element}>`, so a last branch that always runs is written as `<else/>`"
        ),
        (_, true) => "write the intended expression, or remove the attribute".to_owned(),
        (_, false) => format!(
            "write the intended expression; `<{element}>` needs `{what}`, so it cannot simply be removed"
        ),
    });
    if let Some(attribute) = removable {
        diagnostic = diagnostic.with_fix(Fix {
            message: format!("remove the empty `{what}`"),
            applicability,
            edits: vec![FixEdit {
                span: attribute_removal_span(document, attribute.span),
                replacement: String::new(),
            }],
        });
    }
    diagnostic
}

fn not_checked_diagnostic(data_model: &str, site: &Site<'_>) -> Diagnostic {
    let help = if data_model == xpath::DATA_MODEL {
        "split the expression into smaller ones, assigning intermediate results to `<data>` variables with `<assign>`, so each fragment stays within the bound"
    } else if site.kind == ExpressionKind::Script {
        "split the script into smaller `<script>` elements or helper functions, so each fragment stays within the bound"
    } else {
        "split the expression into smaller ones, assigning intermediate results to variables with `<assign>` or computing them in a `<script>` function, so each fragment stays within the bound"
    };
    Diagnostic::error(
        "LIMIT003",
        format!("`{}` was declined by the data-model syntax checker", site.what),
        site.value_span,
    )
    .with_primary_message("over the data model's size or nesting bound")
    .with_note("the data model applies the same bound when it evaluates this fragment, so it would be rejected at run time")
    .with_help(help)
    .with_spec_reference("SCXML 1.0 §5.9 Expressions")
}

pub(crate) mod data_ids;
mod effects;
mod habits;
mod in_predicate;
mod xpath;

pub(crate) use data_ids::{EcmaScriptDataIdRule, ReservedDataIdRule};
pub(crate) use in_predicate::InPredicateRule;

#[cfg(test)]
mod tests;
