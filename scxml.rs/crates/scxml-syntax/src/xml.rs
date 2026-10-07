//! Source-preserving XML parsing through `quick-xml`.

use std::fmt;

use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::{NsReader, Reader};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_types::wasm_bound;

use crate::dtd::{Content, Entities};
use crate::source::{
    Attribute, Document, Element, ExpandedName, NamespaceDeclaration, Node, SourceId, SourceText,
    Span, Text,
};

/// Closed classification of XML trust-boundary failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmlErrorKind {
    /// Malformed XML, invalid UTF-8, unbound prefix, or bad document shape.
    Malformed,
    /// A configured resource budget was exceeded.
    ResourceLimit,
    /// Caller-supplied limits were invalid (a zero maximum).
    InvalidLimits,
    /// A general entity reference that names neither one of the five
    /// predefined XML entities (`lt`, `gt`, `amp`, `apos`, `quot`) nor an
    /// entity the document's internal DTD subset declares.
    UndefinedEntityReference,
    /// An entity the parser does not read, relied on by the document: a
    /// reference in content to an external entity (never read, XML 1.0
    /// §4.4.3: "If a non-validating processor does not include the
    /// replacement text, it MUST inform the application that it recognized,
    /// but did not read, the entity."), or to an entity only declarations
    /// after a parameter entity that is not read could declare (§5.1).
    UnsupportedDtdConstruct,
    /// A character reference that is lexically invalid or names a code point
    /// outside the XML 1.0 `Char` production.
    InvalidCharacterReference,
}

/// XML well-formedness or namespace-resolution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmlError {
    /// Stable failure classification.
    pub kind: XmlErrorKind,
    /// Human-readable, project-owned message (no parser byte positions).
    pub message: String,
    /// Best available source range: the responsible bytes when `detail` is
    /// classified.
    pub span: Span,
    /// Structured facts about the violated well-formedness constraint, so
    /// diagnostics can offer exact labels and repairs without re-parsing.
    pub detail: XmlErrorDetail,
}

/// Closed classification of well-formedness violations that AI authors and
/// editors commonly produce. Every span indexes the original source bytes
/// (including a leading byte order mark).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum XmlErrorDetail {
    /// No further structure is known; `message` and `span` are all there is.
    Unclassified,
    /// Non-whitespace character data, a reference, or CDATA outside the root
    /// element (XML 1.0 §2.1 `document ::= prolog element Misc*`), for example
    /// a Markdown code fence or chat prose around the document. `removals`
    /// are non-overlapping, ascending byte ranges whose deletion removes every
    /// offending run found (prolog runs, then the whole epilog tail).
    TextOutsideRoot {
        /// Ranges to delete.
        removals: Vec<Span>,
    },
    /// A literal `<` in an attribute value (XML 1.0 §3.1 WFC: No < in
    /// Attribute Values). `occurrences` locates every `<` in that value.
    LessThanInAttributeValue {
        /// One-byte spans of each `<` in the offending value.
        occurrences: Vec<Span>,
    },
    /// A `<` in character data that cannot start markup because no valid
    /// element name follows it (XML 1.0 §2.4), typically a comparison in a
    /// script or expression. `element` is the innermost open element.
    LessThanInText {
        /// Lexical name of the element containing the `<`, if any.
        element: Option<String>,
    },
    /// A comment containing `--` or ending in `--->` (XML 1.0 §2.5).
    DoubleHyphenInComment {
        /// The comment content between `<!--` and the first `-->`.
        content: Span,
    },
    /// An XML declaration that is not at the very start of the document
    /// (XML 1.0 §2.8). `leading` is the whitespace-only range before it when
    /// deleting that range would make the declaration first.
    MisplacedXmlDeclaration {
        /// Whitespace-only bytes preceding the declaration, if that is all
        /// that precedes it.
        leading: Option<Span>,
    },
    /// The same attribute name twice in one start tag (XML 1.0 §3.1 WFC:
    /// Unique Att Spec). The primary span is the repeated attribute.
    DuplicateAttribute {
        /// Name bytes of the first occurrence.
        first: Span,
        /// The repeated attribute together with its preceding whitespace.
        removal: Span,
    },
    /// A second top-level element (XML 1.0 §2.1). The primary span is the
    /// second root's name.
    MultipleRootElements {
        /// Name bytes of the first root element.
        first_root: Span,
    },
}

/// Deterministic resource limits for untrusted XML input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XmlLimits {
    /// Maximum UTF-8 source bytes.
    pub max_source_bytes: usize,
    /// Maximum simultaneously open element depth, including the root.
    pub max_depth: usize,
    /// Maximum parser events, including ignored declarations and comments.
    pub max_events: usize,
    /// Maximum element and text nodes retained in the parsed tree.
    pub max_nodes: usize,
    /// Maximum non-namespace attributes and namespace declarations.
    pub max_attributes: usize,
}

impl XmlLimits {
    /// Repository-default XML trust-boundary limits.
    ///
    /// The size and count limits are resource bounds of wasm builds only
    /// ([`scxml_types::wasm_bound`]): natively they are `usize::MAX`. The
    /// depth limit applies to every build, because the parsed tree is walked
    /// (and dropped) recursively, so it guards the native stack too.
    pub const DEFAULT: Self = Self {
        max_source_bytes: wasm_bound(8 * 1024 * 1024),
        max_depth: 256,
        max_events: wasm_bound(1_000_000),
        max_nodes: wasm_bound(250_000),
        max_attributes: wasm_bound(1_000_000),
    };
}

impl Default for XmlLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for XmlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for XmlError {}

/// Parses one UTF-8 XML document while preserving unknown namespaces and spans.
///
/// # Errors
///
/// Returns [`XmlError`] for malformed XML, unbound prefixes, invalid UTF-8,
/// documents without exactly one root element, or any entity/character
/// reference other than the five predefined entities and XML 1.0 `Char`
/// character references (see [`XmlErrorKind`]). References in element text and
/// attribute values are resolved; text interleaved with references is one
/// [`Text`] node whose span covers the exact raw source range. CDATA stays
/// literal.
pub fn parse_xml(source: SourceText) -> Result<Document, XmlError> {
    parse_xml_with_limits(source, XmlLimits::default())
}

/// Parses one UTF-8 XML document under explicit deterministic resource limits.
///
/// The input is borrowed from [`SourceText`], and the returned document owns
/// the same source. Parsing has no external side effects. Every parser loop and
/// retained tree dimension is bounded by `limits`.
///
/// # Errors
///
/// Returns [`XmlError`] for malformed XML, invalid limits, or a resource limit.
pub fn parse_xml_with_limits(source: SourceText, limits: XmlLimits) -> Result<Document, XmlError> {
    let span = observe(Module::Syntax, Operation::SyntaxParseXml);
    let original = source.clone();
    let result = parse_xml_bounded(source, limits)
        .map_err(|error| on_char_boundaries(original.text(), error));
    match &result {
        Ok(_) => span.event(SignalEvent::InputAccepted),
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(error_category(error));
        }
    }
    result
}

/// Verifies that an already constructed document satisfies XML tree budgets.
///
/// This closes the public-structure bypass around [`parse_xml_with_limits`].
/// Traversal is iterative, deterministic, read-only, and bounded by `limits`.
///
/// # Errors
///
/// Returns [`XmlError`] when source bytes, depth, nodes, or attributes exceed
/// their configured maximum, or when any maximum is zero.
pub fn validate_document_limits(document: &Document, limits: XmlLimits) -> Result<(), XmlError> {
    validate_tree_limits(document, limits)
        .map_err(|error| on_char_boundaries(document.source.text(), error))
}

fn validate_tree_limits(document: &Document, limits: XmlLimits) -> Result<(), XmlError> {
    let source_id = document.source.id();
    let source_len = document.source.text().len();
    if source_len > limits.max_source_bytes {
        return Err(limit_error(
            source_id,
            source_len,
            "source bytes",
            limits.max_source_bytes,
        ));
    }
    if limits.max_depth == 0 || limits.max_nodes == 0 || limits.max_attributes == 0 {
        return Err(XmlError {
            kind: XmlErrorKind::InvalidLimits,
            detail: XmlErrorDetail::Unclassified,
            message: "XML tree limits must all be greater than zero".into(),
            span: Span::new(source_id, 0, source_len.min(1)),
        });
    }
    let mut pending = vec![(&document.root, 1usize)];
    let mut node_count = 0usize;
    let mut attribute_count = 0usize;
    while let Some((element, depth)) = pending.pop() {
        if depth > limits.max_depth {
            return Err(limit_error_at(
                source_id,
                element.start_tag_span.start,
                source_len,
                "element depth",
                limits.max_depth,
            ));
        }
        increment_count(
            &mut node_count,
            limits.max_nodes,
            source_id,
            element.start_tag_span.start,
            source_len,
            "tree nodes",
        )?;
        add_attributes(
            &mut attribute_count,
            element
                .attributes
                .len()
                .saturating_add(element.namespaces.len()),
            limits.max_attributes,
            source_id,
            element.start_tag_span.start,
            source_len,
        )?;
        let child_depth = depth.checked_add(1).ok_or_else(|| {
            limit_error_at(
                source_id,
                element.start_tag_span.start,
                source_len,
                "element depth",
                limits.max_depth,
            )
        })?;
        for child in element.children.iter().rev() {
            match child {
                Node::Element(child) => pending.push((child, child_depth)),
                Node::Text(text) => increment_count(
                    &mut node_count,
                    limits.max_nodes,
                    source_id,
                    text.span.start,
                    source_len,
                    "tree nodes",
                )?,
            }
        }
    }
    Ok(())
}

/// UTF-8 encoding of U+FEFF, the byte order mark XML 1.0 §4.3.3 permits
/// before the document entity. It is skipped by the parser, but every span
/// still indexes the original bytes, so offsets are shifted by its length.
const UTF8_BOM: &str = "\u{FEFF}";

/// Read-only parse inputs shared by the event loop helpers.
#[derive(Clone, Copy)]
struct ParseContext<'a> {
    /// The complete source, including a leading BOM.
    text: &'a str,
    /// Byte length of the skipped BOM (0 or 3); added to every parser offset.
    base: usize,
    source_id: SourceId,
    limits: XmlLimits,
}

impl ParseContext<'_> {
    const fn source_len(&self) -> usize {
        self.text.len()
    }
}

/// Character data found outside the root element. Runs are collected so one
/// diagnostic can remove every offending run (for example a Markdown fence
/// both before and after the document) instead of one per validation pass.
#[derive(Default)]
struct OutsideText {
    primary: Option<Span>,
    removals: Vec<Span>,
}

impl OutsideText {
    fn record(&mut self, primary: Span, removal: Span) {
        if self.primary.is_none() {
            self.primary = Some(primary);
        }
        match self.removals.last_mut() {
            Some(previous) if previous.end == removal.start => previous.end = removal.end,
            _ => self.removals.push(removal),
        }
    }

    fn is_empty(&self) -> bool {
        self.primary.is_none()
    }

    fn into_error(self) -> Option<XmlError> {
        self.primary.map(|primary| XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::TextOutsideRoot {
                removals: self.removals,
            },
            message: "text outside the root element".into(),
            span: primary,
        })
    }
}

fn parse_xml_bounded(source: SourceText, limits: XmlLimits) -> Result<Document, XmlError> {
    let source_id = source.id();
    let source_len = source.text().len();
    if source_len > limits.max_source_bytes {
        return Err(limit_error(
            source_id,
            source_len,
            "source bytes",
            limits.max_source_bytes,
        ));
    }
    if limits.max_depth == 0
        || limits.max_events == 0
        || limits.max_nodes == 0
        || limits.max_attributes == 0
    {
        return Err(XmlError {
            kind: XmlErrorKind::InvalidLimits,
            detail: XmlErrorDetail::Unclassified,
            message: "XML parser limits must all be greater than zero".into(),
            span: Span::new(source_id, 0, source_len.min(1)),
        });
    }
    let text = source.text();
    let context = ParseContext {
        text,
        base: if text.starts_with(UTF8_BOM) {
            UTF8_BOM.len()
        } else {
            0
        },
        source_id,
        limits,
    };
    let mut outside = OutsideText::default();
    // The reader would silently strip a second BOM (shifting every
    // offset); as character data before the root it is not well formed.
    if text
        .get(context.base..)
        .is_some_and(|body| context.base > 0 && body.starts_with(UTF8_BOM))
    {
        let second = Span::new(
            source_id,
            context.base,
            context.base.saturating_add(UTF8_BOM.len()),
        );
        outside.record(second, second);
        return Err(outside.into_error().unwrap_or_else(|| XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: "text outside the root element".into(),
            span: second,
        }));
    }
    let mut root_seen = false;
    let parsed = parse_events(context, &mut outside, &mut root_seen);
    // Text around a document (a code fence, chat prose) is the root
    // cause of any later failure, so it is reported first; without
    // any root element the document is simply missing.
    if root_seen && let Some(error) = outside.into_error() {
        return Err(error);
    }
    let root = parsed?;
    Ok(Document { source, root })
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded event loop owns every XML token kind, depth, and node-count check"
)]
fn parse_events(
    context: ParseContext<'_>,
    outside: &mut OutsideText,
    root_seen: &mut bool,
) -> Result<Element, XmlError> {
    let ParseContext {
        text,
        base,
        source_id,
        limits,
    } = context;
    let source_len = context.source_len();
    let mut reader = NsReader::from_str(text.get(base..).unwrap_or_default());
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = true;
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut root = None;
    let mut event_count = 0usize;
    let mut node_count = 0usize;
    let mut attribute_count = 0usize;
    // True while the previous event was character data or a reference,
    // so adjacent text and resolved references form one text node.
    let mut text_run_open = false;
    // The general entities the internal DTD subset declares (XML 1.0 §5.1).
    let mut entities = Entities::default();

    loop {
        event_count = event_count.checked_add(1).ok_or_else(|| {
            limit_error(source_id, source_len, "parser events", limits.max_events)
        })?;
        if event_count > limits.max_events {
            return Err(limit_error(
                source_id,
                source_len,
                "parser events",
                limits.max_events,
            ));
        }
        let event_start = buffer_position(&reader, context)?;
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(error) => {
                let reader_end = buffer_position(&reader, context).unwrap_or(source_len);
                return Err(reader_error(
                    context,
                    &error,
                    event_start,
                    reader_end,
                    stack.last(),
                ));
            }
        };
        let event_end = buffer_position(&reader, context)?;
        let continues_text_run = text_run_open;
        text_run_open = matches!(event, Event::Text(_) | Event::GeneralRef(_));
        let outside_root = stack.is_empty();
        match event {
            Event::Start(start) => {
                check_start(context, &start, event_start, &stack, root.as_ref())?;
                *root_seen = true;
                if stack.len() >= limits.max_depth {
                    return Err(limit_error_at(
                        source_id,
                        event_start,
                        source_len,
                        "element depth",
                        limits.max_depth,
                    ));
                }
                increment_count(
                    &mut node_count,
                    limits.max_nodes,
                    source_id,
                    event_start,
                    source_len,
                    "tree nodes",
                )?;
                let element = parse_start(
                    reader.decoder(),
                    text,
                    &start,
                    Span::new(source_id, event_start, event_end),
                    false,
                    TagOrigin::Source,
                    &mut entities,
                    &stack,
                )?;
                add_attributes(
                    &mut attribute_count,
                    element
                        .attributes
                        .len()
                        .saturating_add(element.namespaces.len()),
                    limits.max_attributes,
                    source_id,
                    event_start,
                    source_len,
                )?;
                stack.push(element);
            }
            Event::Empty(start) => {
                check_start(context, &start, event_start, &stack, root.as_ref())?;
                *root_seen = true;
                if stack.len() >= limits.max_depth {
                    return Err(limit_error_at(
                        source_id,
                        event_start,
                        source_len,
                        "element depth",
                        limits.max_depth,
                    ));
                }
                increment_count(
                    &mut node_count,
                    limits.max_nodes,
                    source_id,
                    event_start,
                    source_len,
                    "tree nodes",
                )?;
                let mut element = parse_start(
                    reader.decoder(),
                    text,
                    &start,
                    Span::new(source_id, event_start, event_end),
                    true,
                    TagOrigin::Source,
                    &mut entities,
                    &stack,
                )?;
                add_attributes(
                    &mut attribute_count,
                    element
                        .attributes
                        .len()
                        .saturating_add(element.namespaces.len()),
                    limits.max_attributes,
                    source_id,
                    event_start,
                    source_len,
                )?;
                element.span = Span::new(source_id, event_start, event_end);
                attach_element(element, &mut stack, &mut root, source_id, event_start)?;
            }
            Event::End(_) => {
                let Some(mut element) = stack.pop() else {
                    return Err(XmlError {
                        kind: XmlErrorKind::Malformed,
                        detail: XmlErrorDetail::Unclassified,
                        message: "end tag has no matching start tag".into(),
                        span: Span::new(source_id, event_start, event_end),
                    });
                };
                element.span.end = event_end;
                attach_element(element, &mut stack, &mut root, source_id, event_start)?;
            }
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) if outside_root => {
                if record_outside(context, outside, event_start, event_end, root.is_some()) {
                    break;
                }
            }
            Event::Text(characters) => {
                let decoded = characters.decode().map_err(|error| XmlError {
                    kind: XmlErrorKind::Malformed,
                    detail: XmlErrorDetail::Unclassified,
                    message: format!("character data is not valid text: {error}"),
                    span: Span::new(source_id, event_start, event_end),
                })?;
                let value = unescape(&decoded).map_err(|error| XmlError {
                    kind: XmlErrorKind::Malformed,
                    detail: XmlErrorDetail::Unclassified,
                    message: error.to_string(),
                    span: Span::new(source_id, event_start, event_end),
                })?;
                append_text(
                    &value,
                    Span::new(source_id, event_start, event_end),
                    continues_text_run,
                    &mut stack,
                    &mut node_count,
                    limits.max_nodes,
                    source_len,
                )?;
            }
            Event::GeneralRef(reference) => {
                let span = Span::new(source_id, event_start, event_end);
                // XML 1.0 §4.4.2: a declared internal entity is "Included".
                let name = std::str::from_utf8(&reference).ok();
                let declared = name
                    .filter(|_| !entities.is_empty())
                    .map(|name| entities.content(name, span))
                    .transpose()?
                    .flatten();
                match declared {
                    Some(Content::Markup(replacement)) => {
                        include_markup(
                            &replacement,
                            name.unwrap_or_default(),
                            span,
                            context,
                            &mut stack,
                            &mut entities,
                            &mut Counts {
                                events: &mut event_count,
                                nodes: &mut node_count,
                                attributes: &mut attribute_count,
                            },
                            &mut Vec::new(),
                        )?;
                        text_run_open = false;
                    }
                    Some(Content::Text(expanded)) => append_text(
                        &expanded,
                        span,
                        continues_text_run,
                        &mut stack,
                        &mut node_count,
                        limits.max_nodes,
                        source_len,
                    )?,
                    None => {
                        let mut buffer = [0u8; 4];
                        let value = resolve_general(&reference, span, &entities)?;
                        append_text(
                            value.encode_utf8(&mut buffer),
                            span,
                            continues_text_run,
                            &mut stack,
                            &mut node_count,
                            limits.max_nodes,
                            source_len,
                        )?;
                    }
                }
            }
            Event::CData(cdata) => {
                increment_count(
                    &mut node_count,
                    limits.max_nodes,
                    source_id,
                    event_start,
                    source_len,
                    "tree nodes",
                )?;
                let value = cdata.decode().map_err(|error| XmlError {
                    kind: XmlErrorKind::Malformed,
                    detail: XmlErrorDetail::Unclassified,
                    message: format!("CDATA section is not valid text: {error}"),
                    span: Span::new(source_id, event_start, event_end),
                })?;
                attach_text(
                    value.into_owned(),
                    Span::new(source_id, event_start, event_end),
                    &mut stack,
                );
            }
            Event::Decl(_) => {
                if event_start != base && outside.is_empty() {
                    return Err(misplaced_declaration(
                        context,
                        event_start,
                        event_end,
                        root.is_none(),
                    ));
                }
            }
            Event::Eof => break,
            Event::DocType(_) => {
                // The content between `<!DOCTYPE` and the closing `>`.
                let content_start = event_start.saturating_add("<!DOCTYPE".len());
                let content_end = event_end.saturating_sub(1);
                let doctype_content = text.get(content_start..content_end).unwrap_or_default();
                entities = Entities::from_doctype(
                    doctype_content,
                    content_start,
                    Span::new(source_id, event_start, event_end),
                )?;
            }
            Event::PI(_) | Event::Comment(_) => {}
        }
    }

    if let Some(element) = stack.last() {
        return Err(XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: format!("unclosed element `{}`", element.lexical_name),
            span: element.start_tag_span,
        });
    }
    root.ok_or_else(|| XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::Unclassified,
        message: "XML document has no root element".into(),
        span: Span::new(source_id, 0, source_len.min(1)),
    })
}

/// Widens an error span to whole characters after any leading BOM, so
/// byte-granular fallbacks (a one-byte "here" at offset 0, a clamped end)
/// never split a UTF-8 sequence or point inside the byte order mark.
fn on_char_boundaries(text: &str, mut error: XmlError) -> XmlError {
    let base = if text.starts_with(UTF8_BOM) {
        UTF8_BOM.len()
    } else {
        0
    };
    let mut start = error.span.start.clamp(base, text.len());
    while start > base && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = error.span.end.clamp(start, text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    if end == start
        && let Some(character) = text.get(start..).and_then(|rest| rest.chars().next())
        && error.span.end > error.span.start
    {
        end = start.saturating_add(character.len_utf8());
    }
    error.span = Span::new(error.span.source, start, end);
    error
}

/// XML 1.0 `S` production: space, tab, carriage return, line feed.
const fn is_xml_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// Records one run of character data outside the root element. Returns true
/// when parsing should stop: after the root, everything to the end of the
/// document is removed as one epilog tail, so later bytes cannot matter.
fn record_outside(
    context: ParseContext<'_>,
    outside: &mut OutsideText,
    start: usize,
    end: usize,
    after_root: bool,
) -> bool {
    let raw = context.text.get(start..end).unwrap_or_default();
    let Some(first) = raw.bytes().position(|byte| !is_xml_whitespace(byte)) else {
        return false;
    };
    let line = raw.get(first..).unwrap_or_default();
    let line_len = line.find(['\r', '\n']).unwrap_or(line.len());
    let line_len = line
        .get(..line_len)
        .map_or(line_len, |line| line.trim_end_matches([' ', '\t']).len());
    let primary = Span::new(
        context.source_id,
        start.saturating_add(first),
        start.saturating_add(first).saturating_add(line_len),
    );

    if after_root {
        let text = context.text;
        let terminator = if text.ends_with("\r\n") {
            2
        } else {
            usize::from(text.ends_with(['\n', '\r']))
        };
        let tail_end = text.len().saturating_sub(terminator).max(primary.end);
        outside.record(primary, Span::new(context.source_id, start, tail_end));
        true
    } else {
        outside.record(primary, Span::new(context.source_id, start, end));
        false
    }
}

fn misplaced_declaration(
    context: ParseContext<'_>,
    start: usize,
    end: usize,
    before_root: bool,
) -> XmlError {
    let leading = (before_root
        && context
            .text
            .get(context.base..start)
            .is_some_and(|prefix| prefix.bytes().all(is_xml_whitespace)))
    .then(|| Span::new(context.source_id, context.base, start));

    XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::MisplacedXmlDeclaration { leading },
        message: "the XML declaration must be the very first thing in the document".into(),
        span: Span::new(context.source_id, start, end),
    }
}

/// Rejects a start tag that is really a stray `<` in text, or a second root.
fn check_start(
    context: ParseContext<'_>,
    start: &BytesStart<'_>,
    event_start: usize,
    stack: &[Element],
    root: Option<&Element>,
) -> Result<(), XmlError> {
    if less_than_without_name(context.text, event_start) {
        return Err(less_than_in_text(context, event_start, stack.last()));
    }
    if stack.is_empty()
        && let Some(first) = root
    {
        let name_start = event_start.saturating_add(1);
        let first_start = first.start_tag_span.start.saturating_add(1);
        return Err(XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::MultipleRootElements {
                first_root: Span::new(
                    context.source_id,
                    first_start,
                    first_start.saturating_add(first.lexical_name.len()),
                ),
            },
            message: format!(
                "second root element `<{}>`",
                String::from_utf8_lossy(start.name().as_ref())
            ),
            span: Span::new(
                context.source_id,
                name_start,
                name_start.saturating_add(start.name().as_ref().len()),
            ),
        });
    }
    Ok(())
}

fn less_than_in_text(context: ParseContext<'_>, offset: usize, open: Option<&Element>) -> XmlError {
    XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::LessThanInText {
            element: open.map(|element| element.lexical_name.clone()),
        },
        message: "unescaped `<` in character data: no element name follows it".into(),
        span: Span::new(context.source_id, offset, offset.saturating_add(1)),
    }
}

/// True when the `<` at `offset` cannot start markup: it is not followed by
/// `/`, `!`, `?`, or a valid XML `Name` (XML 1.0 §2.3, §3.1).
fn less_than_without_name(text: &str, offset: usize) -> bool {
    let Some(rest) = text.get(offset..).and_then(|rest| rest.strip_prefix('<')) else {
        return false;
    };
    let mut characters = rest.chars();

    match characters.next() {
        None => true,
        Some('/' | '!' | '?') => false,
        Some(first) if !is_name_start_char(first) => true,
        Some(_) => characters
            .take_while(|character| !matches!(character, ' ' | '\t' | '\r' | '\n' | '/' | '>'))
            .any(|character| !is_name_char(character)),
    }
}

/// XML 1.0 (Fifth Edition) `NameStartChar`.
const fn is_name_start_char(character: char) -> bool {
    matches!(
        character,
        ':' | 'A'..='Z'
            | '_'
            | 'a'..='z'
            | '\u{C0}'..='\u{D6}'
            | '\u{D8}'..='\u{F6}'
            | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}'
            | '\u{37F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

/// XML 1.0 (Fifth Edition) `NameChar`.
const fn is_name_char(character: char) -> bool {
    is_name_start_char(character)
        || matches!(
            character,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}'
        )
}

/// Maps one reader failure to a project-owned, position-free error.
fn reader_error(
    context: ParseContext<'_>,
    error: &quick_xml::Error,
    event_start: usize,
    reader_end: usize,
    open: Option<&Element>,
) -> XmlError {
    let source_id = context.source_id;

    if matches!(
        error,
        quick_xml::Error::IllFormed(quick_xml::errors::IllFormedError::DoubleHyphenInComment)
    ) && let Some((primary, content)) = comment_violation(context, event_start)
    {
        XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::DoubleHyphenInComment { content },
            message: "a comment must not contain `--` or end with `-`".into(),
            span: primary,
        }
    } else if less_than_without_name(context.text, event_start) {
        less_than_in_text(context, event_start, open)
    } else {
        XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: describe_reader_error(error),
            span: error_span(source_id, event_start, reader_end, context.source_len()),
        }
    }
}

/// Locates the offending `--` (or trailing `-`) in the comment starting at
/// `start`, and the comment content up to the first `-->`.
fn comment_violation(context: ParseContext<'_>, start: usize) -> Option<(Span, Span)> {
    let content_start = start.checked_add(4)?;

    (context.text.get(start..content_start) == Some("<!--"))
        .then(|| context.text.get(content_start..))
        .flatten()
        .and_then(|rest| {
            let close = rest.find("-->")?;
            let body = rest.get(..close)?;
            let (relative_start, relative_end) = body
                .find("--")
                .map(|index| (index, index.saturating_add(2)))
                .or_else(|| {
                    body.ends_with('-')
                        .then(|| (close.saturating_sub(1), close))
                })?;
            Some((
                Span::new(
                    context.source_id,
                    content_start.saturating_add(relative_start),
                    content_start.saturating_add(relative_end),
                ),
                Span::new(
                    context.source_id,
                    content_start,
                    content_start.saturating_add(close),
                ),
            ))
        })
}

/// Project-owned wording for every reader failure class: no parser-internal
/// byte positions and no grammar-implementation vocabulary.
fn describe_reader_error(error: &quick_xml::Error) -> String {
    use quick_xml::errors::{IllFormedError, SyntaxError};

    match error {
        quick_xml::Error::Syntax(syntax) => match syntax {
            SyntaxError::InvalidBangMarkup => "unrecognized markup after `<!`: expected a comment `<!--`, a CDATA section `<![CDATA[`, or `<!DOCTYPE`".to_owned(),
            SyntaxError::UnclosedPI => "processing instruction is not closed: `?>` is missing before the end of the document".to_owned(),
            SyntaxError::UnclosedXmlDecl => "XML declaration is not closed: `?>` is missing before the end of the document".to_owned(),
            SyntaxError::UnclosedComment => "comment is not closed: `-->` is missing before the end of the document".to_owned(),
            SyntaxError::UnclosedDoctype => "DOCTYPE declaration is not closed: `>` is missing before the end of the document".to_owned(),
            SyntaxError::UnclosedCData => "CDATA section is not closed: `]]>` is missing before the end of the document".to_owned(),
            SyntaxError::UnclosedTag => "tag is not closed: `>` is missing before the end of the document".to_owned(),
            SyntaxError::UnclosedSingleQuotedAttributeValue => "attribute value is not closed: the closing `'` is missing before the end of the document".to_owned(),
            SyntaxError::UnclosedDoubleQuotedAttributeValue => "attribute value is not closed: the closing `\"` is missing before the end of the document".to_owned(),
        },
        quick_xml::Error::IllFormed(ill_formed) => match ill_formed {
            IllFormedError::MissingDeclVersion(None) => "the XML declaration has no `version`".to_owned(),
            IllFormedError::MissingDeclVersion(Some(found)) => format!("the XML declaration must start with `version`, found `{found}`"),
            IllFormedError::UnknownVersion => "unsupported XML version: SCXML documents use `version=\"1.0\"`".to_owned(),
            IllFormedError::MissingDoctypeName => "`<!DOCTYPE>` has no document type name".to_owned(),
            IllFormedError::MissingEndTag(name) => format!("`<{name}>` is never closed"),
            IllFormedError::UnmatchedEndTag(name) => format!("end tag `</{name}>` has no matching start tag"),
            // `scxml-validator-core` recognizes this exact wording to label
            // both tags; keep it in sync with `mismatched_end_tag` there.
            IllFormedError::MismatchedEndTag { expected, found } => format!("expected `</{expected}>`, but `</{found}>` was found"),
            IllFormedError::DoubleHyphenInComment => "a comment must not contain `--` or end with `-`".to_owned(),
            IllFormedError::UnclosedReference => "entity or character reference is not closed: `;` is missing".to_owned(),
        },
        quick_xml::Error::InvalidAttr(attribute) => describe_attribute_error(attribute),
        quick_xml::Error::Encoding(_) => "the document is not valid UTF-8".to_owned(),
        quick_xml::Error::Escape(escape) => escape.to_string(),
        quick_xml::Error::Namespace(namespace) => namespace.to_string(),
        quick_xml::Error::Io(_) => "the document could not be read".to_owned(),
    }
}

/// Position-free wording for attribute syntax failures.
fn describe_attribute_error(error: &quick_xml::events::attributes::AttrError) -> String {
    use quick_xml::events::attributes::AttrError;

    match error {
        AttrError::ExpectedEq(_) => {
            "an attribute name must be followed by `=` and a quoted value".to_owned()
        }
        AttrError::ExpectedValue(_) => {
            "`=` must be followed by a quoted attribute value".to_owned()
        }
        // `scxml-validator-core` recognizes this wording to quote the value.
        AttrError::UnquotedValue(_) => "attribute value must be enclosed in `\"` or `'`".to_owned(),
        AttrError::ExpectedQuote(_, quote) => format!(
            "attribute value is missing its closing `{}`",
            char::from(*quote)
        ),
        AttrError::Duplicated(_, _) => "duplicate attribute".to_owned(),
    }
}

fn increment_count(
    count: &mut usize,
    limit: usize,
    source_id: SourceId,
    offset: usize,
    source_len: usize,
    resource: &str,
) -> Result<(), XmlError> {
    *count = count
        .checked_add(1)
        .ok_or_else(|| limit_error_at(source_id, offset, source_len, resource, limit))?;
    if *count > limit {
        return Err(limit_error_at(
            source_id, offset, source_len, resource, limit,
        ));
    }
    Ok(())
}

fn add_attributes(
    count: &mut usize,
    added: usize,
    limit: usize,
    source_id: SourceId,
    offset: usize,
    source_len: usize,
) -> Result<(), XmlError> {
    *count = count
        .checked_add(added)
        .ok_or_else(|| limit_error_at(source_id, offset, source_len, "attributes", limit))?;
    if *count > limit {
        return Err(limit_error_at(
            source_id,
            offset,
            source_len,
            "attributes",
            limit,
        ));
    }
    Ok(())
}

fn limit_error(source_id: SourceId, source_len: usize, resource: &str, limit: usize) -> XmlError {
    limit_error_at(source_id, 0, source_len, resource, limit)
}

fn limit_error_at(
    source_id: SourceId,
    offset: usize,
    source_len: usize,
    resource: &str,
    limit: usize,
) -> XmlError {
    XmlError {
        kind: XmlErrorKind::ResourceLimit,
        detail: XmlErrorDetail::Unclassified,
        message: format!("XML {resource} limit exceeded (maximum {limit})"),
        span: error_span(source_id, offset, offset.saturating_add(1), source_len),
    }
}

/// Where one start tag was read.
#[derive(Clone, Copy)]
enum TagOrigin {
    /// The document entity: spans index the tag's own source bytes.
    Source,
    /// The replacement text of a general entity included in content (XML
    /// 1.0 §4.4.2): every span is the including reference's.
    Included,
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one start tag needs the decoder, source, span, shape, origin, entities, and scopes"
)]
fn parse_start(
    decoder: quick_xml::encoding::Decoder,
    text: &str,
    start: &BytesStart<'_>,
    tag_span: Span,
    empty: bool,
    origin: TagOrigin,
    entities: &mut Entities,
    scopes: &[Element],
) -> Result<Element, XmlError> {
    let source_id = tag_span.source;
    let lexical_name = decode(start.name().as_ref(), source_id, tag_span.start)?;
    let local_name = decode(start.local_name().as_ref(), source_id, tag_span.start)?;
    let attributes_base = tag_span.start + 1 + start.name().as_ref().len();
    let raw_attributes = start.attributes_raw();
    let mut scan_offset = 0;
    let mut namespaces = Vec::new();
    // Non-namespace attributes before name resolution: lexical name,
    // value, span, and value span.
    let mut specified: Vec<(String, String, Span, Span)> = Vec::new();
    // Name spans of the attributes seen so far, for XML 1.0 §3.1
    // WFC: Unique Att Spec (checked here so both occurrences can be
    // located exactly).
    let mut seen: std::collections::BTreeMap<Vec<u8>, Span> = std::collections::BTreeMap::new();

    for parsed in start.attributes().with_checks(false) {
        let parsed = parsed.map_err(|error| XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: describe_attribute_error(&error),
            span: tag_span,
        })?;
        let lexical = decode(parsed.key.as_ref(), source_id, tag_span.start)?;
        let (span, name_span, value_span) = match origin {
            TagOrigin::Included => (tag_span, tag_span, tag_span),
            TagOrigin::Source => {
                let relative_key =
                    locate_attribute(raw_attributes, parsed.key.as_ref(), scan_offset)
                        .unwrap_or(scan_offset);
                let value_relative =
                    locate_value(raw_attributes, relative_key, parsed.key.as_ref().len());
                let value_end = value_relative + parsed.value.as_ref().len();
                let full_end = (value_end + 1).min(raw_attributes.len());
                scan_offset = full_end;
                let span = Span::new(
                    source_id,
                    attributes_base + relative_key,
                    attributes_base + full_end,
                );
                (
                    span,
                    Span::new(
                        source_id,
                        span.start,
                        span.start.saturating_add(parsed.key.as_ref().len()),
                    ),
                    Span::new(
                        source_id,
                        attributes_base + value_relative,
                        attributes_base + value_end,
                    ),
                )
            }
        };
        if let Some(first) = seen.get(parsed.key.as_ref()) {
            return Err(duplicate_attribute(text, &lexical, *first, span));
        }
        seen.insert(parsed.key.as_ref().to_vec(), name_span);
        reject_less_than_in_value(parsed.value.as_ref(), &lexical, value_span)?;
        validate_attribute_references(parsed.value.as_ref(), value_span, entities)?;
        // XML 1.0 §4.4.5: a declared internal entity is "Included in
        // Literal"; its replacement text is normalized like the value
        // (§3.3.3). The expansion was bounded and charged above.
        let value = parsed
            .decoded_and_normalized_value_with(
                quick_xml::XmlVersion::Implicit1_0,
                decoder,
                crate::dtd::MAX_ENTITY_DEPTH.saturating_add(1),
                |name| attribute_entity(entities, name),
            )
            .map_err(|error| XmlError {
                kind: XmlErrorKind::Malformed,
                detail: XmlErrorDetail::Unclassified,
                message: error.to_string(),
                span: value_span,
            })?
            .into_owned();
        // §3.3.3: a value of a declared type other than CDATA is further
        // normalized.
        let value = entities.normalize_declared(&lexical_name, &lexical, value);

        if lexical == "xmlns" || lexical.starts_with("xmlns:") {
            let (uri, tag) = split_tag(value);
            namespaces.push(NamespaceDeclaration {
                prefix: lexical.strip_prefix("xmlns:").map(str::to_owned),
                uri,
                tag,
                span,
                value_span,
            });
            continue;
        }
        specified.push((lexical, value, span, value_span));
    }
    // XML 1.0 §5.1: the attribute-list declarations supply the default
    // value of every attribute the tag does not specify; a defaulted
    // `xmlns` attribute declares a namespace like a specified one. Supplied
    // names and text are charged to the document's expansion budget first,
    // so defaults cannot multiply memory per element, even empty ones.
    let supplied: usize = entities
        .defaults(&lexical_name)
        .map(|default| default.name.len().saturating_add(default.value.len()))
        .fold(0, usize::saturating_add);
    entities.charge(supplied, tag_span)?;
    for default in entities.defaults(&lexical_name) {
        let prefix = if default.name == "xmlns" {
            Some(None)
        } else {
            default.name.strip_prefix("xmlns:").map(Some)
        };
        match prefix {
            Some(prefix) => {
                // `seen` holds every name the tag specifies, `xmlns`
                // declarations included.
                if !seen.contains_key(default.name.as_bytes()) {
                    let (uri, tag) = split_tag(default.value.to_owned());
                    namespaces.push(NamespaceDeclaration {
                        prefix: prefix.map(str::to_owned),
                        uri,
                        tag,
                        span: default.span,
                        value_span: default.span,
                    });
                }
            }
            None => {
                if !seen.contains_key(default.name.as_bytes()) {
                    specified.push((
                        default.name.to_owned(),
                        default.value.to_owned(),
                        default.span,
                        default.span,
                    ));
                }
            }
        }
    }
    for declaration in &namespaces {
        check_namespace_declaration(declaration)?;
    }
    let element_prefix = lexical_name.split_once(':').map(|(prefix, _)| prefix);
    let namespace_uri = resolve_prefix(element_prefix, &namespaces, scopes, NameKind::Element)
        .map_err(|prefix| unbound_prefix(prefix, tag_span))?;
    let invoke = namespace_uri.as_deref() == Some(SCXML_NAMESPACE) && local_name == "invoke";
    let mut attributes = Vec::with_capacity(specified.len());
    for (lexical, value, span, value_span) in specified {
        let (namespace, local) = match lexical.split_once(':') {
            Some((prefix, local)) => (
                resolve_prefix(Some(prefix), &namespaces, scopes, NameKind::Attribute)
                    .map_err(|prefix| unbound_prefix(prefix, span))?,
                local.to_owned(),
            ),
            None => (None, lexical.clone()),
        };
        let (value, tag) = if invoke && namespace.is_none() && local == "type" {
            split_tag(value)
        } else {
            (value, None)
        };
        attributes.push(Attribute {
            lexical_name: lexical,
            name: ExpandedName::new(namespace, local),
            value,
            tag,
            span,
            value_span,
        });
    }

    Ok(Element {
        lexical_name,
        name: ExpandedName::new(namespace_uri, local_name),
        start_tag_span: tag_span,
        span: if empty {
            tag_span
        } else {
            Span::new(source_id, tag_span.start, tag_span.end)
        },
        namespaces,
        attributes,
        children: Vec::new(),
    })
}

/// The SCXML namespace, whose `<invoke type>` is read through
/// [`split_tag`].
const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";

/// Reads one namespace declaration or `<invoke type>` value as the document
/// is ingested: a repository package URI's trailing `@<tag>` is split off
/// ([`scxml_types::strip_tag`]), once, so nothing after parsing sees it. A
/// malformed pin is kept exactly as written, for the CLI's resolver to
/// reject.
fn split_tag(value: String) -> (String, Option<String>) {
    match scxml_types::strip_tag(&value) {
        Some((tagless, Some(tag))) => (tagless.to_owned(), Some(tag.to_owned())),
        Some((_, None)) | None => (value, None),
    }
}

/// The XML namespace name, bound to the `xml` prefix by definition.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The namespace name of `xmlns` attributes.
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// The kind of qualified name a prefix is resolved for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NameKind {
    /// An element name: an unprefixed one is in the default namespace.
    Element,
    /// An attribute name: an unprefixed one is in no namespace.
    Attribute,
}

/// The namespace `prefix` (the default namespace when `None`) names in the
/// scope of an element declaring `own` inside the open elements `scopes`
/// (Namespaces in XML 1.0 §6). An element's unprefixed name is in the
/// default namespace; an attribute's unprefixed name is in none. `Err`
/// carries an unbound prefix.
fn resolve_prefix<'a>(
    prefix: Option<&'a str>,
    own: &[NamespaceDeclaration],
    scopes: &[Element],
    kind: NameKind,
) -> Result<Option<String>, &'a str> {
    if prefix == Some("xml") {
        return Ok(Some(XML_NAMESPACE.to_owned()));
    }
    if prefix.is_none() && kind == NameKind::Attribute {
        return Ok(None);
    }
    let declared = own
        .iter()
        .rev()
        .chain(
            scopes
                .iter()
                .rev()
                .flat_map(|scope| scope.namespaces.iter().rev()),
        )
        .find(|declaration| declaration.prefix.as_deref() == prefix);
    match (declared, prefix) {
        (Some(declaration), _) if !declaration.uri.is_empty() => Ok(Some(declaration.uri.clone())),
        (_, None) => Ok(None),
        (_, Some(prefix)) => Err(prefix),
    }
}

/// Namespaces in XML 1.0 §3 and §4 constraints on one declaration:
/// "The prefix xml is by definition bound to the namespace name
/// `http://www.w3.org/XML/1998/namespace`. It MAY, but need not, be declared,
/// and MUST NOT be bound to any other namespace name. Other prefixes MUST
/// NOT be bound to this namespace name"; "The prefix xmlns ... MUST NOT be
/// declared", nor its namespace name bound; and "In a namespace declaration
/// for a prefix ... the attribute value MUST NOT be empty."
fn check_namespace_declaration(declaration: &NamespaceDeclaration) -> Result<(), XmlError> {
    let reason = match (declaration.prefix.as_deref(), declaration.uri.as_str()) {
        (Some("xml"), XML_NAMESPACE) => None,
        (Some("xml"), _) => Some("the prefix `xml` cannot be bound to another namespace name"),
        (Some("xmlns"), _) => Some("the prefix `xmlns` cannot be declared"),
        (_, XML_NAMESPACE) => Some("only the prefix `xml` can be bound to the XML namespace"),
        (_, XMLNS_NAMESPACE) => Some("no prefix can be bound to the `xmlns` namespace"),
        (Some(_), "") => Some("a namespace declaration for a prefix cannot be empty"),
        _ => None,
    };
    match reason {
        None => Ok(()),
        Some(reason) => Err(XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: format!("{reason} (Namespaces in XML 1.0 §3)"),
            span: declaration.span,
        }),
    }
}

fn unbound_prefix(prefix: &str, at: Span) -> XmlError {
    XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::Unclassified,
        message: format!("unbound XML namespace prefix `{prefix}`"),
        span: Span::new(at.source, at.start, at.start.saturating_add(prefix.len())),
    }
}

/// The replacement text quick-xml normalizes for `&name;` in an attribute
/// value: a declared internal entity's (XML 1.0 §4.4.5), else the predefined
/// entity's, with `<` and `&` written as character references so the
/// normalizer includes them as data.
fn attribute_entity<'entity>(entities: &'entity Entities, name: &str) -> Option<&'entity str> {
    entities.replacement(name).or(match name {
        "lt" => Some("&#60;"),
        "gt" => Some(">"),
        "amp" => Some("&#38;"),
        "apos" => Some("'"),
        "quot" => Some("\""),
        _ => None,
    })
}

/// XML 1.0 §3.1 WFC: Unique Att Spec. The repeated attribute is primary; the
/// removal range also covers its preceding whitespace so deleting it leaves a
/// well-formed tag.
fn duplicate_attribute(text: &str, lexical: &str, first: Span, repeated: Span) -> XmlError {
    let leading = text.get(..repeated.start).map_or(0, |before| {
        before.len() - before.trim_end_matches([' ', '\t', '\r', '\n']).len()
    });

    XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::DuplicateAttribute {
            first,
            removal: Span::new(
                repeated.source,
                repeated.start.saturating_sub(leading),
                repeated.end,
            ),
        },
        message: format!("duplicate attribute `{lexical}`"),
        span: repeated,
    }
}

/// XML 1.0 §3.1 WFC: No < in Attribute Values. Locates every `<` in the raw
/// value so one repair can escape them all.
fn reject_less_than_in_value(raw: &[u8], lexical: &str, value_span: Span) -> Result<(), XmlError> {
    let occurrences: Vec<Span> = raw
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'<')
        .map(|(index, _)| {
            let start = value_span.start.saturating_add(index);
            Span::new(value_span.source, start, start.saturating_add(1))
        })
        .collect();
    match occurrences.first().copied() {
        None => Ok(()),
        Some(first) => Err(XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::LessThanInAttributeValue { occurrences },
            message: format!("unescaped `<` in the value of attribute `{lexical}`"),
            span: first,
        }),
    }
}

fn decode(bytes: &[u8], source_id: SourceId, offset: usize) -> Result<String, XmlError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|error| XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: error.to_string(),
            span: Span::new(source_id, offset, offset.saturating_add(bytes.len())),
        })
}

fn locate_attribute(raw: &[u8], key: &[u8], from: usize) -> Option<usize> {
    raw.get(from..).and_then(|tail| {
        tail.windows(key.len())
            .position(|window| window == key)
            .map(|relative| from + relative)
    })
}

fn locate_value(raw: &[u8], key_start: usize, key_len: usize) -> usize {
    let after_key = key_start.saturating_add(key_len).min(raw.len());
    let equals = raw[after_key..]
        .iter()
        .position(|byte| *byte == b'=')
        .map_or(after_key, |relative| after_key + relative + 1);

    raw[equals..]
        .iter()
        .position(|byte| *byte == b'\'' || *byte == b'"')
        .map_or(equals, |relative| equals + relative + 1)
}

fn attach_text(value: String, span: Span, stack: &mut [Element]) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(Node::Text(Text { value, span }));
    }
}

/// Appends decoded character data to the open element.
///
/// When `continues_run` is true and the open element's last child is a text
/// node ending exactly at `span.start`, the value is merged into that node and
/// its span is extended, so text interleaved with references stays one node
/// whose span covers the exact raw source range. Otherwise a new node is
/// created and counted against `max_nodes`.
#[allow(
    clippy::too_many_arguments,
    reason = "parser loop state is passed explicitly rather than hidden in a mutable context"
)]
fn append_text(
    value: &str,
    span: Span,
    continues_run: bool,
    stack: &mut [Element],
    node_count: &mut usize,
    max_nodes: usize,
    source_len: usize,
) -> Result<(), XmlError> {
    if continues_run
        && let Some(Node::Text(previous)) = stack
            .last_mut()
            .and_then(|parent| parent.children.last_mut())
        && previous.span.end == span.start
    {
        previous.value.push_str(value);
        previous.span.end = span.end;
        return Ok(());
    }
    increment_count(
        node_count,
        max_nodes,
        span.source,
        span.start,
        source_len,
        "tree nodes",
    )?;
    attach_text(value.to_owned(), span, stack);
    Ok(())
}

fn error_category(error: &XmlError) -> ErrorCategory {
    match error.kind {
        XmlErrorKind::ResourceLimit | XmlErrorKind::InvalidLimits => ErrorCategory::Budget,
        XmlErrorKind::Malformed
        | XmlErrorKind::UndefinedEntityReference
        | XmlErrorKind::UnsupportedDtdConstruct
        | XmlErrorKind::InvalidCharacterReference => ErrorCategory::Parse,
    }
}

/// Maximum significant digits accepted in a character reference. Seven hex or
/// decimal digits already exceed U+10FFFF; leading zeros are skipped first.
const MAX_CHAR_REF_DIGITS: usize = 7;

/// Resolves one general reference (the bytes between `&` and `;`).
///
/// Accepts only the five predefined XML entities and character references
/// that match `'&#' [0-9]+ ';'` or `'&#x' [0-9a-fA-F]+ ';'` and name an XML 1.0
/// `Char`. Every other reference fails closed with `span` as its location.
/// Work is bounded by the reference length.
fn resolve_reference(content: &[u8], span: Span) -> Result<char, XmlError> {
    if let Some(number) = content.strip_prefix(b"#") {
        let (digits, radix) = match number.strip_prefix(b"x") {
            Some(hex) => (hex, 16u32),
            None => (number, 10u32),
        };
        let invalid = |reason: &str| XmlError {
            kind: XmlErrorKind::InvalidCharacterReference,
            detail: XmlErrorDetail::Unclassified,
            message: format!("invalid XML character reference: {reason}"),
            span,
        };
        if digits.is_empty() {
            return Err(invalid("missing digits"));
        }
        let significant = digits
            .iter()
            .position(|byte| *byte != b'0')
            .map_or(&[][..], |first| digits.get(first..).unwrap_or(&[]));
        if significant.len() > MAX_CHAR_REF_DIGITS {
            return Err(invalid("code point is outside the XML Char range"));
        }
        let mut code = 0u32;
        for byte in digits {
            let digit = char::from(*byte)
                .to_digit(radix)
                .ok_or_else(|| invalid("digits must be decimal or lowercase-x hexadecimal"))?;
            code = code
                .checked_mul(radix)
                .and_then(|value| value.checked_add(digit))
                .ok_or_else(|| invalid("code point is outside the XML Char range"))?;
        }
        return char::from_u32(code)
            .filter(|character| is_xml_char(*character))
            .ok_or_else(|| invalid("code point is outside the XML Char range"));
    }
    match content {
        b"lt" => Ok('<'),
        b"gt" => Ok('>'),
        b"amp" => Ok('&'),
        b"apos" => Ok('\''),
        b"quot" => Ok('"'),
        _ => Err(XmlError {
            kind: XmlErrorKind::UndefinedEntityReference,
            detail: XmlErrorDetail::Unclassified,
            message: "undefined XML entity reference; only the predefined entities \
                      `lt`, `gt`, `amp`, `apos`, and `quot` are supported"
                .into(),
            span,
        }),
    }
}

/// XML 1.0 (Fifth Edition) `Char` production.
const fn is_xml_char(character: char) -> bool {
    matches!(
        character,
        '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

/// Validates every reference in a raw (still escaped) attribute value before
/// quick-xml normalizes it, because quick-xml accepts character references to
/// non-`Char` code points. Error spans cover the exact `&...;` source range.
fn validate_attribute_references(
    raw: &[u8],
    value_span: Span,
    entities: &mut Entities,
) -> Result<(), XmlError> {
    let mut offset = 0usize;
    while let Some(relative) = raw
        .get(offset..)
        .and_then(|tail| tail.iter().position(|byte| *byte == b'&'))
    {
        let ampersand = offset.saturating_add(relative);
        let content_start = ampersand.saturating_add(1);
        let Some(length) = raw
            .get(content_start..)
            .and_then(|tail| tail.iter().position(|byte| *byte == b';'))
        else {
            return Err(XmlError {
                kind: XmlErrorKind::Malformed,
                detail: XmlErrorDetail::Unclassified,
                message: "unterminated XML reference in attribute value".into(),
                span: Span::new(
                    value_span.source,
                    value_span.start.saturating_add(ampersand),
                    value_span.end,
                ),
            });
        };
        let content_end = content_start.saturating_add(length);
        let reference_span = Span::new(
            value_span.source,
            value_span.start.saturating_add(ampersand),
            value_span
                .start
                .saturating_add(content_end)
                .saturating_add(1),
        );
        let content = raw.get(content_start..content_end).unwrap_or(&[]);
        let declared = std::str::from_utf8(content)
            .ok()
            .filter(|_| !entities.is_empty())
            .map(|name| entities.expanded_len(name, reference_span))
            .transpose()?
            .flatten();
        match declared {
            Some(length) => entities.charge(length, reference_span)?,
            None => {
                resolve_reference(content, reference_span)?;
            }
        }
        offset = content_end.saturating_add(1);
    }
    Ok(())
}

/// The parser's running counts, shared with included replacement text.
struct Counts<'a> {
    events: &'a mut usize,
    nodes: &'a mut usize,
    attributes: &'a mut usize,
}

/// A general or character reference no declared entity answers: a
/// predefined entity or a character reference, else the failure the
/// internal subset gives it (XML 1.0 §4.1 WFC: Entity Declared, or §5.1 when
/// a parameter entity that is not read may declare it).
fn resolve_general(reference: &[u8], span: Span, entities: &Entities) -> Result<char, XmlError> {
    match std::str::from_utf8(reference) {
        Ok(name)
            if !entities.is_empty()
                && !name.starts_with('#')
                && !matches!(name, "lt" | "gt" | "amp" | "apos" | "quot") =>
        {
            Err(entities.undeclared(name, span))
        }
        _ => resolve_reference(reference, span),
    }
}

/// Includes the replacement text of the general entity `name`, which
/// contains markup, in place of its reference in element content (XML 1.0
/// §4.4.2 "Included": "its replacement text is retrieved and processed, in
/// place of the reference itself, as though it were part of the document
/// at the location the reference was recognized"). The text is parsed on
/// its own as `content` (§4.3.2: "An internal general parsed entity is
/// well-formed if its replacement text matches the production labeled
/// content"), so an element it opens must close in it; every node it adds
/// spans the reference. Entities it refers to are included the same way.
/// Bounded by the parser's event, node, attribute, and depth limits and
/// the entity nesting bound.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one bounded event loop over replacement text shares the parser's state explicitly"
)]
fn include_markup(
    replacement: &str,
    name: &str,
    span: Span,
    context: ParseContext<'_>,
    stack: &mut Vec<Element>,
    entities: &mut Entities,
    counts: &mut Counts<'_>,
    active: &mut Vec<String>,
) -> Result<(), XmlError> {
    let limits = context.limits;
    let source_len = context.source_len();
    let malformed = |message: String| XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::Unclassified,
        message,
        span,
    };
    if active.iter().any(|open| open == name) {
        return Err(malformed(
            "entity reference is recursive (XML 1.0 §4.1 WFC: No Recursion)".into(),
        ));
    }
    if active.len() >= crate::dtd::MAX_ENTITY_DEPTH {
        return Err(limit_error_at(
            span.source,
            span.start,
            source_len,
            "entity reference nesting",
            crate::dtd::MAX_ENTITY_DEPTH,
        ));
    }
    active.push(name.to_owned());
    let base = stack.len();
    let mut reader = Reader::from_str(replacement);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = true;
    reader.config_mut().check_comments = true;
    let mut text_run_open = false;
    loop {
        *counts.events = counts.events.saturating_add(1);
        if *counts.events > limits.max_events {
            return Err(limit_error(
                span.source,
                source_len,
                "parser events",
                limits.max_events,
            ));
        }
        let event = reader.read_event().map_err(|error| {
            malformed(format!(
                "the replacement text of entity `{name}` is not well-formed content (XML 1.0 §4.3.2): {}",
                describe_reader_error(&error)
            ))
        })?;
        let text_run = if text_run_open {
            TextRun::Continues
        } else {
            TextRun::Starts
        };
        text_run_open = matches!(event, Event::Text(_) | Event::GeneralRef(_));
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                if stack.len() >= limits.max_depth {
                    return Err(limit_error_at(
                        span.source,
                        span.start,
                        source_len,
                        "element depth",
                        limits.max_depth,
                    ));
                }
                increment_count(
                    counts.nodes,
                    limits.max_nodes,
                    span.source,
                    span.start,
                    source_len,
                    "tree nodes",
                )?;
                let element = parse_start(
                    reader.decoder(),
                    context.text,
                    start,
                    span,
                    empty,
                    TagOrigin::Included,
                    entities,
                    stack,
                )?;
                add_attributes(
                    counts.attributes,
                    element
                        .attributes
                        .len()
                        .saturating_add(element.namespaces.len()),
                    limits.max_attributes,
                    span.source,
                    span.start,
                    source_len,
                )?;
                if empty {
                    attach_included(element, stack, span)?;
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                if stack.len() <= base {
                    return Err(malformed(format!(
                        "the replacement text of entity `{name}` closes an element it did not open (XML 1.0 §4.3.2)"
                    )));
                }
                let Some(element) = stack.pop() else {
                    return Err(malformed("end tag has no matching start tag".into()));
                };
                attach_included(element, stack, span)?;
            }
            Event::Text(characters) => {
                let decoded = characters.decode().map_err(|error| {
                    malformed(format!("character data is not valid text: {error}"))
                })?;
                let value = unescape(&decoded).map_err(|error| malformed(error.to_string()))?;
                append_included_text(&value, span, text_run, stack, counts, limits, source_len)?;
            }
            Event::CData(cdata) => {
                let value = cdata.decode().map_err(|error| {
                    malformed(format!("CDATA section is not valid text: {error}"))
                })?;
                append_included_text(
                    &value,
                    span,
                    TextRun::Starts,
                    stack,
                    counts,
                    limits,
                    source_len,
                )?;
                text_run_open = false;
            }
            Event::GeneralRef(reference) => {
                let nested = std::str::from_utf8(&reference).ok();
                let declared = nested
                    .filter(|_| !entities.is_empty())
                    .map(|nested| entities.content(nested, span))
                    .transpose()?
                    .flatten();
                match declared {
                    Some(Content::Markup(text)) => {
                        include_markup(
                            &text,
                            nested.unwrap_or_default(),
                            span,
                            context,
                            stack,
                            entities,
                            counts,
                            active,
                        )?;
                        text_run_open = false;
                    }
                    Some(Content::Text(text)) => append_included_text(
                        &text, span, text_run, stack, counts, limits, source_len,
                    )?,
                    None => {
                        let mut buffer = [0u8; 4];
                        let value = resolve_general(&reference, span, entities)?;
                        append_included_text(
                            value.encode_utf8(&mut buffer),
                            span,
                            text_run,
                            stack,
                            counts,
                            limits,
                            source_len,
                        )?;
                    }
                }
            }
            Event::PI(_) | Event::Comment(_) => {}
            Event::Decl(_) | Event::DocType(_) => {
                return Err(malformed(format!(
                    "the replacement text of entity `{name}` holds a declaration, which is not content (XML 1.0 §4.3.2)"
                )));
            }
            Event::Eof => break,
        }
    }
    if stack.len() != base {
        return Err(malformed(format!(
            "the replacement text of entity `{name}` leaves an element open (XML 1.0 §4.3.2)"
        )));
    }
    active.pop();
    Ok(())
}

/// Adds an element closed in included replacement text to its parent.
fn attach_included(element: Element, stack: &mut [Element], span: Span) -> Result<(), XmlError> {
    let parent = stack.last_mut().ok_or_else(|| XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::Unclassified,
        message: "entity replacement text is included outside the root element".into(),
        span,
    })?;
    parent.children.push(Node::Element(element));
    Ok(())
}

/// Whether character data continues the text run of the previous event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextRun {
    /// The previous event was character data or a reference.
    Continues,
    /// Anything else came before it.
    Starts,
}

/// Appends included character data to the open element: merged into the
/// previous text node of the same inclusion when the run continues.
fn append_included_text(
    value: &str,
    span: Span,
    text_run: TextRun,
    stack: &mut [Element],
    counts: &mut Counts<'_>,
    limits: XmlLimits,
    source_len: usize,
) -> Result<(), XmlError> {
    if value.is_empty() {
        return Ok(());
    }
    if text_run == TextRun::Continues
        && let Some(Node::Text(previous)) = stack
            .last_mut()
            .and_then(|parent| parent.children.last_mut())
        && previous.span == span
    {
        previous.value.push_str(value);
        return Ok(());
    }
    increment_count(
        counts.nodes,
        limits.max_nodes,
        span.source,
        span.start,
        source_len,
        "tree nodes",
    )?;
    attach_text(value.to_owned(), span, stack);
    Ok(())
}

fn attach_element(
    element: Element,
    stack: &mut [Element],
    root: &mut Option<Element>,
    source_id: SourceId,
    offset: usize,
) -> Result<(), XmlError> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(Node::Element(element));
        Ok(())
    } else if root.replace(element).is_none() {
        Ok(())
    } else {
        Err(XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: "XML document has more than one root element".into(),
            span: Span::new(source_id, offset, offset.saturating_add(1)),
        })
    }
}

/// Current parser offset in original source bytes (the skipped BOM included).
fn buffer_position(reader: &NsReader<&[u8]>, context: ParseContext<'_>) -> Result<usize, XmlError> {
    usize::try_from(reader.buffer_position())
        .ok()
        .and_then(|position| position.checked_add(context.base))
        .ok_or_else(|| XmlError {
            kind: XmlErrorKind::Malformed,
            detail: XmlErrorDetail::Unclassified,
            message: "XML source exceeds addressable parser span size".into(),
            span: Span::new(
                context.source_id,
                context.source_len(),
                context.source_len(),
            ),
        })
}

fn error_span(source_id: SourceId, start: usize, end: usize, source_len: usize) -> Span {
    let start = start.min(source_len);
    let mut end = end.min(source_len);
    if end <= start {
        end = start.saturating_add(1).min(source_len);
    }

    Span::new(source_id, start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_telemetry::{ErrorCategory, Recorder, install_recorder};
    use std::sync::{Mutex, MutexGuard};

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);
    /// The source id of the documents these tests parse.
    const DEEP_SOURCE: SourceId = SourceId::new(2);
    /// The source id of the documents these tests parse.
    const DEPTH_LIMIT_SOURCE: SourceId = SourceId::new(3);
    /// The source id of the documents these tests parse.
    const MANUAL_SOURCE: SourceId = SourceId::new(4);
    /// The source id of the documents these tests parse.
    const BUDGET_SOURCE: SourceId = SourceId::new(5);
    /// The source id of the documents these tests parse.
    const ERROR_SOURCE: SourceId = SourceId::new(7);
    /// The source id of the documents these tests parse.
    const LIMITS_SOURCE: SourceId = SourceId::new(9);
    /// The source id of the documents these tests parse.
    const REFERENCE_SOURCE: SourceId = SourceId::new(11);

    static RECORDER_LOCK: Mutex<()> = Mutex::new(());

    fn recorder_lock() -> MutexGuard<'static, ()> {
        RECORDER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn preserves_exact_namespace_uris_independent_of_prefixes() {
        let document = parse_xml(SourceText::new(
            SOURCE,
            "test.scxml",
            r#"<s:scxml xmlns:s="http://www.w3.org/2005/07/scxml" xmlns:x="urn:test:custom"><x:item x:value="ok"/></s:scxml>"#,
        ))
        .expect("namespace-preservation XML fixture must parse");
        assert_eq!(
            document.root.name.namespace_uri.as_deref(),
            Some("http://www.w3.org/2005/07/scxml")
        );
        let Node::Element(item) = &document.root.children[0] else {
            panic!("expected element");
        };
        assert_eq!(item.name.namespace_uri.as_deref(), Some("urn:test:custom"));
        assert_eq!(
            item.attributes[0].name.namespace_uri.as_deref(),
            Some("urn:test:custom")
        );
    }

    #[test]
    fn ingestion_splits_a_repository_package_pin_and_nothing_else() {
        let text = concat!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" "#,
            r#"xmlns:x="https://github.com/o/r/namespaces/x@v1.0.0" "#,
            r#"xmlns:y="https://example.org/a@b/ns" xmlns:z="urn:example:a@b" "#,
            r#"xmlns:w="https://github.com/o/r/namespaces/w@v1/extra">"#,
            r#"<state id="s"><invoke type="https://github.com/o/r/services/s@v2"/>"#,
            r#"<invoke type="https://github.com/o/r/services/t"/>"#,
            r#"<x:item type="https://github.com/o/r/services/s@v2" x:value="ok"/>"#,
            r#"</state></scxml>"#,
        );
        let document = parse_xml(SourceText::new(SOURCE, "pins.scx", text))
            .expect("the pin fixture must parse");
        let declarations: Vec<(&str, Option<&str>)> = document
            .root
            .namespaces
            .iter()
            .map(|declaration| (declaration.uri.as_str(), declaration.tag.as_deref()))
            .collect();
        assert_eq!(
            declarations,
            [
                ("http://www.w3.org/2005/07/scxml", None),
                ("https://github.com/o/r/namespaces/x", Some("v1.0.0")),
                ("https://example.org/a@b/ns", None),
                ("urn:example:a@b", None),
                // A malformed pin is kept as written for the resolver.
                ("https://github.com/o/r/namespaces/w@v1/extra", None),
            ]
        );
        // The value span still covers what the author wrote.
        let pinned = &document.root.namespaces[1];
        assert_eq!(
            document.source.slice(pinned.value_span),
            Some("https://github.com/o/r/namespaces/x@v1.0.0")
        );
        let Node::Element(state) = &document.root.children[0] else {
            panic!("expected the state");
        };
        let elements: Vec<&Element> = state
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(element) => Some(element),
                Node::Text(_) => None,
            })
            .collect();
        let invoke_type = elements[0].attribute("type").expect("invoke type");
        assert_eq!(invoke_type.value, "https://github.com/o/r/services/s");
        assert_eq!(invoke_type.tag.as_deref(), Some("v2"));
        assert_eq!(
            document.source.slice(invoke_type.value_span),
            Some("https://github.com/o/r/services/s@v2")
        );
        let tagless = elements[1].attribute("type").expect("invoke type");
        assert_eq!(tagless.value, "https://github.com/o/r/services/t");
        assert_eq!(tagless.tag, None);
        // Only an SCXML `<invoke type>` is read through the split.
        let item = elements[2];
        assert_eq!(
            item.name.namespace_uri.as_deref(),
            Some("https://github.com/o/r/namespaces/x")
        );
        let other = item.attribute("type").expect("type attribute");
        assert_eq!(other.value, "https://github.com/o/r/services/s@v2");
        assert_eq!(other.tag, None);
        assert_eq!(
            item.attributes[1].name.namespace_uri.as_deref(),
            Some("https://github.com/o/r/namespaces/x")
        );
    }

    #[test]
    fn malformed_xml_reports_a_source_position() {
        let source = SourceText::new(ERROR_SOURCE, "bad.xml", "<root>\n<child></root>");
        let error = parse_xml(source).unwrap_err();
        assert_eq!(error.span.source, ERROR_SOURCE);
        assert!(error.span.start > 0);
    }

    /// Natively the default size and count limits are unbounded: a source
    /// past every wasm bound (8 MiB, 250,000 nodes, 1,000,000 events and
    /// attributes, 256 general entity declarations) parses. Only the depth
    /// guard and the entity-expansion guards remain.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_default_limits_bound_only_depth() {
        let entities = (0..257).fold(String::new(), |mut entities, index| {
            entities.push_str("<!ENTITY e");
            entities.push_str(&index.to_string());
            entities.push_str(" \"v\">");
            entities
        });
        let elements = r#"<a b="1"/>"#.repeat(1_000_001);
        let text = "x".repeat(9 * 1024 * 1024);
        let xml = format!("<!DOCTYPE r [{entities}]><r>{elements}{text}&e256;</r>");
        let document = parse_xml(SourceText::new(SOURCE, "big.xml", xml))
            .expect("native default limits admit any size");
        assert_eq!(document.root.children.len(), 1_000_002);
        assert_eq!(XmlLimits::DEFAULT.max_depth, 256);
    }

    #[test]
    fn rejects_depth_node_attribute_event_and_source_limits() {
        let cases = [
            (
                "<a><b/></a>",
                XmlLimits {
                    max_depth: 1,
                    ..XmlLimits::default()
                },
                "element depth limit exceeded",
            ),
            (
                "<a><b/></a>",
                XmlLimits {
                    max_nodes: 1,
                    ..XmlLimits::default()
                },
                "tree nodes limit exceeded",
            ),
            (
                "<a x='1' y='2'/>",
                XmlLimits {
                    max_attributes: 1,
                    ..XmlLimits::default()
                },
                "attributes limit exceeded",
            ),
            (
                "<a/>",
                XmlLimits {
                    max_events: 1,
                    ..XmlLimits::default()
                },
                "parser events limit exceeded",
            ),
            (
                "<a/>",
                XmlLimits {
                    max_source_bytes: 3,
                    ..XmlLimits::default()
                },
                "source bytes limit exceeded",
            ),
        ];
        for (xml, limits, expected) in cases {
            let error =
                parse_xml_with_limits(SourceText::new(LIMITS_SOURCE, "limit.xml", xml), limits)
                    .expect_err("limit must reject input");
            assert!(error.message.contains(expected), "{}", error.message);
        }
    }

    #[test]
    fn parser_records_bounded_success_and_budget_failure_telemetry() {
        let _lock = recorder_lock();
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let source = SourceText::new(SOURCE, "ok.xml", "<root/>");
        parse_xml(source).expect("bounded document parses");
        let source = SourceText::new(DEEP_SOURCE, "deep.xml", "<a><b/></a>");
        parse_xml_with_limits(
            source,
            XmlLimits {
                max_depth: 1,
                ..XmlLimits::default()
            },
        )
        .expect_err("depth budget rejects input");
        let observations = recorder.observations();
        // Exactly one boundary span per parse: helpers, per-event scanning,
        // and error builders run inside it and record nothing of their own.
        assert_eq!(observations.len(), 2, "{observations:?}");
        assert!(observations.iter().all(|item| {
            item.module == Module::Syntax && item.operation == Operation::SyntaxParseXml
        }));
        assert_eq!(observations[0].events, [SignalEvent::InputAccepted]);
        assert_eq!(observations[0].error, None);
        assert_eq!(observations[1].events, [SignalEvent::InputRejected]);
        assert_eq!(observations[1].error, Some(ErrorCategory::Budget));
    }

    fn parse_str(xml: &str) -> Result<Document, XmlError> {
        parse_xml(SourceText::new(REFERENCE_SOURCE, "refs.xml", xml))
    }

    fn only_text(element: &Element) -> &Text {
        match element.children.as_slice() {
            [Node::Text(text)] => text,
            other => panic!("expected exactly one merged text child, got {other:?}"),
        }
    }

    #[test]
    fn element_text_resolves_predefined_and_character_references() {
        let xml = "<s>if (x &lt; 5 &amp;&amp; y &gt; 2) &quot;a&apos; &#x41;&#66;&#x1F600;</s>";
        let document = parse_str(xml).expect("predefined and character references must parse");
        let text = only_text(&document.root);
        assert_eq!(text.value, "if (x < 5 && y > 2) \"a' AB\u{1F600}");
        assert_eq!(
            &xml[text.span.start..text.span.end],
            "if (x &lt; 5 &amp;&amp; y &gt; 2) &quot;a&apos; &#x41;&#66;&#x1F600;",
            "merged text span must cover the exact source range of text and references"
        );
    }

    #[test]
    fn leading_and_standalone_references_form_one_text_node() {
        let document = parse_str("<s>&lt;</s>").expect("lone reference must parse");
        let text = only_text(&document.root);
        assert_eq!(text.value, "<");
        assert_eq!((text.span.start, text.span.end), (3, 7));
        let document =
            parse_str("<s>&lt;a<![CDATA[&lt;]]>&gt;</s>").expect("mixed CDATA must parse");
        let values: Vec<&str> = document
            .root
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.value.as_str()),
                Node::Element(_) => None,
            })
            .collect();
        assert_eq!(values, ["<a", "&lt;", ">"], "CDATA must stay literal");
    }

    #[test]
    fn attribute_values_resolve_references_and_reject_invalid_ones() {
        let document = parse_str(r#"<a v="&lt;&#x41;&amp;&#9;&quot;"/>"#)
            .expect("attribute references must parse");
        assert_eq!(document.root.attributes[0].value, "<A&\t\"");
        for (xml, reference) in [
            (r#"<a v="x&foo;y"/>"#, "&foo;"),
            (r#"<a v="&#1;"/>"#, "&#1;"),
            (r#"<a v="&#xFFFF;"/>"#, "&#xFFFF;"),
            (r#"<a v="&#xFFFE;"/>"#, "&#xFFFE;"),
            (r#"<a b="ok" v="&#xD800;"/>"#, "&#xD800;"),
            (r#"<a v="&nbsp;"/>"#, "&nbsp;"),
        ] {
            let error = parse_str(xml).expect_err("invalid attribute reference must fail closed");
            assert_eq!(
                &xml[error.span.start..error.span.end],
                reference,
                "{xml}: {}",
                error.message
            );
            assert!(error.message.contains("reference"), "{}", error.message);
        }
    }

    #[test]
    fn hostile_text_references_fail_closed_with_exact_spans() {
        for reference in [
            "&foo;",
            "&nbsp;",
            "&#0;",
            "&#1;",
            "&#x8;",
            "&#xB;",
            "&#xD800;",
            "&#xDFFF;",
            "&#xFFFE;",
            "&#xFFFF;",
            "&#x110000;",
            "&#99999999999999999999999;",
            "&#xFFFFFFFFFFFFFFFFFFFF;",
            "&#x;",
            "&#;",
            "&#-65;",
            "&#+65;",
            "&#x-41;",
            "&#X41;",
            "&#6a;",
        ] {
            let xml = format!("<s>ok {reference} tail</s>");
            let error = parse_str(&xml).expect_err("hostile reference must fail closed");
            assert_eq!(
                &xml[error.span.start..error.span.end],
                reference,
                "{reference}: {}",
                error.message
            );
            assert!(error.message.contains("reference"), "{}", error.message);
        }
    }

    #[test]
    fn reference_failures_carry_typed_kinds_and_parse_telemetry() {
        let _lock = recorder_lock();
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let cases = [
            ("<s>&foo;</s>", XmlErrorKind::UndefinedEntityReference),
            ("<s a='&ext;'/>", XmlErrorKind::UndefinedEntityReference),
            ("<s>&#xD800;</s>", XmlErrorKind::InvalidCharacterReference),
            ("<s a='&#0;'/>", XmlErrorKind::InvalidCharacterReference),
            (
                "<!DOCTYPE s [<!ENTITY e \"x\">]><s>&f;</s>",
                XmlErrorKind::UndefinedEntityReference,
            ),
        ];
        for (xml, kind) in cases {
            let error = parse_str(xml).expect_err("reference must fail closed");
            assert_eq!(error.kind, kind, "{xml}");
        }
        assert!(recorder.observations().iter().any(|item| {
            item.module == Module::Syntax
                && item.operation == Operation::SyntaxParseXml
                && item.error == Some(ErrorCategory::Parse)
        }));
        let limit = parse_xml_with_limits(
            SourceText::new(DEPTH_LIMIT_SOURCE, "limit.xml", "<a><b/></a>"),
            XmlLimits {
                max_depth: 1,
                ..XmlLimits::default()
            },
        )
        .expect_err("depth limit");
        assert_eq!(limit.kind, XmlErrorKind::ResourceLimit);
    }

    /// XML 1.0 §5.1: a non-validating processor "MUST use the information
    /// in those declarations to ... include the replacement text of
    /// internal entities"; §4.4.2 and §4.4.5 include it in content and in
    /// attribute values; §4.5 replaces character references in an entity
    /// value when it is declared.
    #[test]
    fn internal_subset_entities_expand_in_content_and_attribute_values() {
        let xml = "<!DOCTYPE s [\n  <!-- a comment --><?pi x?>\n  <!ENTITY who \"world\">\n  <!ENTITY greet 'hello &who;&#33;'>\n  <!ENTITY who \"ignored\">\n  <!ELEMENT s ANY>\n  <!ATTLIST s a CDATA #IMPLIED>\n]><s a=\"&greet; &amp; &who;\">&greet; &lt;&who;&gt;</s>";
        let document = parse_str(xml).expect("internal entities must expand");
        assert_eq!(only_text(&document.root).value, "hello world! <world>");
        assert_eq!(
            document.root.attribute("a").map(|a| a.value.as_str()),
            Some("hello world! & world")
        );
        let text = only_text(&document.root);
        assert_eq!(
            &xml[text.span.start..text.span.end],
            "&greet; &lt;&who;&gt;",
            "the text span covers the references exactly"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one table covers every fail-closed DTD construct"
    )]
    fn unsafe_or_ill_formed_dtd_constructs_fail_closed() {
        let laughs = {
            let mut subset = String::from("<!ENTITY l0 \"lollollollollol\">");
            for level in 1..=9 {
                let previous = format!("&l{};", level - 1);
                std::fmt::Write::write_fmt(
                    &mut subset,
                    format_args!("<!ENTITY l{level} \"{}\">", previous.repeat(10)),
                )
                .expect("writing to a String cannot fail");
            }
            format!("<!DOCTYPE s [{subset}]><s>&l9;</s>")
        };
        for (xml, kind) in [
            // XML external entity: never read.
            (
                "<!DOCTYPE s [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><s>&x;</s>".to_owned(),
                XmlErrorKind::UnsupportedDtdConstruct,
            ),
            // §3.1 WFC: No External Entity References (in an attribute value).
            (
                "<!DOCTYPE s [<!ENTITY x PUBLIC \"-//x\" \"http://example.invalid/x\">]><s a=\"&x;\"/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // Exponential expansion ("billion laughs").
            (laughs, XmlErrorKind::ResourceLimit),
            // Exponential expansion of empty entities in an attribute
            // default: no bytes are produced, every reference is work.
            (
                {
                    let mut subset = String::from("<!ENTITY e0 \"\">");
                    for level in 1..=15 {
                        let previous = format!("&e{};", level - 1);
                        std::fmt::Write::write_fmt(
                            &mut subset,
                            format_args!("<!ENTITY e{level} \"{}\">", previous.repeat(10)),
                        )
                        .expect("writing to a String cannot fail");
                    }
                    format!("<!DOCTYPE s [{subset}<!ATTLIST s a CDATA \"&e15;\">]><s/>")
                },
                XmlErrorKind::ResourceLimit,
            ),
            // XML 1.0 §4.1 WFC: No Recursion.
            (
                "<!DOCTYPE s [<!ENTITY a \"&b;\"><!ENTITY b \"&a;\">]><s>&a;</s>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // Exponential expansion through markup inclusion.
            (
                {
                    let mut subset = String::from("<!ENTITY m0 \"<a/><a/><a/><a/><a/><a/><a/><a/>\">");
                    for level in 1..=9 {
                        let previous = format!("&m{};", level - 1);
                        std::fmt::Write::write_fmt(
                            &mut subset,
                            format_args!("<!ENTITY m{level} \"{}\">", previous.repeat(10)),
                        )
                        .expect("writing to a String cannot fail");
                    }
                    format!("<!DOCTYPE s [{subset}]><s>&m9;</s>")
                },
                XmlErrorKind::ResourceLimit,
            ),
            // Repeated parameter-entity inclusion.
            (
                format!(
                    "<!DOCTYPE s [<!ENTITY % p \"<!ENTITY x 'y'><!ENTITY z 'w'>\">{}]><s/>",
                    "%p;".repeat(3_000)
                ),
                XmlErrorKind::ResourceLimit,
            ),
            // §2.8 WFC: PEs in Internal Subset.
            (
                "<!DOCTYPE s [<!ENTITY % p \"CDATA\"><!ATTLIST s a %p; #IMPLIED>]><s/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // §4.1 WFC: No Recursion, through parameter entities.
            (
                "<!DOCTYPE s [<!ENTITY % a \"&#37;a;\"> %a;]><s/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // §3.4: no conditional section in the internal subset itself.
            (
                "<!DOCTYPE s [<![INCLUDE[<!ENTITY e \"x\">]]>]><s/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // §4.3.2: replacement text must be well-formed content.
            (
                "<!DOCTYPE s [<!ENTITY m \"<b>\">]><s>&m;</s>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            (
                "<!DOCTYPE s [<!ENTITY m \"</b>\">]><s><b>&m;</b></s>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            (
                "<!DOCTYPE s [<!ENTITY m \"<b>&m;</b>\">]><s>&m;</s>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // §3.1 WFC: No < in Attribute Values, directly and by default.
            (
                "<!DOCTYPE s [<!ENTITY m \"<b/>\">]><s a=\"&m;\"/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            (
                "<!DOCTYPE s [<!ENTITY m \"&#60;\"><!ATTLIST s a CDATA \"&m;\">]><s/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // §3.1 WFC: No External Entity References in a default.
            (
                "<!DOCTYPE s [<!ENTITY x SYSTEM \"x.txt\"><!ATTLIST s a CDATA \"&x;\">]><s/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // §4.1 WFC: Entity Declared (before its use in a default).
            (
                "<!DOCTYPE s [<!ATTLIST s a CDATA \"&later;\"><!ENTITY later \"x\">]><s/>".to_owned(),
                XmlErrorKind::UndefinedEntityReference,
            ),
            // A prefix bound by included markup is still checked (Namespaces
            // in XML 1.0 §3).
            (
                "<!DOCTYPE s [<!ENTITY m \"<b xmlns:xml='urn:x'/>\">]><s>&m;</s>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            (
                "<!DOCTYPE s [<!ENTITY m \"<p:b/>\">]><s>&m;</s>".to_owned(),
                XmlErrorKind::Malformed,
            ),
            // §5.1: after a parameter entity that is not read, later entity
            // declarations are not processed, so their entities are not read.
            (
                "<!DOCTYPE s [<!ENTITY % ext SYSTEM \"ext.dtd\"> %ext; <!ENTITY e \"x\">]><s>&e;</s>".to_owned(),
                XmlErrorKind::UnsupportedDtdConstruct,
            ),
            (
                "<!DOCTYPE s [<!ENTITY e \"x\"]><s/>".to_owned(),
                XmlErrorKind::Malformed,
            ),
        ] {
            let error = parse_str(&xml).expect_err("the construct must fail closed");
            assert_eq!(error.kind, kind, "{xml}: {}", error.message);
            assert!(error.span.end <= xml.len() && error.span.start < error.span.end, "{xml}");
        }
        // A declared-but-unused external entity is harmless.
        assert!(parse_str("<!DOCTYPE s [<!ENTITY x SYSTEM \"x.txt\">]><s/>").is_ok());
    }

    fn children(element: &Element) -> Vec<&Node> {
        element.children.iter().collect()
    }

    /// XML 1.0 §4.4.2: a referenced internal entity is "Included": "its
    /// replacement text is retrieved and processed, in place of the
    /// reference itself, as though it were part of the document at the
    /// location the reference was recognized"; §4.5 replaces character
    /// references when the entity is declared, so `&#60;` becomes markup.
    #[test]
    fn markup_in_entity_replacement_text_is_included_as_content() {
        let xml = "<!DOCTYPE s [<!ENTITY inner \"&#60;i/&#62;\"><!ENTITY m \"<p:b x='1 &amp; 2'>t &inner;&lt;</p:b><c/>\">]><s xmlns='urn:d' xmlns:p='urn:p'>a&m;z</s>";
        let document = parse_str(xml).expect("markup replacement text is included");
        let reference = xml.find("&m;").expect("the reference is in the source");
        let span = Span::new(REFERENCE_SOURCE, reference, reference + "&m;".len());
        let nodes = children(&document.root);
        let [
            Node::Text(before),
            Node::Element(bold),
            Node::Element(empty),
            Node::Text(after),
        ] = nodes.as_slice()
        else {
            panic!("expected text, two included elements, and text: {nodes:?}");
        };
        assert_eq!((before.value.as_str(), after.value.as_str()), ("a", "z"));
        assert_eq!(bold.name, ExpandedName::new(Some("urn:p"), "b"));
        assert_eq!(empty.name, ExpandedName::new(Some("urn:d"), "c"));
        assert_eq!(bold.attribute("x").map(|x| x.value.as_str()), Some("1 & 2"));
        assert_eq!(
            (bold.span, bold.start_tag_span, empty.span),
            (span, span, span)
        );
        let [
            Node::Text(leading),
            Node::Element(inner),
            Node::Text(trailing),
        ] = children(bold).as_slice()
        else {
            panic!("expected the nested inclusion: {:?}", bold.children);
        };
        assert_eq!(
            (leading.value.as_str(), trailing.value.as_str()),
            ("t ", "<")
        );
        assert_eq!(inner.name, ExpandedName::new(Some("urn:d"), "i"));
        assert_eq!(
            inner.span, span,
            "nested inclusions span the outer reference"
        );
    }

    /// XML 1.0 §4.4.8: a parameter-entity reference between declarations is
    /// "Included as PE"; §2.8 WFC: PE Between Declarations lets its
    /// replacement text hold conditional sections (§3.4).
    #[test]
    fn parameter_entities_between_declarations_are_included() {
        let xml = "<!DOCTYPE s [\n  <!ENTITY % decls \"<!ENTITY e 'from a parameter entity'>\">\n  <!ENTITY % sections \"<![ INCLUDE [<!ENTITY f 'included'>]]><![IGNORE[<!ENTITY f 'ignored'><![INCLUDE[ nested ]]>]]>\">\n  %decls; %sections;\n  <!ENTITY e \"ignored: the first declaration binds\">\n]><s a=\"&f;\">&e;</s>";
        let document = parse_str(xml).expect("parameter entities are included");
        assert_eq!(only_text(&document.root).value, "from a parameter entity");
        assert_eq!(
            document.root.attribute("a").map(|a| a.value.as_str()),
            Some("included")
        );
    }

    /// XML 1.0 §5.1: a non-validating processor "MUST use the information in
    /// those declarations to normalize attribute values ... and supply
    /// default attribute values"; §3.3: the first definition of an
    /// attribute binds; §3.3.3 normalizes defaults and, for types other than
    /// CDATA, every value.
    #[test]
    fn attribute_list_declarations_supply_defaults_and_normalize() {
        let xml = "<!DOCTYPE s [\n  <!ENTITY w \"wo&#9;rld\">\n  <!ATTLIST s xmlns CDATA #FIXED 'urn:x'\n              a CDATA 'hello  &w;'\n              b NMTOKENS ' p   q '\n              e (on|off) 'on'\n              c CDATA #IMPLIED\n              r CDATA #REQUIRED>\n  <!ATTLIST s a CDATA 'ignored: the first definition binds' t ID #IMPLIED>\n  <!ATTLIST k z CDATA \"50%\">\n]><s b='  u   v ' t='  id1 ' r='x'><k/></s>";
        let document = parse_str(xml).expect("attribute-list declarations apply");
        let root = &document.root;
        let value = |element: &Element, name: &str| {
            element
                .attribute(name)
                .map(|attribute| attribute.value.clone())
        };
        assert_eq!(root.name, ExpandedName::new(Some("urn:x"), "s"));
        assert_eq!(value(root, "a").as_deref(), Some("hello  wo rld"));
        assert_eq!(
            value(root, "b").as_deref(),
            Some("u v"),
            "specified NMTOKENS"
        );
        assert_eq!(value(root, "t").as_deref(), Some("id1"), "specified ID");
        assert_eq!(value(root, "e").as_deref(), Some("on"));
        assert_eq!(value(root, "c"), None, "#IMPLIED supplies nothing");
        let default = root.attribute("a").expect("defaulted");
        assert_eq!(
            &xml[default.span.start..default.span.end],
            "'hello  &w;'",
            "a default spans its literal in the DTD"
        );
        let Node::Element(k) = &root.children[0] else {
            panic!("expected k");
        };
        assert_eq!(k.name, ExpandedName::new(Some("urn:x"), "k"));
        assert_eq!(value(k, "z").as_deref(), Some("50%"));
    }

    /// Empty defaults supply attributes without value bytes, so every
    /// defaulted attribute is charged its name too: definitions times
    /// elements cannot grow without bound on any build.
    #[test]
    fn empty_attribute_defaults_are_charged_per_supplied_attribute() {
        let mut subset = String::from("<!ATTLIST k");
        for index in 0..1_000 {
            std::fmt::Write::write_fmt(&mut subset, format_args!(" attr{index} CDATA ''"))
                .expect("writing to a String cannot fail");
        }
        subset.push('>');
        let xml = format!("<!DOCTYPE s [{subset}]><s>{}</s>", "<k/>".repeat(300));
        let Err(error) = parse_str(&xml) else {
            panic!("supplied defaults are charged");
        };
        assert_eq!(error.kind, XmlErrorKind::ResourceLimit, "{}", error.message);
        // A few elements with the same defaults stay well inside the budget.
        let xml = format!("<!DOCTYPE s [{subset}]><s>{}</s>", "<k/>".repeat(3));
        let document = parse_str(&xml).expect("a small supply is accepted");
        let Node::Element(k) = &document.root.children[0] else {
            panic!("expected k");
        };
        assert_eq!(k.attributes.len(), 1_000);
    }

    /// Parses `xml` and returns the outcome and how long it took: a
    /// document either finishes in time or fails with `ResourceLimit`.
    fn timed_parse(xml: &str) -> (Result<Document, XmlError>, std::time::Duration) {
        let started = std::time::Instant::now();
        let result = parse_str(xml);
        (result, started.elapsed())
    }

    /// Debug-build time any of the per-element attribute workloads below
    /// may take: the quadratic lookups took minutes.
    const ATTRIBUTE_WORK_TIME: std::time::Duration = std::time::Duration::from_secs(60);

    /// `#IMPLIED` definitions supply nothing, so they cost no work per
    /// element: 200,000 of them over 1,000,000 elements are not scanned
    /// per element on any build.
    #[test]
    fn implied_attribute_definitions_cost_no_work_per_element() {
        let mut subset = String::from("<!ATTLIST k");
        for index in 0..200_000 {
            std::fmt::Write::write_fmt(&mut subset, format_args!(" a{index} CDATA #IMPLIED"))
                .expect("writing to a String cannot fail");
        }
        subset.push('>');
        let xml = format!("<!DOCTYPE s [{subset}]><s>{}</s>", "<k/>".repeat(1_000_000));
        let (result, elapsed) = timed_parse(&xml);
        assert!(elapsed < ATTRIBUTE_WORK_TIME, "{elapsed:?}");
        match result {
            Ok(document) => assert_eq!(document.root.children.len(), 1_000_000),
            Err(error) => assert_eq!(error.kind, XmlErrorKind::ResourceLimit, "{}", error.message),
        }
    }

    /// Each specified attribute finds its declared type without scanning
    /// its element's definitions: 200,000 specified attributes of an
    /// element type with 200,000 tokenized definitions.
    #[test]
    fn specified_attributes_find_their_declared_type_without_a_scan() {
        let mut subset = String::from("<!ATTLIST k");
        let mut attributes = String::new();
        for index in 0..200_000 {
            std::fmt::Write::write_fmt(&mut subset, format_args!(" a{index} NMTOKENS #IMPLIED"))
                .expect("writing to a String cannot fail");
            std::fmt::Write::write_fmt(&mut attributes, format_args!(" a{index}=' v  w '"))
                .expect("writing to a String cannot fail");
        }
        subset.push('>');
        let xml = format!("<!DOCTYPE k [{subset}]><k{attributes}/>");
        let (result, elapsed) = timed_parse(&xml);
        assert!(elapsed < ATTRIBUTE_WORK_TIME, "{elapsed:?}");
        match result {
            Ok(document) => {
                assert_eq!(document.root.attributes.len(), 200_000);
                assert!(
                    document
                        .root
                        .attributes
                        .iter()
                        .all(|attribute| attribute.value == "v w"),
                    "every specified value is normalized as its declared type"
                );
            }
            Err(error) => assert_eq!(error.kind, XmlErrorKind::ResourceLimit, "{}", error.message),
        }
    }

    #[test]
    fn merged_reference_runs_do_not_consume_extra_node_budget() {
        let xml = "<s>a&lt;b&gt;c</s>";
        let document = parse_xml_with_limits(
            SourceText::new(BUDGET_SOURCE, "budget.xml", xml),
            XmlLimits {
                max_nodes: 2,
                ..XmlLimits::default()
            },
        )
        .expect("one element and one merged text node fit in two nodes");
        assert_eq!(only_text(&document.root).value, "a<b>c");
    }

    #[test]
    fn valid_boundary_character_references_are_accepted() {
        let document = parse_str(
            "<s>&#9;&#xA;&#13;&#x20;&#xD7FF;&#xE000;&#xFFFD;&#x10000;&#x10FFFF;&#00065;</s>",
        )
        .expect("boundary XML Char references must parse");
        assert_eq!(
            only_text(&document.root).value,
            "\t\n\r \u{D7FF}\u{E000}\u{FFFD}\u{10000}\u{10FFFF}A"
        );
    }

    #[test]
    fn rejects_an_over_budget_manually_constructed_tree() {
        let root = Element {
            lexical_name: "root".into(),
            name: ExpandedName::new(None::<String>, "root"),
            start_tag_span: Span::new(MANUAL_SOURCE, 0, 6),
            span: Span::new(MANUAL_SOURCE, 0, 13),
            namespaces: Vec::new(),
            attributes: Vec::new(),
            children: vec![Node::Element(Element {
                lexical_name: "child".into(),
                name: ExpandedName::new(None::<String>, "child"),
                start_tag_span: Span::new(MANUAL_SOURCE, 6, 14),
                span: Span::new(MANUAL_SOURCE, 6, 14),
                namespaces: Vec::new(),
                attributes: Vec::new(),
                children: Vec::new(),
            })],
        };
        let document = Document {
            source: SourceText::new(MANUAL_SOURCE, "manual.xml", "<root></root>"),
            root,
        };
        let error = validate_document_limits(
            &document,
            XmlLimits {
                max_depth: 1,
                ..XmlLimits::default()
            },
        )
        .expect_err("manual tree must obey traversal limits");
        assert!(error.message.contains("element depth limit exceeded"));
    }
}
