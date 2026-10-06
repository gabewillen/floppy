//! The internal DTD subset, processed as XML 1.0 (Fifth Edition) requires of
//! a non-validating processor, within fixed bounds.
//!
//! XML 1.0 §5.1: non-validating processors "are REQUIRED to check only the
//! document entity, including the entire internal DTD subset, for
//! well-formedness" and "MUST use the information in those declarations to
//! normalize attribute values, include the replacement text of internal
//! entities, and supply default attribute values". This module reads the
//! internal subset's general and parameter entity declarations and its
//! attribute-list declarations:
//!
//! * general entity references in element content are included
//!   (§4.4.2 "Included"): replacement text without markup as character
//!   data, replacement text with markup parsed as `content` in place of the
//!   reference (§4.3.2: "An internal general parsed entity is well-formed if
//!   its replacement text matches the production labeled content") by the
//!   XML parser through [`Entities::content`];
//! * general entity references in attribute values are included in literal
//!   (§4.4.5) and normalized (§3.3.3);
//! * parameter-entity references between declarations are included as PE
//!   (§4.4.8), their replacement text processed as declarations (§2.8 WFC:
//!   PE Between Declarations: it "MUST match the production extSubsetDecl",
//!   so conditional sections may occur in it, §3.4);
//! * attribute-list declarations supply default values and declared types
//!   (§3.3: "When more than one `AttlistDecl` is provided for a given element
//!   type, the contents of all those provided are merged. When more than one
//!   definition is provided for the same attribute of a given element type,
//!   the first declaration is binding and later declarations are
//!   ignored."); defaults are normalized as §3.3.3 prescribes, and every
//!   value of an attribute declared with a type other than CDATA is further
//!   normalized ("discarding any leading and trailing space (#x20)
//!   characters, and by replacing sequences of space (#x20) characters by a
//!   single space (#x20) character").
//!
//! Character references in an entity value are replaced when it is declared
//! and general entity references on use (§4.5).
//!
//! Bounds and implementation-defined choices (the XML 1.0 text leaves each
//! open to the processor or makes it a resource question):
//!
//! * External entities, general or parameter, are never read: §4.4.3 lets a
//!   non-validating processor not include them ("the processor MAY, but
//!   need not, include the entity's replacement text"), and reading them
//!   would let a document fetch local files or network resources (XML
//!   external entity attacks). A reference to an external general entity
//!   fails closed with [`crate::XmlErrorKind::UnsupportedDtdConstruct`]
//!   ("If a non-validating processor does not include the replacement text,
//!   it MUST inform the application that it recognized, but did not read,
//!   the entity."). After a reference to a parameter entity that is not
//!   read, later entity and attribute-list declarations are not processed
//!   (§5.1: such a processor "MUST NOT process entity declarations or
//!   attribute-list declarations encountered after a reference to a
//!   parameter entity that is not read, since the entity may have contained
//!   overriding declarations"), and a reference to a general entity they
//!   might have declared fails closed the same way.
//! * Expansion is bounded per reference and per document, the entity
//!   references expanded into attribute defaults are counted per document,
//!   and nesting is bounded, so exponential entity expansion ("billion
//!   laughs"), even of entities that expand to nothing, fails closed as a
//!   resource limit.
//!
//! No spans: this module runs inside the parser's boundary span, and every
//! failure carries the exact source range the caller supplies.

use std::collections::{BTreeMap, BTreeSet};

use crate::source::Span;
use crate::xml::{XmlError, XmlErrorDetail, XmlErrorKind};

// The declaration counts are resource bounds of wasm builds only. The
// markup-declaration, expansion, and entity-depth limits apply to every
// build: entity references expand exponentially ("billion laughs"), so they
// guard against amplification rather than bound a resource.

/// Maximum general entity declarations kept from one internal subset
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_ENTITY_DECLARATIONS: usize = scxml_types::wasm_bound(256);
/// Maximum parameter entity declarations kept from one internal subset
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_PARAMETER_ENTITY_DECLARATIONS: usize = scxml_types::wasm_bound(256);
/// Maximum attribute definitions kept from one internal subset
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_ATTRIBUTE_DEFINITIONS: usize = scxml_types::wasm_bound(1024);
/// Maximum markup declarations processed for one document, counting every
/// declaration a parameter-entity inclusion repeats (every build).
pub const MAX_MARKUP_DECLARATIONS: usize = 4096;
/// Maximum bytes of one entity's replacement text, fully expanded (every
/// build).
pub const MAX_ENTITY_EXPANSION_BYTES: usize = 64 * 1024;
/// Maximum bytes all entity references of one document expand to (every
/// build).
pub const MAX_DOCUMENT_EXPANSION_BYTES: usize = 1024 * 1024;
/// Maximum nesting of entity references inside replacement text (every
/// build).
pub const MAX_ENTITY_DEPTH: usize = 16;
/// Maximum entity references one document's attribute-default
/// normalization expands, counting a nested reference each time it is
/// expanded (every build): references to empty entities produce no bytes,
/// so the expansion byte bounds alone do not limit this work.
pub const MAX_DOCUMENT_ENTITY_REFERENCES: usize = 1024 * 1024;

/// One entity declared in the internal subset.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Entity {
    /// An internal entity: its replacement text (character references
    /// already replaced, general entity references kept, XML 1.0 §4.5).
    Internal(String),
    /// An external (parsed or unparsed) entity, never read.
    External,
}

/// One attribute default of an attribute-list declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
struct DefaultedAttribute {
    /// Attribute name as declared (a lexical `QName`).
    name: String,
    /// The normalized default value and the source range of its literal.
    value: String,
    span: Span,
}

/// How an attribute's value is normalized, by its declared type (XML 1.0
/// §3.3.3): a CDATA value keeps its spaces; a value of any other type is
/// further normalized (leading and trailing spaces dropped, runs collapsed).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueNormalization {
    Cdata,
    Tokenized,
}

/// The attribute definitions of one element type, indexed for the two
/// lookups every tag makes: the attributes whose declared type is not CDATA
/// (§3.3.3 further normalizes their values), found by name, and the
/// definitions that supply a default (§5.1), in declaration order. A
/// definition without a default (`#REQUIRED`, `#IMPLIED`) costs a tag no
/// work.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ElementAttributes {
    tokenized: BTreeSet<String>,
    defaulted: Vec<DefaultedAttribute>,
}

/// What a general entity reference in element content includes (XML 1.0
/// §4.4.2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Content {
    /// Replacement text without markup, fully expanded: character data.
    Text(String),
    /// Replacement text containing markup, unexpanded: the parser parses it
    /// as `content` in place of the reference (§4.3.2).
    Markup(String),
}

/// One default attribute value the internal subset supplies (XML 1.0 §5.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttributeDefault<'a> {
    /// Attribute name as declared (a lexical `QName`, possibly `xmlns`).
    pub name: &'a str,
    /// Normalized default value (§3.3.3).
    pub value: &'a str,
    /// Source range of the default value's literal in the DTD (the
    /// parameter-entity reference that included it, when one did).
    pub span: Span,
}

/// Where one run of declarations ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Terminator {
    /// The internal subset: at its closing `]`.
    Subset,
    /// The replacement text of a parameter entity: at its end.
    End,
    /// An `INCLUDE` conditional section: at its `]]>`.
    Section,
}

/// The declarations one document's internal subset makes.
#[derive(Clone, Debug, Default)]
pub struct Entities {
    general: BTreeMap<String, Entity>,
    parameters: BTreeMap<String, Entity>,
    /// Attribute definitions by element type name.
    attributes: BTreeMap<String, ElementAttributes>,
    /// Element type and attribute name of every kept definition, so a
    /// repeated definition is found without scanning its element's list.
    attribute_names: BTreeSet<(String, String)>,
    attribute_definitions: usize,
    /// Fully expanded text of each markup-free internal entity.
    expanded: BTreeMap<String, String>,
    /// Whether each internal entity's inclusion holds markup, computed once.
    markup: BTreeMap<String, bool>,
    /// Bytes expanded so far in this document.
    used: usize,
    /// Entity references expanded so far by attribute-default normalization.
    references: usize,
    /// Markup declarations processed so far.
    declarations: usize,
    /// Set by a reference to a parameter entity that is not read (§5.1).
    unread_parameter: bool,
}

impl Entities {
    /// Reads the internal subset of one `<!DOCTYPE ...>` whose content (the
    /// bytes between `<!DOCTYPE` and the closing `>`) starts at byte
    /// `offset` of the source.
    ///
    /// # Errors
    ///
    /// Malformed declarations ([`XmlErrorKind::Malformed`]), a bound
    /// exceeded ([`XmlErrorKind::ResourceLimit`]), each at its exact source
    /// range.
    pub fn from_doctype(content: &str, offset: usize, span: Span) -> Result<Self, XmlError> {
        let mut entities = Self::default();
        let Some(open) = subset_open(content) else {
            return Ok(entities);
        };
        let subset = content.get(open.saturating_add(1)..).unwrap_or_default();
        let base = offset.saturating_add(open).saturating_add(1);
        let at = |start: usize, end: usize| {
            Span::new(
                span.source,
                base.saturating_add(start),
                base.saturating_add(end),
            )
        };
        entities.declarations_in(subset, &at, Terminator::Subset, &mut Vec::new())?;
        Ok(entities)
    }

    /// Processes the declarations of `text` up to `terminator`; returns the
    /// byte index just past it (the end of `text` for [`Terminator::End`]).
    #[allow(
        clippy::too_many_lines,
        reason = "one bounded loop owns every markup declaration kind of the DTD grammar"
    )]
    fn declarations_in(
        &mut self,
        text: &str,
        at: &dyn Fn(usize, usize) -> Span,
        terminator: Terminator,
        active: &mut Vec<String>,
    ) -> Result<usize, XmlError> {
        let bytes = text.as_bytes();
        let mut position = 0usize;
        loop {
            let rest = text.get(position..).unwrap_or_default();
            let Some(&byte) = bytes.get(position) else {
                return match terminator {
                    Terminator::End => Ok(position),
                    Terminator::Subset => Err(malformed(
                        "the internal DTD subset is not closed with `]`",
                        at(0, text.len()),
                    )),
                    Terminator::Section => Err(malformed(
                        "conditional section is not closed with `]]>`",
                        at(0, text.len()),
                    )),
                };
            };
            if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                position = position.saturating_add(1);
                continue;
            }
            match terminator {
                Terminator::Subset if byte == b']' => return Ok(position.saturating_add(1)),
                Terminator::Section if rest.starts_with("]]>") => {
                    return Ok(position.saturating_add(3));
                }
                _ => {}
            }
            if rest.starts_with("<!--") {
                let end = rest.find("-->").ok_or_else(|| {
                    malformed("comment in the DTD is not closed", at(position, text.len()))
                })?;
                if rest.get(4..end).is_some_and(|body| body.contains("--")) {
                    return Err(malformed(
                        "a comment must not contain `--` (XML 1.0 §2.5)",
                        at(position, position.saturating_add(end).saturating_add(3)),
                    ));
                }
                position = position.saturating_add(end).saturating_add(3);
                continue;
            }
            if rest.starts_with("<?") {
                let end = rest.find("?>").ok_or_else(|| {
                    malformed(
                        "processing instruction in the DTD is not closed",
                        at(position, text.len()),
                    )
                })?;
                position = position.saturating_add(end).saturating_add(2);
                continue;
            }
            if rest.starts_with("<![") {
                // §3.4: conditional sections occur only in the external
                // subset, and so in the replacement text of a parameter
                // entity included between declarations (§2.8).
                if terminator == Terminator::Subset {
                    return Err(malformed(
                        "conditional sections cannot occur in the internal DTD subset (XML 1.0 §3.4)",
                        at(position, position.saturating_add(3)),
                    ));
                }
                let consumed = self.conditional_section(
                    rest,
                    &|start, end| at(position.saturating_add(start), position.saturating_add(end)),
                    active,
                )?;
                position = position.saturating_add(consumed);
                continue;
            }
            if byte == b'%' {
                let end = rest.find(';').ok_or_else(|| {
                    malformed(
                        "parameter-entity reference is not closed with `;`",
                        at(position, text.len()),
                    )
                })?;
                let name = rest.get(1..end).unwrap_or_default();
                let reference = at(position, position.saturating_add(end).saturating_add(1));
                if !is_name(name) {
                    return Err(malformed(
                        "parameter-entity reference does not name an XML Name",
                        reference,
                    ));
                }
                self.include_parameter(name, reference, active)?;
                position = position.saturating_add(end).saturating_add(1);
                continue;
            }
            let kind = ["<!ENTITY", "<!ATTLIST", "<!ELEMENT", "<!NOTATION"]
                .into_iter()
                .find(|keyword| rest.starts_with(keyword));
            let Some(kind) = kind else {
                return Err(malformed(
                    "unexpected content in the DTD",
                    at(position, position.saturating_add(1)),
                ));
            };
            let end = declaration_end(rest).ok_or_else(|| {
                malformed(
                    "markup declaration is not closed with `>`",
                    at(position, text.len()),
                )
            })?;
            let declaration = rest.get(..end).unwrap_or_default();
            let declaration_at = |start: usize, end: usize| {
                at(position.saturating_add(start), position.saturating_add(end))
            };
            self.count_declaration(declaration_at(0, end.saturating_add(1)))?;
            if kind != "<!ENTITY" && unquoted_percent(declaration) {
                return Err(pe_in_markup(declaration_at(0, end.saturating_add(1))));
            }
            match kind {
                "<!ENTITY" => self.declare(declaration, &declaration_at)?,
                "<!ATTLIST" => self.attribute_list(declaration, &declaration_at)?,
                _ => {}
            }
            position = position.saturating_add(end).saturating_add(1);
        }
    }

    /// Processes one conditional section at the start of `text` (§3.4);
    /// returns its byte length.
    fn conditional_section(
        &mut self,
        text: &str,
        at: &dyn Fn(usize, usize) -> Span,
        active: &mut Vec<String>,
    ) -> Result<usize, XmlError> {
        let after = text.get(3..).unwrap_or_default();
        let open = after
            .find('[')
            .ok_or_else(|| malformed("conditional section has no `[`", at(0, text.len())))?;
        let keyword_text = after
            .get(..open)
            .unwrap_or_default()
            .trim_matches([' ', '\t', '\r', '\n']);
        let keyword = if let Some(name) = keyword_text
            .strip_prefix('%')
            .and_then(|reference| reference.strip_suffix(';'))
        {
            // §3.4: the keyword may be given by a parameter entity.
            match self.parameters.get(name) {
                Some(Entity::Internal(value)) => {
                    value.trim_matches([' ', '\t', '\r', '\n']).to_owned()
                }
                Some(Entity::External) | None => {
                    return Err(unsupported(
                        "a conditional section keyword names a parameter entity that is not read",
                        at(0, open.saturating_add(4)),
                    ));
                }
            }
        } else {
            keyword_text.to_owned()
        };
        let body_start = open.saturating_add(4);
        let body = text.get(body_start..).unwrap_or_default();
        match keyword.as_str() {
            "INCLUDE" => {
                let consumed = self.declarations_in(
                    body,
                    &|start, end| {
                        at(
                            body_start.saturating_add(start),
                            body_start.saturating_add(end),
                        )
                    },
                    Terminator::Section,
                    active,
                )?;
                Ok(body_start.saturating_add(consumed))
            }
            "IGNORE" => {
                let consumed = ignored_section_end(body).ok_or_else(|| {
                    malformed(
                        "conditional section is not closed with `]]>`",
                        at(0, text.len()),
                    )
                })?;
                Ok(body_start.saturating_add(consumed))
            }
            _ => Err(malformed(
                "a conditional section keyword must be INCLUDE or IGNORE (XML 1.0 §3.4)",
                at(0, body_start),
            )),
        }
    }

    /// Includes the parameter entity `name` between declarations (§4.4.8).
    fn include_parameter(
        &mut self,
        name: &str,
        reference: Span,
        active: &mut Vec<String>,
    ) -> Result<(), XmlError> {
        let replacement = match self.parameters.get(name) {
            Some(Entity::Internal(text)) => text.clone(),
            // §5.1: not read; later entity and attribute-list declarations
            // are not processed.
            Some(Entity::External) | None => {
                self.unread_parameter = true;
                return Ok(());
            }
        };
        if active.iter().any(|open| open == name) {
            return Err(malformed(
                "parameter-entity reference is recursive (XML 1.0 §4.1 WFC: No Recursion)",
                reference,
            ));
        }
        if active.len() >= MAX_ENTITY_DEPTH {
            return Err(limit(
                "entity reference nesting",
                MAX_ENTITY_DEPTH,
                reference,
            ));
        }
        self.charge(replacement.len(), reference)?;
        active.push(name.to_owned());
        self.declarations_in(&replacement, &|_, _| reference, Terminator::End, active)?;
        active.pop();
        Ok(())
    }

    fn count_declaration(&mut self, span: Span) -> Result<(), XmlError> {
        self.declarations = self.declarations.saturating_add(1);
        if self.declarations > MAX_MARKUP_DECLARATIONS {
            return Err(limit(
                "markup declarations per document",
                MAX_MARKUP_DECLARATIONS,
                span,
            ));
        }
        Ok(())
    }

    /// Records one `<!ENTITY ...` declaration (without its closing `>`).
    fn declare(
        &mut self,
        declaration: &str,
        at: &dyn Fn(usize, usize) -> Span,
    ) -> Result<(), XmlError> {
        let whole = at(0, declaration.len().saturating_add(1));
        let mut rest = declaration
            .strip_prefix("<!ENTITY")
            .ok_or_else(|| malformed("expected an entity declaration", whole))?;
        if !rest.starts_with([' ', '\t', '\r', '\n']) {
            return Err(malformed("expected white space after `<!ENTITY`", whole));
        }
        rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        let parameter = rest.starts_with('%');
        if parameter {
            rest = rest.strip_prefix('%').unwrap_or(rest);
            if !rest.starts_with([' ', '\t', '\r', '\n']) {
                return Err(malformed("expected white space after `%`", whole));
            }
            rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        }
        let name_end = rest
            .find([' ', '\t', '\r', '\n'])
            .ok_or_else(|| malformed("entity declaration has no value", whole))?;
        let name = rest.get(..name_end).unwrap_or_default();
        if !is_name(name) {
            return Err(malformed("entity name is not an XML Name", whole));
        }
        rest = rest
            .get(name_end..)
            .unwrap_or_default()
            .trim_start_matches([' ', '\t', '\r', '\n']);
        if unquoted_percent(rest) {
            return Err(pe_in_markup(whole));
        }
        let entity = if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let body = rest.get(1..).unwrap_or_default();
            let close = body
                .find(quote)
                .ok_or_else(|| malformed("entity value literal is not closed", whole))?;
            let literal = body.get(..close).unwrap_or_default();
            if !body
                .get(close.saturating_add(1)..)
                .unwrap_or_default()
                .trim_matches([' ', '\t', '\r', '\n'])
                .is_empty()
            {
                return Err(malformed(
                    "unexpected content after the entity value",
                    whole,
                ));
            }
            Entity::Internal(declared_value(literal, whole)?)
        } else if rest.starts_with("SYSTEM") || rest.starts_with("PUBLIC") {
            Entity::External
        } else {
            return Err(malformed(
                "an entity declaration needs a quoted value or an external identifier",
                whole,
            ));
        };
        if self.unread_parameter {
            // §5.1: not processed after a parameter entity that is not read.
            return Ok(());
        }
        // XML 1.0 §4.2: "If the same entity is declared more than once, the
        // first declaration encountered is binding".
        if parameter {
            if scxml_types::exceeds_wasm_bound(
                self.parameters.len().saturating_add(1),
                MAX_PARAMETER_ENTITY_DECLARATIONS,
            ) && !self.parameters.contains_key(name)
            {
                return Err(limit(
                    "parameter entity declarations",
                    MAX_PARAMETER_ENTITY_DECLARATIONS,
                    whole,
                ));
            }
            self.parameters.entry(name.to_owned()).or_insert(entity);
            return Ok(());
        }
        if scxml_types::exceeds_wasm_bound(
            self.general.len().saturating_add(1),
            MAX_ENTITY_DECLARATIONS,
        ) && !self.general.contains_key(name)
        {
            return Err(limit(
                "general entity declarations",
                MAX_ENTITY_DECLARATIONS,
                whole,
            ));
        }
        // The predefined entities keep their meaning (§4.6).
        if !matches!(name, "lt" | "gt" | "amp" | "apos" | "quot") {
            self.general.entry(name.to_owned()).or_insert(entity);
        }
        Ok(())
    }

    /// Records one `<!ATTLIST ...` declaration (without its closing `>`):
    /// `AttlistDecl ::= '<!ATTLIST' S Name AttDef* S? '>'` and `AttDef ::= S
    /// Name S AttType S DefaultDecl` (XML 1.0 §3.3).
    fn attribute_list(
        &mut self,
        declaration: &str,
        at: &dyn Fn(usize, usize) -> Span,
    ) -> Result<(), XmlError> {
        let whole = at(0, declaration.len().saturating_add(1));
        let mut cursor = Cursor {
            text: declaration,
            position: "<!ATTLIST".len(),
        };
        if !cursor.space() {
            return Err(malformed("expected white space after `<!ATTLIST`", whole));
        }
        let element = cursor
            .name()
            .ok_or_else(|| malformed("attribute-list declaration names no element type", whole))?
            .to_owned();
        let mut definitions = Vec::new();
        loop {
            let spaced = cursor.space();
            if cursor.at_end() {
                break;
            }
            if !spaced {
                return Err(malformed(
                    "expected white space before an attribute definition",
                    whole,
                ));
            }
            let name = cursor
                .name()
                .ok_or_else(|| malformed("attribute definition has no XML Name", whole))?
                .to_owned();
            if !cursor.space() {
                return Err(malformed(
                    "expected white space after the attribute name",
                    whole,
                ));
            }
            let normalization = cursor.attribute_type().ok_or_else(|| {
                malformed(
                    "attribute type must be CDATA, a tokenized type, NOTATION, or an enumeration (XML 1.0 §3.3.1)",
                    whole,
                )
            })?;
            if !cursor.space() {
                return Err(malformed(
                    "expected white space after the attribute type",
                    whole,
                ));
            }
            let default = if cursor.keyword("#REQUIRED") || cursor.keyword("#IMPLIED") {
                None
            } else {
                if cursor.keyword("#FIXED") && !cursor.space() {
                    return Err(malformed("expected white space after `#FIXED`", whole));
                }
                let (start, literal) = cursor.literal().ok_or_else(|| {
                    malformed(
                        "attribute default must be #REQUIRED, #IMPLIED, or a quoted value",
                        whole,
                    )
                })?;
                let span = at(start, start.saturating_add(literal.len()).saturating_add(2));
                Some((literal.to_owned(), span))
            };
            definitions.push((name, normalization, default));
        }
        if self.unread_parameter {
            // §5.1: not processed after a parameter entity that is not read.
            return Ok(());
        }
        for (name, normalization, default) in definitions {
            let default = default
                .map(|(literal, span)| {
                    self.normalize_literal(&literal, normalization, span)
                        .map(|value| (value, span))
                })
                .transpose()?;
            if self
                .attribute_names
                .contains(&(element.clone(), name.clone()))
            {
                continue;
            }
            if scxml_types::exceeds_wasm_bound(
                self.attribute_definitions.saturating_add(1),
                MAX_ATTRIBUTE_DEFINITIONS,
            ) {
                return Err(limit(
                    "attribute definitions",
                    MAX_ATTRIBUTE_DEFINITIONS,
                    whole,
                ));
            }
            self.define_attribute(&element, name, normalization, default);
        }
        Ok(())
    }

    /// Keeps one attribute definition of the element type `element`,
    /// indexed for the lookups each tag makes (see [`ElementAttributes`]).
    fn define_attribute(
        &mut self,
        element: &str,
        name: String,
        normalization: ValueNormalization,
        default: Option<(String, Span)>,
    ) {
        self.attribute_definitions = self.attribute_definitions.saturating_add(1);
        self.attribute_names
            .insert((element.to_owned(), name.clone()));
        let indexed = self.attributes.entry(element.to_owned()).or_default();
        if normalization == ValueNormalization::Tokenized {
            indexed.tokenized.insert(name.clone());
        }
        if let Some((value, span)) = default {
            indexed
                .defaulted
                .push(DefaultedAttribute { name, value, span });
        }
    }

    /// Normalizes an attribute default literal (XML 1.0 §3.3.3): white
    /// space becomes a space, references are replaced (entity references
    /// recursively), and a tokenized type's value is further normalized.
    fn normalize_literal(
        &mut self,
        literal: &str,
        normalization: ValueNormalization,
        span: Span,
    ) -> Result<String, XmlError> {
        let mut out = String::new();
        self.normalize_into(&line_ends(literal), span, &mut Vec::new(), &mut out)?;
        self.charge(out.len(), span)?;
        Ok(match normalization {
            ValueNormalization::Cdata => out,
            ValueNormalization::Tokenized => collapse(&out),
        })
    }

    fn normalize_into(
        &mut self,
        text: &str,
        span: Span,
        active: &mut Vec<String>,
        out: &mut String,
    ) -> Result<(), XmlError> {
        let mut rest = text;
        while let Some(index) = rest.find(['&', '<', ' ', '\t', '\r', '\n']) {
            out.push_str(rest.get(..index).unwrap_or_default());
            let tail = rest.get(index..).unwrap_or_default();
            if tail.starts_with('<') {
                return Err(malformed(
                    "an attribute value cannot contain `<` (XML 1.0 §3.1 WFC: No < in Attribute Values)",
                    span,
                ));
            }
            if !tail.starts_with('&') {
                out.push(' ');
                rest = tail.get(1..).unwrap_or_default();
                continue;
            }
            let end = tail
                .find(';')
                .ok_or_else(|| malformed("unterminated reference in an attribute value", span))?;
            let reference = tail.get(1..end).unwrap_or_default();
            match reference {
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "amp" => out.push('&'),
                "apos" => out.push('\''),
                "quot" => out.push('"'),
                other if other.starts_with('#') => out.push(character_reference(other, span)?),
                other => {
                    let replacement = match self.general.get(other) {
                        Some(Entity::Internal(text)) => text.clone(),
                        Some(Entity::External) => {
                            return Err(malformed(
                                "an attribute value cannot refer to an external entity (XML 1.0 §3.1 WFC: No External Entity References)",
                                span,
                            ));
                        }
                        None => return Err(self.undeclared(other, span)),
                    };
                    if active.iter().any(|open| open == other) {
                        return Err(malformed(
                            "entity reference is recursive (XML 1.0 §4.1 WFC: No Recursion)",
                            span,
                        ));
                    }
                    if active.len() >= MAX_ENTITY_DEPTH {
                        return Err(limit("entity reference nesting", MAX_ENTITY_DEPTH, span));
                    }
                    self.references = self.references.saturating_add(1);
                    if self.references > MAX_DOCUMENT_ENTITY_REFERENCES {
                        return Err(limit(
                            "entity references expanded per document",
                            MAX_DOCUMENT_ENTITY_REFERENCES,
                            span,
                        ));
                    }
                    active.push(other.to_owned());
                    self.normalize_into(&replacement, span, active, out)?;
                    active.pop();
                }
            }
            if out.len() > MAX_ENTITY_EXPANSION_BYTES {
                return Err(limit(
                    "entity expansion bytes per reference",
                    MAX_ENTITY_EXPANSION_BYTES,
                    span,
                ));
            }
            rest = tail.get(end.saturating_add(1)..).unwrap_or_default();
        }
        out.push_str(rest);
        if out.len() > MAX_ENTITY_EXPANSION_BYTES {
            return Err(limit(
                "entity expansion bytes per reference",
                MAX_ENTITY_EXPANSION_BYTES,
                span,
            ));
        }
        Ok(())
    }

    /// The replacement text of the internal general entity `name` (XML 1.0
    /// §4.5), unexpanded, when declared.
    #[must_use]
    pub fn replacement(&self, name: &str) -> Option<&str> {
        match self.general.get(name) {
            Some(Entity::Internal(text)) => Some(text.as_str()),
            Some(Entity::External) | None => None,
        }
    }

    /// Whether a reference to an undeclared general entity may still be a
    /// well-formed one: the document declares a general entity, or a
    /// parameter entity that is not read may have declared it (§5.1).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.general.is_empty() && !self.unread_parameter
    }

    /// The failure of a reference to `name`, which no processed declaration
    /// declares: not read when a parameter entity that is not read may have
    /// declared it (§5.1), otherwise undefined (§4.1 WFC: Entity Declared).
    #[must_use]
    pub fn undeclared(&self, name: &str, span: Span) -> XmlError {
        if self.unread_parameter {
            unsupported(
                "the entity is not declared by the declarations processed: a parameter entity that is not read may declare it (XML 1.0 §5.1)",
                span,
            )
        } else {
            XmlError {
                kind: XmlErrorKind::UndefinedEntityReference,
                detail: XmlErrorDetail::Unclassified,
                message: format!("entity reference `&{name};` names no declared entity"),
                span,
            }
        }
    }

    /// What `&name;` in element content includes (§4.4.2): its fully
    /// expanded text when neither its replacement text nor any entity it
    /// refers to contains markup, else its replacement text for the parser
    /// to parse as `content`. `Ok(None)` when `name` is not declared (the
    /// caller then applies its predefined rules). The expanded text is
    /// charged to the document budget; markup is charged by the parser as
    /// it includes it.
    ///
    /// # Errors
    ///
    /// An external entity, a recursive or too deep reference, and an
    /// expansion over its bounds, at `span`.
    pub fn content(&mut self, name: &str, span: Span) -> Result<Option<Content>, XmlError> {
        let replacement = match self.general.get(name) {
            None => return Ok(None),
            Some(Entity::External) => return Err(not_read(span)),
            Some(Entity::Internal(text)) => text.clone(),
        };
        if self.has_markup(name, span, &mut Vec::new())? {
            self.charge(replacement.len(), span)?;
            return Ok(Some(Content::Markup(replacement)));
        }
        let text = self.expanded_text(name, span, &mut Vec::new())?;
        self.charge(text.len(), span)?;
        Ok(Some(Content::Text(text)))
    }

    /// Whether the replacement text of `name`, or of an entity it refers to,
    /// contains markup (`<`).
    fn has_markup(
        &mut self,
        name: &str,
        span: Span,
        active: &mut Vec<String>,
    ) -> Result<bool, XmlError> {
        if let Some(known) = self.markup.get(name) {
            return Ok(*known);
        }
        let text = match self.general.get(name) {
            Some(Entity::Internal(text)) => text.clone(),
            Some(Entity::External) => return Err(not_read(span)),
            None => return Err(self.undeclared(name, span)),
        };
        if text.contains('<') {
            self.markup.insert(name.to_owned(), true);
            return Ok(true);
        }
        if active.iter().any(|open| open == name) {
            return Err(malformed(
                "entity reference is recursive (XML 1.0 §4.1 WFC: No Recursion)",
                span,
            ));
        }
        if active.len() >= MAX_ENTITY_DEPTH {
            return Err(limit("entity reference nesting", MAX_ENTITY_DEPTH, span));
        }
        active.push(name.to_owned());
        let mut rest = text.as_str();
        while let Some(index) = rest.find('&') {
            let tail = rest.get(index..).unwrap_or_default();
            let end = tail
                .find(';')
                .ok_or_else(|| malformed("unterminated reference in entity text", span))?;
            let reference = tail.get(1..end).unwrap_or_default();
            if !reference.starts_with('#')
                && !matches!(reference, "lt" | "gt" | "amp" | "apos" | "quot")
                && self.has_markup(reference, span, active)?
            {
                self.markup.insert(name.to_owned(), true);
                return Ok(true);
            }
            rest = tail.get(end.saturating_add(1)..).unwrap_or_default();
        }
        active.pop();
        self.markup.insert(name.to_owned(), false);
        Ok(false)
    }

    /// Bytes `&name;` in an attribute value expands to, when declared;
    /// charges nothing.
    ///
    /// # Errors
    ///
    /// An external entity (§3.1 WFC: No External Entity References),
    /// replacement text containing `<` (WFC: No < in Attribute Values), a
    /// recursive or too deep reference, and an expansion over its bounds,
    /// at `span`.
    pub fn expanded_len(&mut self, name: &str, span: Span) -> Result<Option<usize>, XmlError> {
        match self.general.get(name) {
            None => return Ok(None),
            Some(Entity::External) => {
                return Err(malformed(
                    "an attribute value cannot refer to an external entity (XML 1.0 §3.1 WFC: No External Entity References)",
                    span,
                ));
            }
            Some(Entity::Internal(_)) => {}
        }
        if self.has_markup(name, span, &mut Vec::new())? {
            return Err(malformed(
                "an entity referred to in an attribute value cannot contain `<` (XML 1.0 §3.1 WFC: No < in Attribute Values)",
                span,
            ));
        }
        self.expanded_text(name, span, &mut Vec::new())
            .map(|text| Some(text.len()))
    }

    /// Charges `bytes` of expansion to the document budget.
    ///
    /// # Errors
    ///
    /// [`XmlErrorKind::ResourceLimit`] when the document budget is spent.
    pub fn charge(&mut self, bytes: usize, span: Span) -> Result<(), XmlError> {
        self.used = self.used.saturating_add(bytes);
        if self.used > MAX_DOCUMENT_EXPANSION_BYTES {
            return Err(limit(
                "entity expansion bytes per document",
                MAX_DOCUMENT_EXPANSION_BYTES,
                span,
            ));
        }
        Ok(())
    }

    /// The default attribute values the attribute-list declarations of the
    /// element type `element` supply, in declaration order.
    pub fn defaults<'a>(&'a self, element: &str) -> impl Iterator<Item = AttributeDefault<'a>> {
        self.attributes
            .get(element)
            .into_iter()
            .flat_map(|indexed| &indexed.defaulted)
            .map(|default| AttributeDefault {
                name: default.name.as_str(),
                value: default.value.as_str(),
                span: default.span,
            })
    }

    /// Normalizes a specified value of `attribute` of the element type
    /// `element` as its declared type requires (§3.3.3): a value of a type
    /// other than CDATA loses leading and trailing spaces and its space
    /// runs collapse to one; any other value is returned unchanged.
    #[must_use]
    pub fn normalize_declared(&self, element: &str, attribute: &str, value: String) -> String {
        let tokenized = self
            .attributes
            .get(element)
            .is_some_and(|indexed| indexed.tokenized.contains(attribute));
        if tokenized { collapse(&value) } else { value }
    }

    fn expanded_text(
        &mut self,
        name: &str,
        span: Span,
        active: &mut Vec<String>,
    ) -> Result<String, XmlError> {
        if let Some(text) = self.expanded.get(name) {
            return Ok(text.clone());
        }
        let replacement = match self.general.get(name) {
            Some(Entity::Internal(text)) => text.clone(),
            Some(Entity::External) => return Err(not_read(span)),
            None => return Err(self.undeclared(name, span)),
        };
        if active.iter().any(|open| open == name) {
            // XML 1.0 §4.1 WFC: No Recursion.
            return Err(malformed(
                "entity reference is recursive (XML 1.0 §4.1 WFC: No Recursion)",
                span,
            ));
        }
        if active.len() >= MAX_ENTITY_DEPTH {
            return Err(limit("entity reference nesting", MAX_ENTITY_DEPTH, span));
        }
        active.push(name.to_owned());
        let mut out = String::new();
        let mut rest = replacement.as_str();
        while let Some(index) = rest.find('&') {
            out.push_str(rest.get(..index).unwrap_or_default());
            let tail = rest.get(index..).unwrap_or_default();
            let end = tail
                .find(';')
                .ok_or_else(|| malformed("unterminated reference in entity text", span))?;
            let reference = tail.get(1..end).unwrap_or_default();
            match reference {
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "amp" => out.push('&'),
                "apos" => out.push('\''),
                "quot" => out.push('"'),
                other if other.starts_with('#') => {
                    out.push(character_reference(other, span)?);
                }
                other => {
                    let nested = self.expanded_text(other, span, active)?;
                    out.push_str(&nested);
                }
            }
            if out.len() > MAX_ENTITY_EXPANSION_BYTES {
                return Err(limit(
                    "entity expansion bytes per reference",
                    MAX_ENTITY_EXPANSION_BYTES,
                    span,
                ));
            }
            rest = tail.get(end.saturating_add(1)..).unwrap_or_default();
        }
        out.push_str(rest);
        active.pop();
        if out.len() > MAX_ENTITY_EXPANSION_BYTES {
            return Err(limit(
                "entity expansion bytes per reference",
                MAX_ENTITY_EXPANSION_BYTES,
                span,
            ));
        }
        self.expanded.insert(name.to_owned(), out.clone());
        Ok(out)
    }
}

/// The tokens of an attribute-type group (§3.3.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GroupTokens {
    /// Names: a `NOTATION` type.
    Names,
    /// Name tokens: an enumeration.
    Nmtokens,
}

/// A position in one markup declaration.
struct Cursor<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        self.text.get(self.position..).unwrap_or_default()
    }

    fn at_end(&self) -> bool {
        self.rest().is_empty()
    }

    /// Skips white space; whether there was any.
    fn space(&mut self) -> bool {
        let rest = self.rest();
        let skipped = rest.len() - rest.trim_start_matches([' ', '\t', '\r', '\n']).len();
        self.position = self.position.saturating_add(skipped);
        skipped > 0
    }

    /// One XML `Name`.
    fn name(&mut self) -> Option<&'a str> {
        let rest = self.rest();
        let end = rest
            .find([' ', '\t', '\r', '\n', '(', ')', '|', '"', '\''])
            .unwrap_or(rest.len());
        let name = rest.get(..end)?;
        is_name(name).then(|| {
            self.position = self.position.saturating_add(end);
            name
        })
    }

    fn keyword(&mut self, keyword: &str) -> bool {
        let rest = self.rest();
        let matched = rest.starts_with(keyword)
            && !rest
                .get(keyword.len()..)
                .and_then(|after| after.chars().next())
                .is_some_and(is_name_char);
        if matched {
            self.position = self.position.saturating_add(keyword.len());
        }
        matched
    }

    /// `AttType`, as the normalization its values receive.
    fn attribute_type(&mut self) -> Option<ValueNormalization> {
        if self.keyword("CDATA") {
            return Some(ValueNormalization::Cdata);
        }
        for tokenized in [
            "IDREFS", "IDREF", "ID", "ENTITIES", "ENTITY", "NMTOKENS", "NMTOKEN",
        ] {
            if self.keyword(tokenized) {
                return Some(ValueNormalization::Tokenized);
            }
        }
        if self.keyword("NOTATION") {
            self.space();
            return self
                .group(GroupTokens::Names)
                .then_some(ValueNormalization::Tokenized);
        }
        self.group(GroupTokens::Nmtokens)
            .then_some(ValueNormalization::Tokenized)
    }

    /// `'(' S? token (S? '|' S? token)* S? ')'` of names (a notation type)
    /// or name tokens (an enumeration).
    fn group(&mut self, tokens: GroupTokens) -> bool {
        let Some(rest) = self.rest().strip_prefix('(') else {
            return false;
        };
        let Some(close) = rest.find(')') else {
            return false;
        };
        let body = rest.get(..close).unwrap_or_default();
        let valid = body.split('|').all(|token| {
            let token = token.trim_matches([' ', '\t', '\r', '\n']);
            match tokens {
                GroupTokens::Names => is_name(token),
                GroupTokens::Nmtokens => !token.is_empty() && token.chars().all(is_name_char),
            }
        });
        if valid {
            self.position = self.position.saturating_add(close).saturating_add(2);
        }
        valid
    }

    /// One quoted literal: its start offset (at the quote) and its body.
    fn literal(&mut self) -> Option<(usize, &'a str)> {
        let rest = self.rest();
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let body = rest.get(1..)?;
        let close = body.find(quote)?;
        let start = self.position;
        self.position = self.position.saturating_add(close).saturating_add(2);
        Some((start, body.get(..close)?))
    }
}

/// Index of the `[` opening the internal subset of a DOCTYPE's content,
/// skipping the quoted literals of an external identifier.
fn subset_open(content: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in content.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if open == character => quote = None,
            (None, '[') => return Some(index),
            _ => {}
        }
    }
    None
}

/// Length of an `IGNORE` section body through its `]]>`, nested sections
/// included (§3.4 `ignoreSectContents`).
fn ignored_section_end(body: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut position = 0usize;
    while let Some(rest) = body.get(position..) {
        let open = rest.find("<![");
        let close = rest.find("]]>")?;
        match open {
            Some(open) if open < close => {
                depth = depth.checked_add(1)?;
                position = position.saturating_add(open).saturating_add(3);
            }
            _ => {
                depth -= 1;
                position = position.saturating_add(close).saturating_add(3);
                if depth == 0 {
                    return Some(position);
                }
            }
        }
    }
    None
}

/// The replacement text of an entity value literal (XML 1.0 §4.5):
/// line ends are normalized (§2.11), character references are replaced now;
/// general entity references are kept for expansion on use; a
/// parameter-entity reference cannot occur inside a markup declaration in
/// the internal subset (§2.8 WFC: PEs in Internal Subset).
fn declared_value(literal: &str, span: Span) -> Result<String, XmlError> {
    if literal.contains('%') {
        return Err(pe_in_markup(span));
    }
    if literal.len() > MAX_ENTITY_EXPANSION_BYTES {
        return Err(limit(
            "entity expansion bytes per reference",
            MAX_ENTITY_EXPANSION_BYTES,
            span,
        ));
    }
    let literal = line_ends(literal);
    let mut out = String::with_capacity(literal.len());
    let mut rest = literal.as_str();
    while let Some(index) = rest.find('&') {
        out.push_str(rest.get(..index).unwrap_or_default());
        let tail = rest.get(index..).unwrap_or_default();
        let end = tail
            .find(';')
            .ok_or_else(|| malformed("unterminated reference in an entity value", span))?;
        let reference = tail.get(1..end).unwrap_or_default();
        if reference.starts_with('#') {
            out.push(character_reference(reference, span)?);
        } else if is_name(reference) {
            out.push_str(tail.get(..=end).unwrap_or_default());
        } else {
            return Err(malformed("invalid reference in an entity value", span));
        }
        rest = tail.get(end.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(out)
}

/// XML 1.0 §2.11: `#xD#xA` and a lone `#xD` become `#xA`.
fn line_ends(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// §3.3.3 further normalization of a tokenized type's value.
fn collapse(value: &str) -> String {
    value
        .split(' ')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The character a `#…` reference body names (XML 1.0 §4.1).
fn character_reference(reference: &str, span: Span) -> Result<char, XmlError> {
    let (digits, radix) = match reference.strip_prefix("#x") {
        Some(hex) => (hex, 16),
        None => (reference.strip_prefix('#').unwrap_or_default(), 10),
    };
    let invalid = || XmlError {
        kind: XmlErrorKind::InvalidCharacterReference,
        detail: XmlErrorDetail::Unclassified,
        message: "invalid XML character reference in entity text".into(),
        span,
    };
    if digits.is_empty() || digits.len() > 8 || !digits.chars().all(|c| c.is_digit(radix)) {
        return Err(invalid());
    }
    u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
        .filter(|character| {
            matches!(
                character,
                '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
            )
        })
        .ok_or_else(invalid)
}

/// Whether `text` holds a `%` outside its quoted literals: a
/// parameter-entity reference inside a markup declaration.
fn unquoted_percent(text: &str) -> bool {
    let mut quote = None;
    text.chars().any(|character| match (quote, character) {
        (None, '"' | '\'') => {
            quote = Some(character);
            false
        }
        (Some(open), _) if open == character => {
            quote = None;
            false
        }
        (None, '%') => true,
        _ => false,
    })
}

/// Index of the `>` that closes a markup declaration, skipping quoted
/// literals.
fn declaration_end(declaration: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in declaration.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if open == character => quote = None,
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// XML 1.0 `Name` (checked loosely for non-ASCII characters: quick-xml
/// checks the document).
fn is_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first == ':' || first.is_alphabetic())
        && characters.all(is_name_char)
}

fn is_name_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | ':' | '-' | '.' | '\u{B7}')
}

fn pe_in_markup(span: Span) -> XmlError {
    malformed(
        "a parameter-entity reference cannot occur inside a markup declaration in the internal subset (XML 1.0 §2.8 WFC: PEs in Internal Subset)",
        span,
    )
}

fn not_read(span: Span) -> XmlError {
    unsupported(
        "external entities are never read by this parser (XML 1.0 §4.4.3 lets a non-validating processor not include them)",
        span,
    )
}

fn malformed(message: &str, span: Span) -> XmlError {
    XmlError {
        kind: XmlErrorKind::Malformed,
        detail: XmlErrorDetail::Unclassified,
        message: message.to_owned(),
        span,
    }
}

fn unsupported(message: &str, span: Span) -> XmlError {
    XmlError {
        kind: XmlErrorKind::UnsupportedDtdConstruct,
        detail: XmlErrorDetail::Unclassified,
        message: message.to_owned(),
        span,
    }
}

fn limit(resource: &str, limit: usize, span: Span) -> XmlError {
    XmlError {
        kind: XmlErrorKind::ResourceLimit,
        detail: XmlErrorDetail::Unclassified,
        message: format!("XML resource limit exceeded: {resource} > {limit}"),
        span,
    }
}
