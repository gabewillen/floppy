//! Bounded, iterative XML fragment parser feeding the arena [`Store`].
//!
//! Accepted: elements, attributes, namespace declarations, character data,
//! CDATA sections, comments, processing instructions, the five predefined
//! entities, and character references, plus an XML declaration at the very
//! start. Rejected: document type declarations, other entity references,
//! non-XML characters, undeclared prefixes, duplicate attributes, and any
//! malformed markup. Multiple top-level nodes (including text) are allowed.

use alloc::borrow::{Cow, ToOwned as _};
use alloc::string::String;
use alloc::vec::Vec;

use crate::dom::{Binding, Kind, NIL, NodeId, XML_NAMESPACE, XMLNS_NAMESPACE};
use crate::{
    Error, Limit, QName, Store, XmlError, is_name_char, is_name_start, is_xml_space, limits,
};

struct Open<'a> {
    id: NodeId,
    lexical: &'a str,
    declarations: Vec<Binding>,
}

struct Parser<'a> {
    source: &'a str,
    position: usize,
}

impl Store {
    /// Parses a bounded XML fragment into new detached top-level nodes.
    ///
    /// Whitespace-only text is preserved; line endings are normalized to LF;
    /// attribute values are normalized per XML 1.0 §3.3.3 (CDATA type).
    ///
    /// # Errors
    ///
    /// [`Error::Xml`] for malformed input, [`Error::Limit`] for
    /// [`limits::MAX_XML_BYTES`], depth, attribute, name, or store bounds.
    /// The store is unchanged on failure.
    pub fn parse_fragment(&mut self, xml: &str) -> Result<Vec<NodeId>, Error> {
        if xml.len() > limits::MAX_XML_BYTES {
            return Err(Error::Limit(Limit::XmlBytes));
        }
        let mark = self.mark();
        let result = self.parse_fragment_inner(xml);
        if result.is_err() {
            self.rollback(mark);
        }
        result
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one iterative markup loop keeps every token kind and its stack effect together"
    )]
    fn parse_fragment_inner(&mut self, xml: &str) -> Result<Vec<NodeId>, Error> {
        let mut parser = Parser {
            source: xml,
            position: 0,
        };
        let mut top: Vec<NodeId> = Vec::new();
        let mut stack: Vec<Open> = Vec::new();
        let mut text = String::new();
        if parser.rest().starts_with("<?xml")
            && parser
                .rest()
                .get(5..)
                .and_then(|rest| rest.chars().next())
                .is_some_and(is_xml_space)
        {
            let end = parser.rest().find("?>").ok_or(XmlError::Malformed)?;
            parser.position = end.saturating_add(2);
        }
        while !parser.at_end() {
            let rest = parser.rest();
            if !rest.starts_with('<') {
                parser.char_data(&mut text)?;
                continue;
            }
            if rest.starts_with("<![CDATA[") {
                parser.position = parser.position.saturating_add(9);
                let end = parser.rest().find("]]>").ok_or(XmlError::Malformed)?;
                let content = parser.take(end)?;
                check_chars(content)?;
                push_text(&mut text, &normalize_newlines(content))?;
                parser.position = parser.position.saturating_add(3);
                continue;
            }
            self.flush_text(&mut text, &mut stack, &mut top)?;
            if rest.starts_with("<!--") {
                parser.position = parser.position.saturating_add(4);
                let end = parser.rest().find("--").ok_or(XmlError::Malformed)?;
                let content = parser.take(end)?;
                if !parser.rest().starts_with("-->") {
                    return Err(Error::Xml(XmlError::Malformed));
                }
                parser.position = parser.position.saturating_add(3);
                check_chars(content)?;
                let content = normalize_newlines(content).into_owned();
                let bytes = content.len();
                let id = self.alloc_leaf(Kind::Comment, NIL, content, bytes)?;
                self.attach(id, &stack, &mut top)?;
            } else if rest.starts_with("<!") {
                return Err(Error::Xml(if rest.starts_with("<!DOCTYPE") {
                    XmlError::Doctype
                } else {
                    XmlError::Malformed
                }));
            } else if rest.starts_with("<?") {
                parser.position = parser.position.saturating_add(2);
                let target = parser.name()?;
                if target.contains(':') || target.eq_ignore_ascii_case("xml") {
                    return Err(Error::Xml(XmlError::Malformed));
                }
                let end = parser.rest().find("?>").ok_or(XmlError::Malformed)?;
                let raw = parser.take(end)?;
                parser.position = parser.position.saturating_add(2);
                if !raw.is_empty() && !raw.starts_with(is_xml_space) {
                    return Err(Error::Xml(XmlError::Malformed));
                }
                check_chars(raw)?;
                let data = normalize_newlines(raw.trim_start_matches(is_xml_space)).into_owned();
                let bytes = target.len().saturating_add(data.len());
                let name = self.names.intern(QName {
                    namespace_uri: None,
                    local_name: target.to_owned(),
                    prefix: None,
                })?;
                let id = self.alloc_leaf(Kind::Pi, name, data, bytes)?;
                self.attach(id, &stack, &mut top)?;
            } else if rest.starts_with("</") {
                parser.position = parser.position.saturating_add(2);
                let name = parser.name()?;
                parser.skip_space();
                parser.expect(">")?;
                let open = stack.pop().ok_or(XmlError::Malformed)?;
                if open.lexical != name {
                    return Err(Error::Xml(XmlError::Malformed));
                }
            } else {
                parser.position = parser.position.saturating_add(1);
                let lexical = parser.name()?;
                let mut raw_attributes: Vec<(&str, String)> = Vec::new();
                let self_closing = loop {
                    let had_space = parser.skip_space();
                    if parser.rest().starts_with("/>") {
                        parser.position = parser.position.saturating_add(2);
                        break true;
                    }
                    if parser.rest().starts_with('>') {
                        parser.position = parser.position.saturating_add(1);
                        break false;
                    }
                    if !had_space {
                        return Err(Error::Xml(XmlError::Malformed));
                    }
                    let name = parser.name()?;
                    parser.skip_space();
                    parser.expect("=")?;
                    parser.skip_space();
                    let value = parser.attribute_value()?;
                    if raw_attributes.len() >= limits::MAX_ATTRIBUTES {
                        return Err(Error::Limit(Limit::Attributes));
                    }
                    if raw_attributes.iter().any(|(existing, _)| *existing == name) {
                        return Err(Error::Xml(XmlError::Malformed));
                    }
                    raw_attributes.push((name, value));
                };
                if stack.len().saturating_add(1) >= limits::MAX_TREE_DEPTH {
                    return Err(Error::Limit(Limit::TreeDepth));
                }
                let mut declarations: Vec<Binding> = Vec::new();
                let mut attributes: Vec<(&str, String)> = Vec::new();
                for (name, value) in raw_attributes {
                    if name == "xmlns" {
                        if value == XML_NAMESPACE || value == XMLNS_NAMESPACE {
                            return Err(Error::Xml(XmlError::Malformed));
                        }
                        declarations.push((None, value));
                    } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                        let reserved_mismatch = (prefix == "xml") != (value == XML_NAMESPACE);
                        if prefix == "xmlns"
                            || value.is_empty()
                            || reserved_mismatch
                            || value == XMLNS_NAMESPACE
                        {
                            return Err(Error::Xml(XmlError::Malformed));
                        }
                        declarations.push((Some(prefix.to_owned()), value));
                    } else {
                        attributes.push((name, value));
                    }
                }
                let element_name = resolve(lexical, &declarations, &stack, NameKind::Element)?;
                let bytes = lexical.len();
                let element_name = self.intern_resolved(&element_name)?;
                let own = self.new_declarations(
                    declarations
                        .iter()
                        .filter(|(prefix, _)| prefix.as_deref() != Some("xml"))
                        .cloned()
                        .collect(),
                )?;
                let id = self.alloc(Kind::Element, element_name, own, bytes)?;
                let mut seen: Vec<u32> = Vec::new();
                for (name, value) in attributes {
                    let qname = resolve(name, &declarations, &stack, NameKind::Attribute)?;
                    let qname = self.intern_resolved(&qname)?;
                    let expanded = self
                        .names
                        .entries
                        .get(qname as usize)
                        .ok_or(Error::InvalidNode)?
                        .expanded;
                    if seen.contains(&expanded) {
                        return Err(Error::Xml(XmlError::Malformed));
                    }
                    seen.push(expanded);
                    let bytes = name.len().saturating_add(value.len());
                    let attribute = self.alloc_leaf(Kind::Attribute, qname, value, bytes)?;
                    self.link_attribute(id, attribute)?;
                }
                self.attach(id, &stack, &mut top)?;
                if !self_closing {
                    stack.push(Open {
                        id,
                        lexical,
                        declarations,
                    });
                }
            }
        }
        if !stack.is_empty() {
            return Err(Error::Xml(XmlError::Malformed));
        }
        self.flush_text(&mut text, &mut stack, &mut top)?;
        Ok(top)
    }

    fn flush_text(
        &mut self,
        text: &mut String,
        stack: &mut [Open],
        top: &mut Vec<NodeId>,
    ) -> Result<(), Error> {
        if text.is_empty() {
            return Ok(());
        }
        let content = core::mem::take(text);
        let bytes = content.len();
        let id = self.alloc_leaf(Kind::Text, NIL, content, bytes)?;
        self.attach(id, stack, top)
    }

    fn attach(&mut self, id: NodeId, stack: &[Open], top: &mut Vec<NodeId>) -> Result<(), Error> {
        match stack.last() {
            Some(open) => self.link_child(open.id, NIL, id)?,
            None => top.push(id),
        }
        Ok(())
    }
}

/// A resolved expanded name, borrowing the source and the declarations.
struct Resolved<'s> {
    uri: Option<&'s str>,
    local: &'s str,
    prefix: Option<&'s str>,
}

impl Store {
    fn intern_resolved(&mut self, name: &Resolved<'_>) -> Result<u32, Error> {
        self.names.intern_parts(name.uri, name.local, name.prefix)
    }
}

/// The kind of qualified name being resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NameKind {
    /// An element name: an unprefixed one is in the default namespace.
    Element,
    /// An attribute name: an unprefixed one is in no namespace.
    Attribute,
}

/// Resolves a lexical name against the declarations of the element being
/// opened and the open-element stack.
fn resolve<'s>(
    lexical: &'s str,
    own: &'s [Binding],
    stack: &'s [Open<'_>],
    kind: NameKind,
) -> Result<Resolved<'s>, Error> {
    let (prefix, local) = match lexical.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, lexical),
    };
    if !crate::is_ncname(local) || prefix.is_some_and(|prefix| !crate::is_ncname(prefix)) {
        return Err(Error::Xml(XmlError::Malformed));
    }
    let uri = match prefix {
        Some("xml") => Some(XML_NAMESPACE),
        Some("xmlns") => return Err(Error::Xml(XmlError::Malformed)),
        Some(prefix) => Some(lookup(Some(prefix), own, stack).ok_or(XmlError::UndeclaredPrefix)?),
        None if kind == NameKind::Element => lookup(None, own, stack).filter(|uri| !uri.is_empty()),
        None => None,
    };
    Ok(Resolved { uri, local, prefix })
}

fn lookup<'s>(prefix: Option<&str>, own: &'s [Binding], stack: &'s [Open<'_>]) -> Option<&'s str> {
    let frames =
        core::iter::once(own).chain(stack.iter().rev().map(|open| open.declarations.as_slice()));
    for frame in frames {
        for (bound, uri) in frame.iter().rev() {
            if bound.as_deref() == prefix {
                return Some(uri);
            }
        }
    }
    None
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        self.source.get(self.position..).unwrap_or_default()
    }

    fn at_end(&self) -> bool {
        self.position >= self.source.len()
    }

    fn take(&mut self, length: usize) -> Result<&'a str, Error> {
        let end = self.position.saturating_add(length);
        let slice = self
            .source
            .get(self.position..end)
            .ok_or(XmlError::Malformed)?;
        self.position = end;
        Ok(slice)
    }

    fn skip_space(&mut self) -> bool {
        let rest = self.rest();
        let trimmed = rest.trim_start_matches(is_xml_space);
        let skipped = rest.len().saturating_sub(trimmed.len());
        self.position = self.position.saturating_add(skipped);
        skipped > 0
    }

    fn expect(&mut self, token: &str) -> Result<(), Error> {
        if self.rest().starts_with(token) {
            self.position = self.position.saturating_add(token.len());
            Ok(())
        } else {
            Err(Error::Xml(XmlError::Malformed))
        }
    }

    /// An XML `Name` (colons allowed; namespace validation happens later).
    fn name(&mut self) -> Result<&'a str, Error> {
        let rest = self.rest();
        let mut end = 0;
        for (index, character) in rest.char_indices() {
            let valid = if index == 0 {
                is_name_start(character) || character == ':'
            } else {
                is_name_char(character) || character == ':'
            };
            if !valid {
                break;
            }
            end = index.saturating_add(character.len_utf8());
        }
        if end == 0 {
            return Err(Error::Xml(XmlError::Malformed));
        }
        if end > limits::MAX_NAME_BYTES.saturating_mul(2) {
            return Err(Error::Limit(Limit::NameBytes));
        }
        self.take(end)
    }

    fn attribute_value(&mut self) -> Result<String, Error> {
        let quote = self.rest().chars().next().ok_or(XmlError::Malformed)?;
        if quote != '"' && quote != '\'' {
            return Err(Error::Xml(XmlError::Malformed));
        }
        self.position = self.position.saturating_add(1);
        let end = self.rest().find(quote).ok_or(XmlError::Malformed)?;
        let raw = self.take(end)?;
        self.position = self.position.saturating_add(1);
        if raw.contains('<') {
            return Err(Error::Xml(XmlError::Malformed));
        }
        check_chars(raw)?;
        let normalized = normalize_newlines(raw);
        let normalized = if normalized.contains(['\t', '\n']) {
            Cow::Owned(normalized.replace(['\t', '\n'], " "))
        } else {
            normalized
        };
        expand_references(&normalized).map(Cow::into_owned)
    }

    fn char_data(&mut self, text: &mut String) -> Result<(), Error> {
        let rest = self.rest();
        let end = rest.find('<').unwrap_or(rest.len());
        let raw = self.take(end)?;
        if raw.contains("]]>") {
            return Err(Error::Xml(XmlError::Malformed));
        }
        check_chars(raw)?;
        let normalized = normalize_newlines(raw);
        let expanded = expand_references(&normalized)?;
        push_text(text, &expanded)
    }
}

fn push_text(text: &mut String, more: &str) -> Result<(), Error> {
    if text.len().saturating_add(more.len()) > limits::MAX_STRING_BYTES {
        return Err(Error::Limit(Limit::StringBytes));
    }
    text.push_str(more);
    Ok(())
}

fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

fn is_xml_char(character: char) -> bool {
    matches!(character,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn check_chars(text: &str) -> Result<(), Error> {
    if text.chars().all(is_xml_char) {
        Ok(())
    } else {
        Err(Error::Xml(XmlError::Malformed))
    }
}

fn expand_references(text: &str) -> Result<Cow<'_, str>, Error> {
    if !text.contains('&') {
        return Ok(Cow::Borrowed(text));
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(rest.get(..index).unwrap_or_default());
        let after = rest.get(index.saturating_add(1)..).unwrap_or_default();
        let end = after.find(';').ok_or(XmlError::Malformed)?;
        let reference = after.get(..end).unwrap_or_default();
        let character = match reference {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "apos" => '\'',
            "quot" => '"',
            _ => {
                let code = if let Some(hex) = reference.strip_prefix("#x") {
                    if hex.is_empty()
                        || hex.len() > 8
                        || !hex.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        return Err(Error::Xml(XmlError::Entity));
                    }
                    u32::from_str_radix(hex, 16).map_err(|_| XmlError::Entity)?
                } else if let Some(decimal) = reference.strip_prefix('#') {
                    if decimal.is_empty()
                        || decimal.len() > 10
                        || !decimal.chars().all(|c| c.is_ascii_digit())
                    {
                        return Err(Error::Xml(XmlError::Entity));
                    }
                    decimal.parse::<u32>().map_err(|_| XmlError::Entity)?
                } else {
                    return Err(Error::Xml(XmlError::Entity));
                };
                char::from_u32(code)
                    .filter(|character| is_xml_char(*character))
                    .ok_or(XmlError::Entity)?
            }
        };
        out.push(character);
        rest = after.get(end.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(Cow::Owned(out))
}
