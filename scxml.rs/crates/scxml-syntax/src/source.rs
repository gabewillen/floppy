//! Loss-conscious XML source infrastructure.

use std::fmt;
use std::sync::Arc;

/// Stable identity for one parsed source.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct SourceId(u32);

impl SourceId {
    /// Creates a source identity from an application-assigned integer.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the underlying application-assigned integer.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Half-open byte range in one source.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct Span {
    /// Source containing the range.
    pub source: SourceId,
    /// Inclusive UTF-8 byte offset.
    pub start: usize,
    /// Exclusive UTF-8 byte offset.
    pub end: usize,
}

impl Span {
    /// Creates a span, clamping an inverted end to the start.
    #[must_use]
    pub const fn new(source: SourceId, start: usize, end: usize) -> Self {
        Self {
            source,
            start,
            end: if end < start { start } else { end },
        }
    }

    /// Returns the byte length.
    #[must_use]
    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Returns whether the range is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// One-based source position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineColumn {
    /// One-based line number.
    pub line: usize,
    /// One-based Unicode scalar column.
    pub column: usize,
}

/// Owned source text with a precomputed byte-to-line index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceText {
    id: SourceId,
    name: Arc<str>,
    text: Arc<str>,
    line_starts: Arc<[usize]>,
}

impl SourceText {
    /// Creates a source and indexes LF, CR, and CRLF line endings.
    #[must_use]
    pub fn new(id: SourceId, name: impl Into<Arc<str>>, text: impl Into<Arc<str>>) -> Self {
        let text: Arc<str> = text.into();
        // A leading UTF-8 byte order mark (XML 1.0 §4.3.3) is not part of
        // line 1: columns count from the first character after it, while
        // byte offsets keep indexing the original bytes.
        let bom = if text.starts_with('\u{FEFF}') {
            '\u{FEFF}'.len_utf8()
        } else {
            0
        };
        let mut line_starts = vec![bom];
        let bytes = text.as_bytes();
        let mut index = bom;
        while index < bytes.len() {
            match bytes.get(index).copied() {
                Some(b'\r') if bytes.get(index + 1) == Some(&b'\n') => {
                    index += 2;
                    line_starts.push(index);
                }
                Some(b'\r' | b'\n') => {
                    index += 1;
                    line_starts.push(index);
                }
                Some(_) => index += 1,
                None => break,
            }
        }

        Self {
            id,
            name: name.into(),
            text,
            line_starts: line_starts.into(),
        }
    }

    /// Returns this source identity.
    #[must_use]
    pub const fn id(&self) -> SourceId {
        self.id
    }

    /// Returns the display name or path.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the complete UTF-8 source.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns a valid source slice.
    #[must_use]
    pub fn slice(&self, span: Span) -> Option<&str> {
        (span.source == self.id && span.start <= span.end)
            .then(|| self.text.get(span.start..span.end))
            .flatten()
    }

    /// Maps a byte offset to a one-based line and Unicode scalar column.
    #[must_use]
    pub fn line_column(&self, byte_offset: usize) -> LineColumn {
        let offset = byte_offset.min(self.text.len());
        let line_index = self
            .line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts.get(line_index).copied().unwrap_or(0);
        let column = self
            .text
            .get(line_start..offset)
            .map_or(1, |line| line.chars().count() + 1);

        LineColumn {
            line: line_index + 1,
            column,
        }
    }

    /// Returns the byte range for a one-based source line, excluding its line ending.
    #[must_use]
    pub fn line_span(&self, line: usize) -> Option<Span> {
        let start = *self.line_starts.get(line.checked_sub(1)?)?;
        let next = self
            .line_starts
            .get(line)
            .copied()
            .unwrap_or(self.text.len());
        let line_text = self.text.get(start..next)?;
        let content_len = line_text
            .strip_suffix("\r\n")
            .or_else(|| line_text.strip_suffix('\n'))
            .or_else(|| line_text.strip_suffix('\r'))
            .unwrap_or(line_text)
            .len();

        Some(Span::new(self.id, start, start + content_len))
    }
}

/// Expanded XML name independent of lexical prefixes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ExpandedName {
    /// Exact namespace URI, absent for no namespace.
    pub namespace_uri: Option<String>,
    /// XML local name.
    pub local_name: String,
}

impl ExpandedName {
    /// Creates an expanded XML name.
    #[must_use]
    pub fn new(namespace_uri: Option<impl Into<String>>, local_name: impl Into<String>) -> Self {
        Self {
            namespace_uri: namespace_uri.map(Into::into),
            local_name: local_name.into(),
        }
    }

    /// Returns whether this name has the exact namespace URI and local name.
    #[must_use]
    pub fn is(&self, namespace_uri: Option<&str>, local_name: &str) -> bool {
        self.namespace_uri.as_deref() == namespace_uri && self.local_name == local_name
    }
}

impl fmt::Display for ExpandedName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.namespace_uri {
            Some(uri) => write!(formatter, "{{{uri}}}{}", self.local_name),
            None => formatter.write_str(&self.local_name),
        }
    }
}

/// One namespace declaration preserved on its declaring element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamespaceDeclaration {
    /// Prefix, or `None` for the default namespace declaration.
    pub prefix: Option<String>,
    /// Declared URI, exact except that a repository package URI's trailing
    /// `@<tag>` is split off into [`Self::tag`]
    /// ([`scxml_types::strip_tag`]). The empty string undeclares a
    /// namespace.
    pub uri: String,
    /// The release tag a repository package URI pinned (`@<tag>`), which
    /// only the CLI's resolver reads; `None` for every other declaration.
    pub tag: Option<String>,
    /// Span of the complete declaration.
    pub span: Span,
    /// Span of the URI value as written (the tag included) when
    /// recoverable.
    pub value_span: Span,
}

/// One XML attribute with lexical and expanded names plus value spans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    /// Original lexical `QName`.
    pub lexical_name: String,
    /// Namespace-resolved name.
    pub name: ExpandedName,
    /// Decoded XML value; for the `type` of an SCXML `<invoke>` and for a
    /// `datamodel:src` ([`scxml_types::DATAMODEL_NAMESPACE_URI`]), a
    /// repository package URI's trailing `@<tag>` is split off into
    /// [`Self::tag`] ([`scxml_types::strip_tag`]).
    pub value: String,
    /// The release tag an SCXML `<invoke type>` or `datamodel:src`
    /// repository package URI pinned (`@<tag>`), which only the CLI's
    /// resolver reads; `None` for every other attribute.
    pub tag: Option<String>,
    /// Span of the complete attribute.
    pub span: Span,
    /// Span of the value inside quotes, as written (the tag included).
    pub value_span: Span,
}

/// Character content preserved in document order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Text {
    /// Decoded character content.
    pub value: String,
    /// Source range containing the lexical text or CDATA section.
    pub span: Span,
}

/// Loss-conscious XML node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    /// Child element.
    Element(Element),
    /// Text or CDATA content.
    Text(Text),
}

/// Namespace-resolved XML element preserving unknown and other-namespace content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Element {
    /// Original lexical `QName`.
    pub lexical_name: String,
    /// Namespace-resolved element name.
    pub name: ExpandedName,
    /// Start-tag range.
    pub start_tag_span: Span,
    /// Complete element range.
    pub span: Span,
    /// Namespace declarations in source order.
    pub namespaces: Vec<NamespaceDeclaration>,
    /// Non-namespace attributes in source order.
    pub attributes: Vec<Attribute>,
    /// Child elements and text in source order.
    pub children: Vec<Node>,
}

impl Element {
    /// Returns the first unqualified attribute with the requested local name.
    #[must_use]
    pub fn attribute(&self, local_name: &str) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name.is(None, local_name))
    }

    /// Visits this element and every descendant in document order.
    pub fn visit<'a>(&'a self, visitor: &mut impl FnMut(&'a Element)) {
        let mut pending = vec![self];
        while let Some(element) = pending.pop() {
            visitor(element);
            for child in element.children.iter().rev() {
                if let Node::Element(child) = child {
                    pending.push(child);
                }
            }
        }
    }

    /// Visits this element and every descendant in document order with its
    /// in-scope XML Base (W3C XML Base, Second Edition): each `xml:base`
    /// (the value collapsed of XML white space) resolved against its
    /// parent's base with [`scxml_types::resolve_reference`], so a relative
    /// one stays relative to the document's own base URI, which the host
    /// knows; `None` where no ancestor-or-self has one. An element's own
    /// `xml:base` applies to its own attributes. Iterative; bounded by the
    /// parsed tree.
    pub fn visit_with_base<'a>(&'a self, visitor: &mut impl FnMut(&'a Element, Option<&str>)) {
        let mut pending: Vec<(&Element, Option<std::rc::Rc<str>>)> = vec![(self, None)];
        while let Some((element, inherited)) = pending.pop() {
            let base = match element
                .attributes
                .iter()
                .find(|attribute| attribute.name.is(Some(XML_NAMESPACE), "base"))
            {
                Some(attribute) => Some(std::rc::Rc::from(scxml_types::resolve_reference(
                    inherited.as_deref().unwrap_or_default(),
                    &crate::collapse_whitespace(&attribute.value),
                ))),
                None => inherited,
            };
            visitor(element, base.as_deref());
            for child in element.children.iter().rev() {
                if let Node::Element(child) = child {
                    pending.push((child, base.clone()));
                }
            }
        }
    }
}

/// The XML namespace, which binds `xml:base`.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// Parsed namespace-resolved XML document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Document {
    /// Original source and line mapping.
    pub source: SourceText,
    /// Single document element.
    pub root: Element,
}

impl Document {
    /// Returns all exact namespace URIs used by element and attribute names.
    #[must_use]
    pub fn used_namespaces(&self) -> Vec<String> {
        let mut namespaces = Vec::new();
        self.root.visit(&mut |element| {
            if let Some(uri) = &element.name.namespace_uri {
                namespaces.push(uri.clone());
            }
            namespaces.extend(
                element
                    .attributes
                    .iter()
                    .filter_map(|attribute| attribute.name.namespace_uri.clone()),
            );
        });
        namespaces.sort_unstable();
        namespaces.dedup();
        namespaces
    }
}
