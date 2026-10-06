//! Stable diagnostics for XML trust-boundary failures.
//!
//! Every [`XmlErrorKind`] maps to exactly one stable code so an AI repair loop
//! can dispatch on the code alone:
//!
//! | Kind | Code |
//! | --- | --- |
//! | `Malformed` | `XML001` |
//! | `ResourceLimit` | `XML002` |
//! | `UndefinedEntityReference` | `XML003` |
//! | `InvalidCharacterReference` | `XML004` |
//! | `InvalidLimits` | `XML005` |
//!
//! Reference failures label exactly the `&…;` bytes and, where the intent is
//! unambiguous, carry a structured repair.

use scxml_syntax::{SourceText, Span, XmlError, XmlErrorDetail, XmlErrorKind, parse_xml};

use crate::diagnostic::FixEdit;

use crate::{Applicability, Diagnostic, DiagnosticCategory, Fix, Label};

/// Maximum bytes searched around a reported reference to recover `&…;`.
const MAX_REFERENCE_BYTES: usize = 64;
/// Maximum bytes searched backwards for the start tag a mismatched end tag
/// leaves unclosed; beyond it the opener is not labeled.
const MAX_OPENER_SEARCH_BYTES: usize = 64 * 1024;

/// Longest element name compared for an end-tag misspelling.
const MAX_TAG_TYPO_CHARS: usize = 64;

/// Shared reason every XML001 diagnostic carries.
const WELL_FORMED_NOTE: &str = "SCXML documents must be well-formed XML 1.0 with namespaces; no later validation runs until parsing succeeds";

/// Returns the stable diagnostic code for one XML failure kind.
#[must_use]
pub const fn xml_error_code(kind: XmlErrorKind) -> &'static str {
    match kind {
        XmlErrorKind::Malformed => "XML001",
        XmlErrorKind::ResourceLimit => "XML002",
        XmlErrorKind::UndefinedEntityReference => "XML003",
        XmlErrorKind::InvalidCharacterReference => "XML004",
        XmlErrorKind::InvalidLimits => "XML005",
        XmlErrorKind::UnsupportedDtdConstruct => "XML006",
    }
}

/// HTML named character references commonly pasted into XML, with the
/// code point each denotes. XML predefines only `lt`, `gt`, `amp`, `apos`, and
/// `quot`; everything else must be written as a numeric character reference.
const HTML_ENTITIES: &[(&str, u32)] = &[
    ("nbsp", 160),
    ("iexcl", 161),
    ("cent", 162),
    ("pound", 163),
    ("yen", 165),
    ("sect", 167),
    ("copy", 169),
    ("laquo", 171),
    ("reg", 174),
    ("deg", 176),
    ("plusmn", 177),
    ("micro", 181),
    ("para", 182),
    ("middot", 183),
    ("raquo", 187),
    ("frac12", 189),
    ("times", 215),
    ("divide", 247),
    ("ndash", 8211),
    ("mdash", 8212),
    ("lsquo", 8216),
    ("rsquo", 8217),
    ("ldquo", 8220),
    ("rdquo", 8221),
    ("bull", 8226),
    ("hellip", 8230),
    ("euro", 8364),
    ("trade", 8482),
    ("larr", 8592),
    ("uarr", 8593),
    ("rarr", 8594),
    ("darr", 8595),
    ("harr", 8596),
    ("infin", 8734),
    ("ne", 8800),
    ("le", 8804),
    ("ge", 8805),
];

/// Converts one XML failure into its stable, span-exact diagnostic.
#[must_use]
pub fn xml_error_diagnostic(source: &SourceText, error: &XmlError) -> Diagnostic {
    let code = xml_error_code(error.kind);

    match error.kind {
        XmlErrorKind::Malformed => embedded_document(source, error)
            .or_else(|| classified_malformation(source, error))
            .unwrap_or_else(|| {
            unclosed_at_end(source, error)
                .or_else(|| attribute_without_value(source, error))
                .or_else(|| unterminated_attribute_value(source, error))
                .or_else(|| mismatched_end_tag(source, error))
                .or_else(|| unquoted_attribute(source, error))
                .unwrap_or_else(|| {
                    Diagnostic::error(
                        code,
                        format!("malformed XML: {}", parser_message(&error.message)),
                        error.span,
                    )
                    .with_primary_message("not well-formed XML")
                    .with_help(malformed_help(source, error.span))
                })
                .with_note(WELL_FORMED_NOTE)
                .with_spec_reference("XML 1.0 (Fifth Edition) §2.1 Well-Formed XML Documents")
        }),
        XmlErrorKind::ResourceLimit => Diagnostic::error(code, error.message.clone(), error.span)
            .with_primary_message("resource budget exceeded here")
            .with_note("the validator bounds nesting depth and entity expansion on every build, and source size, parser events, nodes, and attributes on wasm builds only, so hostile input cannot exhaust resources; the limits are listed in docs/validator-rules.md")
            .with_help("split the machine into smaller documents (for example with `<invoke>`), reduce nesting, or validate with larger explicitly configured limits"),
        XmlErrorKind::UndefinedEntityReference => undefined_entity(source, error),
        XmlErrorKind::UnsupportedDtdConstruct => Diagnostic::error(code, error.message.clone(), error.span)
            .with_primary_message("this entity is not read")
            .with_note("the parser processes the internal DTD subset as XML 1.0 §5.1 requires of a non-validating processor; it never reads an external entity (XML 1.0 §4.4.3 lets a non-validating processor not include one, and reading it could disclose files or reach the network), and after a reference to a parameter entity that is not read it does not process later entity and attribute-list declarations (§5.1)")
            .with_help("write the text in place, or declare an internal entity (`<!ENTITY name \"text\">`) in the internal subset")
            .with_spec_reference("XML 1.0 (Fifth Edition) §4.4 XML Processor Treatment of Entities and References"),
        XmlErrorKind::InvalidCharacterReference => invalid_character_reference(source, error),
        XmlErrorKind::InvalidLimits => Diagnostic::error(code, error.message.clone(), error.span)
            .with_category(DiagnosticCategory::Internal)
            .with_primary_message("validation did not start")
            .with_note("this is a caller configuration error, not a problem in the document")
            .with_help("configure every XML limit with a non-zero maximum"),
    }
}

/// Builds the diagnostic for a classified well-formedness violation: exact
/// labels, violation-specific help, the precise XML 1.0 constraint, and a
/// repair whose applicability is honest about the author's intent.
fn classified_malformation(source: &SourceText, error: &XmlError) -> Option<Diagnostic> {
    let diagnostic = match &error.detail {
        XmlErrorDetail::Unclassified => return None,
        XmlErrorDetail::TextOutsideRoot { removals } => text_outside_root(source, error, removals),
        XmlErrorDetail::LessThanInAttributeValue { occurrences } => {
            less_than_in_attribute(error, occurrences)
        }
        XmlErrorDetail::LessThanInText { element } => less_than_in_text(error, element.as_deref()),
        XmlErrorDetail::DoubleHyphenInComment { content } => {
            comment_hyphens(source, error, content)?
        }
        XmlErrorDetail::MisplacedXmlDeclaration { leading } => {
            misplaced_declaration(error, leading.as_ref())
        }
        XmlErrorDetail::DuplicateAttribute { first, removal } => {
            duplicate_attribute(source, error, first, removal)
        }
        XmlErrorDetail::MultipleRootElements { first_root } => {
            second_root(source, error, first_root)
        }
    };
    Some(diagnostic)
}

fn less_than_in_attribute(error: &XmlError, occurrences: &[Span]) -> Diagnostic {
    let code = xml_error_code(XmlErrorKind::Malformed);
    let mut diagnostic = Diagnostic::error(code, error.message.clone(), error.span)
        .with_primary_message("a literal `<` is not allowed in an attribute value")
        .with_note("the parser cannot tell a literal `<` from the start of a tag, so XML forbids it in attribute values; `>` needs no escaping")
        .with_help("escape `<` as `&lt;` (for example `cond=\"x &lt; 3\"`)")
        .with_spec_reference("XML 1.0 (Fifth Edition) §3.1 Start-Tags (WFC: No < in Attribute Values)");
    for other in occurrences.iter().skip(1) {
        diagnostic = diagnostic.with_secondary(Label::new(*other).with_message("also unescaped"));
    }
    diagnostic.with_fix(Fix {
        message: if occurrences.len() > 1 {
            "escape every `<` in this value as `&lt;`".to_owned()
        } else {
            "escape `<` as `&lt;`".to_owned()
        },
        applicability: Applicability::MachineApplicable,
        edits: occurrences
            .iter()
            .map(|span| FixEdit {
                span: *span,
                replacement: "&lt;".to_owned(),
            })
            .collect(),
    })
}

fn less_than_in_text(error: &XmlError, element: Option<&str>) -> Diagnostic {
    let code = xml_error_code(XmlErrorKind::Malformed);
    let local = element.map(|name| name.rsplit(':').next().unwrap_or(name));
    let message = element.map_or_else(
        || error.message.clone(),
        |name| format!("unescaped `<` in the text of `<{name}>`"),
    );
    let help = if local == Some("script") {
        "escape the operator as `&lt;` (and `&` as `&amp;`), or wrap the whole script body in `<![CDATA[ ... ]]>` so it needs no escaping".to_owned()
    } else {
        "escape a literal `<` as `&lt;`, or wrap the text in `<![CDATA[ ... ]]>`".to_owned()
    };
    Diagnostic::error(code, message, error.span)
        .with_primary_message("`<` starts a tag here, but no element name follows it")
        .with_note("in character data `<` always starts markup; a comparison operator must be escaped or placed in a CDATA section")
        .with_help(help)
        .with_spec_reference("XML 1.0 (Fifth Edition) §2.4 Character Data and Markup")
        .with_fix(Fix::replace(
            "escape `<` as `&lt;`",
            Applicability::MachineApplicable,
            error.span,
            "&lt;",
        ))
}

fn comment_hyphens(source: &SourceText, error: &XmlError, content: &Span) -> Option<Diagnostic> {
    let code = xml_error_code(XmlErrorKind::Malformed);
    let trailing = source.slice(error.span) == Some("-");
    let original = source.slice(*content)?;
    let mut repaired = String::with_capacity(original.len().saturating_add(1));
    for character in original.chars() {
        if character == '-' && repaired.ends_with('-') {
            continue;
        }
        repaired.push(character);
    }
    if repaired.ends_with('-') {
        repaired.push(' ');
    }
    Some(
    Diagnostic::error(code, if trailing { "a comment must not end with `-` before `-->`" } else { "`--` inside a comment" }, error.span)
        .with_primary_message(if trailing {
            "this `-` makes the comment end with `--->`"
        } else {
            "`--` is not allowed inside a comment"
        })
        .with_note("XML reserves `--` inside comments (for SGML compatibility), so a comment cannot contain it or end with `--->`")
        .with_help("use a single `-` inside comments, and leave a space before the closing `-->`")
        .with_spec_reference("XML 1.0 (Fifth Edition) §2.5 Comments")
        .with_fix(Fix::replace(
            "collapse repeated hyphens in the comment",
            Applicability::MachineApplicable,
            *content,
            repaired,
        ))
    )
}

fn misplaced_declaration(error: &XmlError, leading: Option<&Span>) -> Diagnostic {
    let code = xml_error_code(XmlErrorKind::Malformed);
    let diagnostic = Diagnostic::error(code, "XML declaration is not at the start of the document", error.span)
        .with_primary_message("`<?xml …?>` is only allowed as the very first bytes of the document")
        .with_note("the XML declaration must precede everything, even whitespace; elsewhere the name `xml` is reserved and `<?xml` is not well formed")
        .with_spec_reference("XML 1.0 (Fifth Edition) §2.8 Prolog and Document Type Declaration");
    match leading {
        Some(leading) => diagnostic
            .with_help("remove the blank lines and spaces before `<?xml`")
            .with_fix(Fix::replace(
                "remove the whitespace before the XML declaration",
                Applicability::MachineApplicable,
                *leading,
                "",
            )),
        None => diagnostic
            .with_help("move the XML declaration to the first line, or delete it: it is optional for UTF-8 XML 1.0 documents")
            .with_fix(Fix::replace(
                "delete the misplaced XML declaration",
                Applicability::MaybeIncorrect,
                error.span,
                "",
            )),
    }
}

fn duplicate_attribute(
    source: &SourceText,
    error: &XmlError,
    first: &Span,
    removal: &Span,
) -> Diagnostic {
    let code = xml_error_code(XmlErrorKind::Malformed);
    let name = source.slice(*first).unwrap_or_default();
    Diagnostic::error(code, error.message.clone(), error.span)
        .with_primary_message(format!("`{name}` is repeated here"))
        .with_secondary(Label::new(*first).with_message(format!("`{name}` is first specified here")))
        .with_note("each attribute may appear at most once in a start tag; a parser cannot choose between two values")
        .with_help(if name == "target" {
            "keep one `target` attribute; a transition to several states lists their IDs in one value, separated by spaces (`target=\"a b\"`)".to_owned()
        } else {
            format!("keep exactly one `{name}` attribute and remove the other")
        })
        .with_spec_reference("XML 1.0 (Fifth Edition) §3.1 Start-Tags (WFC: Unique Att Spec)")
        .with_fix(Fix::replace(
            format!("remove the repeated `{name}` attribute"),
            Applicability::MaybeIncorrect,
            *removal,
            "",
        ))
}

fn second_root(source: &SourceText, error: &XmlError, first_root: &Span) -> Diagnostic {
    let code = xml_error_code(XmlErrorKind::Malformed);
    let first = source.slice(*first_root).unwrap_or_default();
    Diagnostic::error(code, error.message.clone(), error.span)
        .with_primary_message("second top-level element")
        .with_secondary(Label::new(*first_root).with_message(format!("the document's root `<{first}>` starts here")))
        .with_note("an XML document has exactly one root element; everything else must be nested inside it")
        .with_help(format!("remove this element or move it inside `<{first}>`; to define another state machine, put it in its own file (and start it with `<invoke>` if needed)"))
        .with_spec_reference("XML 1.0 (Fifth Edition) §2.1 Well-Formed XML Documents")
}

/// Text around the document, typically a Markdown code fence or chat prose.
fn text_outside_root(source: &SourceText, error: &XmlError, removals: &[Span]) -> Diagnostic {
    let code = xml_error_code(XmlErrorKind::Malformed);
    let primary_text = source.slice(error.span).unwrap_or_default();
    let fence = primary_text.starts_with("```") || primary_text.starts_with("~~~");
    let invisible = primary_text == "\u{FEFF}";
    let mut diagnostic = Diagnostic::error(
        code,
        if fence {
            "Markdown code fence around the XML document"
        } else if invisible {
            "a second byte order mark (U+FEFF) outside the root element"
        } else {
            "text outside the root element"
        },
        error.span,
    )
    .with_primary_message(if fence {
        "a code fence is Markdown, not XML"
    } else if invisible {
        "invisible U+FEFF character here"
    } else {
        "only whitespace, comments, and processing instructions may appear outside the root element"
    })
    .with_note("an SCXML file contains exactly one XML document: an optional `<?xml …?>` declaration, then the single root element; any other text before or after it is not well formed")
    .with_help(if fence {
        "remove the Markdown code fences: save only the XML document itself"
    } else if invisible {
        "remove the duplicated byte order mark; at most one may start the file"
    } else {
        "remove the text outside the root element, or turn it into an XML comment `<!-- … -->`"
    })
    .with_spec_reference("XML 1.0 (Fifth Edition) §2.1 Well-Formed XML Documents");
    for removal in removals {
        if removal.start <= error.span.start && error.span.end <= removal.end {
            continue;
        }
        let text = source.slice(*removal).unwrap_or_default();
        let leading = text.len() - text.trim_start().len();
        let line = text
            .trim_start()
            .lines()
            .next()
            .unwrap_or_default()
            .trim_end();
        if line.is_empty() {
            continue;
        }
        let start = removal.start.saturating_add(leading);
        diagnostic = diagnostic.with_secondary(
            Label::new(Span::new(
                removal.source,
                start,
                start.saturating_add(line.len()),
            ))
            .with_message("also outside the root element"),
        );
    }
    // Deleting pure text cannot change the document; deleting a range that
    // contains markup might drop something the author meant to keep.
    let markup_free = removals.iter().all(|removal| {
        source
            .slice(*removal)
            .is_some_and(|text| !text.contains('<'))
    });
    diagnostic.with_fix(Fix {
        message: if fence {
            "remove the Markdown code fences".to_owned()
        } else {
            "remove the text outside the root element".to_owned()
        },
        applicability: if markup_free {
            Applicability::MachineApplicable
        } else {
            Applicability::MaybeIncorrect
        },
        edits: removals
            .iter()
            .map(|span| FixEdit {
                span: *span,
                replacement: String::new(),
            })
            .collect(),
    })
}

/// `XML001` for bytes that are not UTF-8. `source` is the document decoded
/// with U+FFFD for each invalid sequence; the invalid sequence starts at
/// the same offset there as in `bytes`, since everything before it is
/// valid.
pub(crate) fn invalid_utf8_diagnostic(
    source: &SourceText,
    bytes: &[u8],
    error: &std::str::Utf8Error,
) -> Diagnostic {
    let offset = error.valid_up_to();
    let replacement = '\u{FFFD}'.len_utf8();
    let span = Span::new(source.id(), offset, offset.saturating_add(replacement));
    let invalid: Vec<String> = bytes
        .get(offset..offset.saturating_add(error.error_len().unwrap_or(1).max(1)))
        .unwrap_or_default()
        .iter()
        .map(|byte| format!("0x{byte:02X}"))
        .collect();
    let invalid = invalid.join(" ");
    let declared = source
        .text()
        .strip_prefix('\u{FEFF}')
        .unwrap_or(source.text())
        .starts_with("<?xml")
        && source
            .text()
            .split("?>")
            .next()
            .is_some_and(|declaration| declaration.contains("encoding"));
    Diagnostic::error(
        xml_error_code(XmlErrorKind::Malformed),
        "the document is not valid UTF-8",
        span,
    )
    .with_primary_message(format!("byte {invalid} is not valid UTF-8 (shown as U+FFFD)"))
    .with_note(if declared {
        "this validator reads SCXML documents as UTF-8 and does not transcode other encodings, even when the XML declaration names one; no other rule ran on this document"
    } else {
        "an XML document without an `encoding` declaration must be UTF-8 (or UTF-16 with a byte order mark), and this validator reads UTF-8 only; no other rule ran on this document"
    })
    .with_help("save the file as UTF-8 (a file saved as Latin-1 or Windows-1252 converts with `iconv -f WINDOWS-1252 -t UTF-8`), or write the character as a numeric character reference such as `&#233;`")
    .with_spec_reference("XML 1.0 (Fifth Edition) §4.3.3 Character Encoding in Entities")
}

/// Whether `text` parses as XML. Used only to verify a repair before it is
/// offered; the document bytes are already bounded by the caller's limits.
fn reparses(source: &SourceText, text: String) -> Result<(), XmlError> {
    parse_xml(SourceText::new(source.id(), "repair.scxml", text)).map(drop)
}

/// Whether applying `edits` to the source removes the reported error:
/// the repaired document parses, or its first error lies after `after`.
fn repair_progresses(source: &SourceText, edits: &[FixEdit], after: usize) -> bool {
    let mut text = source.text().to_owned();
    let mut ordered: Vec<&FixEdit> = edits.iter().collect();
    ordered.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
    for edit in ordered {
        if edit.span.end > text.len()
            || !text.is_char_boundary(edit.span.start)
            || !text.is_char_boundary(edit.span.end)
        {
            return false;
        }
        text.replace_range(edit.span.start..edit.span.end, &edit.replacement);
    }
    match reparses(source, text) {
        Ok(()) => true,
        Err(error) => error.span.start > after,
    }
}

/// `<scxml` start tags in `text` whose start tag declares the SCXML
/// namespace: the offset of `<`.
fn namespaced_scxml_start(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(relative) = text.get(from..)?.find("<scxml") {
        let start = from + relative;
        let after = text.get(start + "<scxml".len()..)?;
        let boundary = after.chars().next()?;
        if boundary.is_whitespace() || boundary == '>' {
            let tag_end = after.find('>')?;
            if after.get(..tag_end)?.contains(crate::SCXML_NAMESPACE) {
                return Some(start);
            }
        }
        from = start + 1;
    }
    None
}

/// Whether `part` has non-whitespace text outside `<…>` markup.
fn has_prose(part: &str) -> bool {
    let mut depth = 0_usize;
    part.chars().any(|character| match character {
        '<' => {
            depth += 1;
            false
        }
        '>' => {
            depth = depth.saturating_sub(1);
            false
        }
        _ => depth == 0 && !character.is_whitespace(),
    })
}

/// Chat output with an SCXML document inside it ("Here is the `<scxml>`
/// machine:" followed by a fenced document): markup in the surrounding
/// prose makes the parser take the prose's `<scxml>` for the root, so the
/// ordinary text-outside-root repair removes only part of the prose. When
/// the text contains one SCXML-namespaced document that parses on its own
/// and the text around it contains markup, one repair removes everything
/// around that document.
fn embedded_document(source: &SourceText, error: &XmlError) -> Option<Diagnostic> {
    const END_TAG: &str = "</scxml>";
    let text = source.text();
    let base = if text.starts_with('\u{FEFF}') {
        '\u{FEFF}'.len_utf8()
    } else {
        0
    };
    let root_start = namespaced_scxml_start(text)?;
    let root_end = text.rfind(END_TAG).filter(|end| *end > root_start)? + END_TAG.len();
    // Keep an XML declaration directly before the root.
    let head = text.get(base..root_start)?;
    let keep_start = head
        .rfind("<?xml")
        .map(|offset| base + offset)
        .filter(|declaration| {
            text.get(*declaration..root_start)
                .and_then(|between| between.split_once("?>"))
                .is_some_and(|(_, rest)| rest.trim().is_empty())
        })
        .unwrap_or(root_start);
    let before = text.get(base..keep_start)?;
    let after = text.get(root_end..)?;
    // Only prose that contains markup: plain text or fences around the
    // document are the ordinary text-outside-root finding, and markup alone
    // (a second root element) has its own.
    if matches!(error.detail, XmlErrorDetail::MultipleRootElements { .. })
        || !(before.contains('<') || after.contains('<'))
        || !(has_prose(before) || has_prose(after))
    {
        return None;
    }
    let document = text.get(keep_start..root_end)?;
    reparses(source, format!("{document}\n")).ok()?;
    let fence = |line: &str| {
        let line = line.trim();
        line.starts_with("```") || line.starts_with("~~~")
    };
    let fenced = before.trim_end().lines().next_back().is_some_and(fence)
        && after.trim_start().lines().next().is_none_or(fence);
    let line_span = |start: usize, part: &str| {
        let leading = part.len() - part.trim_start().len();
        let line = part
            .trim_start()
            .lines()
            .next()
            .unwrap_or_default()
            .trim_end();
        let from = start + leading;
        (!line.is_empty()).then(|| Span::new(error.span.source, from, from + line.len()))
    };
    let prose_before = line_span(base, before);
    let prose_after = line_span(root_end, after);
    let primary = prose_before.or(prose_after)?;
    let mut diagnostic = Diagnostic::error(
        xml_error_code(XmlErrorKind::Malformed),
        if fenced {
            "chat text and Markdown fences around the SCXML document"
        } else {
            "text outside the SCXML document"
        },
        primary,
    )
    .with_primary_message("not part of the XML document")
    .with_secondary(
        Label::new(Span::new(
            error.span.source,
            root_start + 1,
            root_start + "<scxml".len(),
        ))
        .with_message("the SCXML document starts here"),
    );
    if let (Some(_), Some(after_span)) = (prose_before, prose_after) {
        diagnostic = diagnostic
            .with_secondary(Label::new(after_span).with_message("also outside the document"));
    }
    let mut edits = Vec::new();
    if !before.is_empty() {
        edits.push(FixEdit {
            span: Span::new(error.span.source, base, keep_start),
            replacement: String::new(),
        });
    }
    if !after.trim().is_empty() {
        edits.push(FixEdit {
            span: Span::new(error.span.source, root_end, text.len()),
            replacement: if text.ends_with('\n') { "\n" } else { "" }.to_owned(),
        });
    }
    Some(
        diagnostic
            .with_note("an SCXML file contains exactly one XML document; the `<scxml>` in the surrounding text is prose, but the parser reads it as the root element, so the real document is nested inside it and never closed")
            .with_help("save only the XML document: remove everything before `<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" …>` and after its `</scxml>`")
            .with_spec_reference("XML 1.0 (Fifth Edition) §2.1 Well-Formed XML Documents")
            .with_fix(Fix {
                message: "keep only the SCXML document".to_owned(),
                applicability: if fenced {
                    Applicability::MachineApplicable
                } else {
                    Applicability::MaybeIncorrect
                },
                edits,
            }),
    )
}

/// Most end tags a truncated-document repair inserts.
const MAX_INSERTED_END_TAGS: usize = 32;

/// An element left unclosed at the end of the document: the document was
/// cut off (or its last end tags were never written). Labels where the
/// document ends and where the element opens, and offers the end tags that
/// make it parse.
fn unclosed_at_end(source: &SourceText, error: &XmlError) -> Option<Diagnostic> {
    let name = error
        .message
        .strip_prefix("unclosed element `")?
        .strip_suffix('`')?;
    let text = source.text();
    let content_end = text.trim_end().len();
    let last = text.get(..content_end)?.char_indices().next_back()?.0;
    let primary = Span::new(error.span.source, last, content_end);
    let mut closers = vec![name.to_owned()];
    let mut insertion = None;
    for _ in 0..MAX_INSERTED_END_TAGS {
        // One end tag per line after the last content.
        let mut lines = String::new();
        for name in &closers {
            lines.push_str("\n</");
            lines.push_str(name);
            lines.push('>');
        }
        let candidate = format!(
            "{}{lines}{}",
            text.get(..content_end)?,
            text.get(content_end..)?
        );
        match reparses(source, candidate) {
            Ok(()) => {
                insertion = Some(lines);
                break;
            }
            Err(next) => match next
                .message
                .strip_prefix("unclosed element `")
                .and_then(|rest| rest.strip_suffix('`'))
            {
                Some(next) if next.len() <= MAX_REFERENCE_BYTES => closers.push(next.to_owned()),
                _ => break,
            },
        }
    }
    let listed = closers
        .iter()
        .map(|name| format!("`<{name}>`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut diagnostic = Diagnostic::error(
        xml_error_code(XmlErrorKind::Malformed),
        if closers.len() == 1 {
            format!("the document ends before `<{name}>` is closed")
        } else {
            format!("the document ends before {listed} are closed")
        },
        primary,
    )
    .with_primary_message("the document ends here");
    if source.slice(error.span).is_some_and(|tag| {
        tag.strip_prefix('<')
            .is_some_and(|rest| rest.starts_with(name))
    }) {
        diagnostic = diagnostic.with_secondary(
            Label::new(Span::new(
                error.span.source,
                error.span.start + 1,
                error.span.start + 1 + name.len(),
            ))
            .with_message(format!("`<{name}>` opens here and is never closed")),
        );
    }
    match insertion {
        Some(lines) => {
            let tags: String = lines.split('\n').collect();
            Some(
                diagnostic
                    .with_help(format!(
                        "add `{tags}` at the end of the document; if the document was cut off, restore the missing content first"
                    ))
                    .with_fix(Fix::replace(
                        format!("add `{tags}` at the end of the document"),
                        Applicability::MaybeIncorrect,
                        Span::new(error.span.source, content_end, content_end),
                        lines,
                    )),
            )
        }
        None => Some(diagnostic.with_help(format!(
            "close {listed} with matching end tags where their content ends"
        ))),
    }
}

/// One attribute of a start tag, as offsets into the tag text.
struct TagAttribute {
    name: (usize, usize),
    /// Offset of the opening quote, when the attribute has `=` and a quote.
    quote: Option<usize>,
}

/// Scans a start tag's attributes up to the first one without `=` (which
/// is returned last with `quote: None`) or the first value that does not
/// close before a `<`.
fn scan_attributes(tag: &str) -> Vec<TagAttribute> {
    let bytes = tag.as_bytes();
    let mut attributes = Vec::new();
    let skip = |mut index: usize| {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        index
    };
    let Some(mut index) = tag
        .get(1..)
        .and_then(|rest| {
            rest.find(|character: char| character.is_whitespace() || matches!(character, '>' | '/'))
        })
        .map(|offset| offset + 1)
    else {
        return attributes;
    };
    loop {
        index = skip(index);
        match bytes.get(index) {
            None | Some(b'>' | b'/' | b'<') => return attributes,
            Some(_) => {}
        }
        let start = index;
        while bytes.get(index).is_some_and(|byte| {
            !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'>' | b'/' | b'<')
        }) {
            index += 1;
        }
        let name = (start, index);
        index = skip(index);
        if bytes.get(index) != Some(&b'=') {
            attributes.push(TagAttribute { name, quote: None });
            return attributes;
        }
        index = skip(index + 1);
        let Some(&quote) = bytes.get(index).filter(|byte| matches!(byte, b'"' | b'\'')) else {
            return attributes;
        };
        attributes.push(TagAttribute {
            name,
            quote: Some(index),
        });
        let value = index + 1;
        match tag.get(value..).and_then(|rest| {
            rest.find(|character: char| character == char::from(quote) || character == '<')
        }) {
            Some(end) if bytes.get(value + end) == Some(&quote) => index = value + end + 1,
            _ => return attributes,
        }
    }
}

/// `<else if cond="…"/>` or `<parallel id="p" final>`: an attribute name
/// with no `=value` (HTML boolean attributes are not XML).
fn attribute_without_value(source: &SourceText, error: &XmlError) -> Option<Diagnostic> {
    if error.message != "an attribute name must be followed by `=` and a quoted value" {
        return None;
    }
    let tag = source.slice(error.span)?;
    let element = tag
        .get(1..)?
        .split(|character: char| character.is_whitespace() || matches!(character, '>' | '/'))
        .next()?;
    let bare = scan_attributes(tag)
        .pop()
        .filter(|attribute| attribute.quote.is_none())?;
    let attribute = tag.get(bare.name.0..bare.name.1)?;
    if attribute.is_empty() {
        return None;
    }
    let at = |offset: usize| error.span.start + offset;
    let name_span = Span::new(error.span.source, at(bare.name.0), at(bare.name.1));
    let code = xml_error_code(XmlErrorKind::Malformed);
    if element == "else" && attribute == "if" {
        let span = Span::new(error.span.source, at(1), at(bare.name.1));
        let fix = FixEdit {
            span,
            replacement: "elseif".to_owned(),
        };
        let mut diagnostic = Diagnostic::error(
            code,
            "`<else if …>` is not SCXML: a conditional branch is the single element `<elseif>`",
            span,
        )
        .with_primary_message("write `elseif` as one word")
        .with_help("write `<elseif cond=\"…\"/>`; `<else/>` takes no condition");
        if repair_progresses(source, std::slice::from_ref(&fix), error.span.end) {
            diagnostic = diagnostic.with_fix(Fix {
                message: "use `<elseif>`".to_owned(),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![fix],
            });
        }
        return Some(diagnostic);
    }
    let removal = FixEdit {
        span: Span::new(
            error.span.source,
            at(tag.get(..bare.name.0)?.trim_end().len()),
            at(bare.name.1),
        ),
        replacement: String::new(),
    };
    let help = if attribute == "final" && matches!(element, "state" | "parallel") {
        "SCXML has no `final` attribute: a final state is the element `<final id=\"…\"/>`; remove `final`, and write the state as `<final>` if it is meant to end its parent".to_owned()
    } else {
        format!("give `{attribute}` a quoted value (`{attribute}=\"…\"`), or remove it")
    };
    let mut diagnostic = Diagnostic::error(
        code,
        format!("attribute `{attribute}` on `<{element}>` has no value"),
        name_span,
    )
    .with_primary_message("an XML attribute needs `=` and a quoted value")
    .with_help(help);
    if repair_progresses(source, std::slice::from_ref(&removal), error.span.end) {
        diagnostic = diagnostic.with_fix(Fix {
            message: format!("remove `{attribute}`"),
            applicability: Applicability::MaybeIncorrect,
            edits: vec![removal],
        });
    }
    Some(diagnostic)
}

/// `id="a>`: an attribute value whose closing quote is missing, so the
/// value swallows the rest of the tag and the following markup. Labels the
/// opening quote and closes the value where the tag was meant to end.
fn unterminated_attribute_value(source: &SourceText, error: &XmlError) -> Option<Diagnostic> {
    if !(error.message.starts_with("attribute value is not closed")
        || error
            .message
            .starts_with("attribute value is missing its closing"))
    {
        return None;
    }
    let tag = source.slice(error.span)?;
    let attribute = scan_attributes(tag).pop()?;
    let quote_at = attribute.quote?;
    let quote = char::from(*tag.as_bytes().get(quote_at)?);
    let name = tag.get(attribute.name.0..attribute.name.1)?;
    let value = tag.get(quote_at + 1..)?;
    if value
        .find([quote, '<'])
        .is_some_and(|end| value.get(end..).is_some_and(|rest| rest.starts_with(quote)))
    {
        // Every scanned value closes; the parser's failure is elsewhere.
        return None;
    }
    // The value was meant to end at the first `>` (before `/>`) or line end.
    let end = value.find(['>', '\n', '<']).unwrap_or(value.len());
    let meant = value.get(..end)?;
    let meant = meant.strip_suffix('/').unwrap_or(meant).trim_end();
    let at = |offset: usize| error.span.start + offset;
    let quote_span = Span::new(error.span.source, at(quote_at), at(quote_at + 1));
    let insert = at(quote_at + 1 + meant.len());
    let mut diagnostic = Diagnostic::error(
        xml_error_code(XmlErrorKind::Malformed),
        format!("the value of `{name}` is never closed: `{quote}` is missing"),
        quote_span,
    )
    .with_primary_message(format!("this `{quote}` opens the value of `{name}`"))
    .with_help(format!(
        "close the value with `{quote}`: `{name}={quote}{meant}{quote}`"
    ));
    let fix = FixEdit {
        span: Span::new(error.span.source, insert, insert),
        replacement: quote.to_string(),
    };
    if repair_progresses(source, std::slice::from_ref(&fix), insert) {
        diagnostic = diagnostic.with_fix(Fix {
            message: format!("close the value of `{name}` with `{quote}`"),
            applicability: Applicability::MaybeIncorrect,
            edits: vec![fix],
        });
    }
    Some(diagnostic)
}

/// Removes the parser's internal byte-position prefix (`position 9: `); the
/// diagnostic's span already carries the exact location.
fn parser_message(message: &str) -> &str {
    message
        .strip_prefix("position ")
        .and_then(|rest| rest.split_once(": "))
        .filter(|(digits, _)| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .map_or(message, |(_, rest)| rest)
}

/// `expected `</open>`, but `</close>` was found`: labels the end-tag name
/// (primary) and the unclosed start tag (secondary), with a repair.
fn mismatched_end_tag(source: &SourceText, error: &XmlError) -> Option<Diagnostic> {
    let rest = error.message.split_once("expected `</")?.1;
    let (open, rest) = rest.split_once(">`, but `</")?;
    let (close, _) = rest.split_once(">` was found")?;
    let text = source.text();
    let end_tag = source.slice(error.span)?;
    let close_offset = end_tag.find(close).filter(|_| end_tag.starts_with("</"))?;
    let close_start = error.span.start + close_offset;
    let primary = Span::new(error.span.source, close_start, close_start + close.len());
    let opener = find_unclosed_opener(text, open, error.span.start);
    let mut diagnostic = Diagnostic::error(
        xml_error_code(XmlErrorKind::Malformed),
        format!("mismatched end tag: expected `</{open}>`, found `</{close}>`"),
        primary,
    )
    .with_primary_message(format!(
        "`</{close}>` closes an outer element while `<{open}>` is still open"
    ));
    let Some((name_start, tag_end)) = opener else {
        return Some(diagnostic.with_help(format!(
            "close `<{open}>` before `</{close}>`, or end its start tag with `/>`"
        )));
    };
    diagnostic = diagnostic.with_secondary(
        Label::new(Span::new(
            error.span.source,
            name_start,
            name_start + open.len(),
        ))
        .with_message(format!("unclosed `<{open}>` starts here")),
    );
    let between = text.get(tag_end..error.span.start).unwrap_or_default();
    let open_chars: Vec<char> = open.chars().take(MAX_TAG_TYPO_CHARS + 1).collect();
    let close_chars: Vec<char> = close.chars().take(MAX_TAG_TYPO_CHARS + 1).collect();
    let misspelled = open_chars.len() <= MAX_TAG_TYPO_CHARS
        && close_chars.len() <= MAX_TAG_TYPO_CHARS
        && crate::semantic::osa_distance(&open_chars, &close_chars) <= 2;
    if misspelled && !between.trim().is_empty() {
        // `</stat>` for `<state>`: the end tag is a misspelling, so renaming
        // it closes the right element; adding `</state>` would leave
        // `</stat>` unmatched.
        diagnostic = diagnostic
            .with_help(format!("did you mean `</{open}>`?"))
            .with_fix(Fix::replace(
                format!("rename the end tag to `</{open}>`"),
                Applicability::MaybeIncorrect,
                primary,
                open,
            ));
    } else if between.trim().is_empty() {
        diagnostic = diagnostic
            .with_help(format!(
                "`<{open}>` has no content, so end its start tag with `/>`"
            ))
            .with_fix(Fix::replace(
                format!("self-close `<{open}>`"),
                Applicability::MaybeIncorrect,
                Span::new(error.span.source, tag_end - 1, tag_end),
                "/>",
            ));
    } else {
        diagnostic = diagnostic
            .with_help(format!("add `</{open}>` where the `<{open}>` content ends"))
            .with_fix(Fix::replace(
                format!("close `<{open}>` before `</{close}>`"),
                Applicability::MaybeIncorrect,
                Span::new(error.span.source, error.span.start, error.span.start),
                format!("</{open}>"),
            ));
    }
    Some(diagnostic)
}

/// Finds the nearest start tag `<open …>` (not self-closed) before `before`,
/// returning the name's start offset and the offset just past its `>`.
fn find_unclosed_opener(text: &str, open: &str, before: usize) -> Option<(usize, usize)> {
    let window_start = before.saturating_sub(MAX_OPENER_SEARCH_BYTES);
    let window = text.get(window_start..before)?;
    let needle = format!("<{open}");
    let mut search_end = window.len();
    while let Some(relative) = window.get(..search_end)?.rfind(&needle) {
        let name_start = window_start + relative + 1;
        let after_name = text.get(name_start + open.len()..)?;
        let boundary = after_name.chars().next()?;
        if boundary.is_whitespace() || boundary == '>' {
            let close = after_name.find('>')?;
            let tag_end = name_start + open.len() + close + 1;
            let self_closed = text.get(..tag_end - 1)?.ends_with('/');
            if !self_closed && tag_end <= before {
                return Some((name_start, tag_end));
            }
        }
        search_end = relative;
    }
    None
}

/// `attribute value must be enclosed in quotes`: labels exactly the unquoted
/// value and quotes it.
fn unquoted_attribute(source: &SourceText, error: &XmlError) -> Option<Diagnostic> {
    if !error
        .message
        .contains("attribute value must be enclosed in")
    {
        return None;
    }
    let tag = source.slice(error.span)?;
    let mut offset = 0;
    while let Some(relative) = tag.get(offset..)?.find('=') {
        let equals = offset + relative;
        let after = tag.get(equals + 1..)?;
        let leading = after.len() - after.trim_start().len();
        let value_start = equals + 1 + leading;
        let first = tag.get(value_start..)?.chars().next()?;
        if first != '"' && first != '\'' {
            let length = tag
                .get(value_start..)?
                .find(|character: char| {
                    character.is_whitespace() || character == '>' || character == '/'
                })
                .unwrap_or(tag.len() - value_start);
            let value = tag.get(value_start..value_start + length)?;
            if value.is_empty() || value.contains('"') {
                return None;
            }
            let span = Span::new(
                error.span.source,
                error.span.start + value_start,
                error.span.start + value_start + length,
            );
            let quoted = format!("\"{value}\"");
            return Some(
                Diagnostic::error(
                    xml_error_code(XmlErrorKind::Malformed),
                    "attribute value must be enclosed in quotes",
                    span,
                )
                .with_primary_message("unquoted attribute value")
                .with_help(format!("quote the value: {quoted}"))
                .with_fix(Fix::replace(
                    "quote the attribute value",
                    Applicability::MachineApplicable,
                    span,
                    quoted,
                )),
            );
        }
        offset = equals + 1;
    }
    None
}

/// Names a character for help text: visible characters in backticks,
/// whitespace and invisible characters by code point so they are never
/// rendered as an apparently empty literal.
fn describe_character(character: char) -> String {
    let invisible = character.is_whitespace()
        || character.is_control()
        || matches!(character, '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}');
    if invisible {
        format!("U+{:04X}", u32::from(character))
    } else {
        format!(
            "the character `{character}` (U+{:04X})",
            u32::from(character)
        )
    }
}

fn malformed_help(source: &SourceText, span: Span) -> String {
    let near = source
        .text()
        .get(span.start.saturating_sub(MAX_REFERENCE_BYTES)..span.end)
        .unwrap_or_default();
    if near.contains("&&") || near.ends_with('&') {
        "escape a literal `&` as `&amp;` (for example `cond=\"a &amp;&amp; b\"`), or put the expression in a CDATA section inside `<script>`".to_owned()
    } else {
        "fix the XML syntax at the marked position: check for unclosed tags, mismatched end tags, unquoted attribute values, and unescaped `<` or `&`".to_owned()
    }
}

/// Recovers the exact `&…;` bytes around a reported reference span.
fn reference_span(source: &SourceText, span: Span) -> (Span, Option<String>) {
    let text = source.text();
    if let Some(slice) = source.slice(span)
        && let Some(inner) = slice
            .strip_prefix('&')
            .and_then(|rest| rest.strip_suffix(';'))
    {
        return (span, Some(inner.to_owned()));
    }
    let window_start = span.start.saturating_sub(MAX_REFERENCE_BYTES);
    let Some(before) = text.get(window_start..=span.start.min(text.len().saturating_sub(1))) else {
        return (span, None);
    };
    let Some(ampersand) = before.rfind('&').map(|offset| window_start + offset) else {
        return (span, None);
    };
    let Some(after) = text.get(ampersand..(ampersand + MAX_REFERENCE_BYTES).min(text.len())) else {
        return (span, None);
    };
    let Some(semicolon) = after.find(';') else {
        return (span, None);
    };
    let exact = Span::new(span.source, ampersand, ampersand + semicolon + 1);
    let inner = after.get(1..semicolon).map(str::to_owned);
    (exact, inner)
}

fn undefined_entity(source: &SourceText, error: &XmlError) -> Diagnostic {
    let (span, name) = reference_span(source, error.span);
    let name = name.unwrap_or_default();
    let mut diagnostic = Diagnostic::error(
        xml_error_code(XmlErrorKind::UndefinedEntityReference),
        format!("undefined entity reference `&{name};`"),
        span,
    )
    .with_primary_message("not a predefined or declared entity")
    .with_note("XML predefines only `&lt;`, `&gt;`, `&amp;`, `&apos;`, and `&quot;`; any other entity must be declared in the document's internal DTD subset")
    .with_spec_reference("XML 1.0 (Fifth Edition) §4.6 Predefined Entities");
    if let Some((_, code_point)) = HTML_ENTITIES.iter().find(|(entity, _)| *entity == name)
        && let Some(character) = char::from_u32(*code_point)
    {
        diagnostic = diagnostic
            .with_help(format!(
                "`&{name};` is an HTML entity; write {} as the numeric reference `&#{code_point};`",
                describe_character(character)
            ))
            .with_fix(Fix::replace(
                format!("use the numeric character reference `&#{code_point};`"),
                Applicability::MachineApplicable,
                span,
                format!("&#{code_point};"),
            ));
    } else {
        diagnostic = diagnostic
            .with_help("if a literal ampersand was intended, escape it as `&amp;`; otherwise use a numeric character reference such as `&#169;`")
            .with_fix(Fix::replace(
                "escape the ampersand",
                Applicability::MaybeIncorrect,
                Span::new(span.source, span.start, span.start.saturating_add(1)),
                "&amp;",
            ));
    }
    diagnostic
}

fn invalid_character_reference(source: &SourceText, error: &XmlError) -> Diagnostic {
    let (span, reference) = reference_span(source, error.span);
    let reference = reference.map_or_else(String::new, |inner| format!("&{inner};"));
    let reason = error
        .message
        .split_once(": ")
        .map_or(error.message.as_str(), |(_, reason)| reason);
    Diagnostic::error(
        xml_error_code(XmlErrorKind::InvalidCharacterReference),
        format!("invalid character reference `{reference}`"),
        span,
    )
    .with_primary_message(reason.to_owned())
    .with_note("XML 1.0 `Char` excludes NUL, C0 controls other than tab, newline, and carriage return, the surrogates U+D800–U+DFFF, and U+FFFE/U+FFFF")
    .with_help("use `&#NNNN;` (decimal) or `&#xHHHH;` (lowercase `x`, hexadecimal) naming a legal XML character, or remove the reference")
    .with_fix(Fix::replace(
        "remove the invalid character reference",
        Applicability::MaybeIncorrect,
        span,
        "",
    ))
    .with_spec_reference("XML 1.0 (Fifth Edition) §4.1 Character and Entity References")
}

#[cfg(test)]
mod tests {
    use scxml_syntax::{SourceId, parse_xml};

    use super::*;

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);

    fn diagnose(xml: &str) -> (SourceText, Diagnostic) {
        let source = SourceText::new(SOURCE, "xml.scxml", xml);
        let error = parse_xml(source.clone()).expect_err("fixture must be rejected");
        let diagnostic = xml_error_diagnostic(&source, &error);
        (source, diagnostic)
    }

    #[test]
    fn every_xml_error_kind_has_a_distinct_stable_code() {
        let codes = [
            XmlErrorKind::Malformed,
            XmlErrorKind::ResourceLimit,
            XmlErrorKind::UndefinedEntityReference,
            XmlErrorKind::InvalidCharacterReference,
            XmlErrorKind::InvalidLimits,
        ]
        .map(xml_error_code);
        let unique: std::collections::BTreeSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
    }

    #[test]
    fn html_entity_is_labeled_exactly_with_a_machine_applicable_fix() {
        let (source, diagnostic) = diagnose("<s a=\"x&nbsp;y\"/>");
        assert_eq!(diagnostic.code, "XML003");
        assert_eq!(source.slice(diagnostic.primary.span), Some("&nbsp;"));
        assert_eq!(
            diagnostic.fixes[0].applicability,
            Applicability::MachineApplicable
        );
        assert_eq!(diagnostic.fixes[0].edits[0].replacement, "&#160;");
    }

    #[test]
    fn unknown_entity_offers_maybe_incorrect_ampersand_escape() {
        let (source, diagnostic) = diagnose("<s>&foo;</s>");
        assert_eq!(diagnostic.code, "XML003");
        assert_eq!(source.slice(diagnostic.primary.span), Some("&foo;"));
        assert_eq!(
            diagnostic.fixes[0].applicability,
            Applicability::MaybeIncorrect
        );
        assert_eq!(source.slice(diagnostic.fixes[0].edits[0].span), Some("&"));
    }

    /// Applies a fix's edits to `source`.
    fn apply(source: &SourceText, fix: &Fix) -> String {
        let mut text = source.text().to_owned();
        let mut edits: Vec<&FixEdit> = fix.edits.iter().collect();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
        for edit in edits {
            text.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        }
        text
    }

    fn parses(text: String) -> bool {
        parse_xml(SourceText::new(SOURCE, "fixed.scxml", text)).is_ok()
    }

    #[test]
    fn a_truncated_document_labels_its_end_and_inserts_every_missing_end_tag() {
        let (source, diagnostic) = diagnose("<scxml>\n  <state id=\"a\">\n    <final id=\"b\"/>\n");
        assert_eq!(
            diagnostic.message,
            "the document ends before `<state>`, `<scxml>` are closed"
        );
        assert_eq!(source.slice(diagnostic.primary.span), Some(">"));
        assert_eq!(source.slice(diagnostic.secondary[0].span), Some("state"));
        let fix = &diagnostic.fixes[0];
        assert_eq!(fix.applicability, Applicability::MaybeIncorrect);
        assert_eq!(fix.edits[0].replacement, "\n</state>\n</scxml>");
        assert!(parses(apply(&source, fix)));
    }

    #[test]
    fn an_attribute_without_a_value_labels_its_name() {
        let (source, diagnostic) = diagnose("<scxml><parallel id=\"p\" final></parallel></scxml>");
        assert_eq!(source.slice(diagnostic.primary.span), Some("final"));
        assert!(diagnostic.message.contains("`final`"), "{diagnostic:?}");
        assert_eq!(
            apply(&source, &diagnostic.fixes[0]),
            "<scxml><parallel id=\"p\"></parallel></scxml>"
        );

        let (source, diagnostic) = diagnose("<if cond=\"a\"><else if cond=\"b\"/></if>");
        assert_eq!(source.slice(diagnostic.primary.span), Some("else if"));
        assert_eq!(
            apply(&source, &diagnostic.fixes[0]),
            "<if cond=\"a\"><elseif cond=\"b\"/></if>"
        );
    }

    #[test]
    fn an_unterminated_attribute_value_labels_its_opening_quote() {
        let (source, diagnostic) =
            diagnose("<scxml>\n<state id=\"a>\n<final id=\"b\"/>\n</state>\n</scxml>\n");
        assert_eq!(source.slice(diagnostic.primary.span), Some("\""));
        assert_eq!(diagnostic.primary.span.start, 18);
        let fixed = apply(&source, &diagnostic.fixes[0]);
        assert!(fixed.starts_with("<scxml>\n<state id=\"a\">"), "{fixed}");
        assert!(parses(fixed));
    }

    #[test]
    fn chat_prose_with_a_tag_is_removed_in_one_converging_fix() {
        let xml = "Here is the <scxml> machine you asked for:\n\n```xml\n<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"/></scxml>\n```\n";
        let (source, diagnostic) = diagnose(xml);
        assert_eq!(
            source.slice(diagnostic.primary.span),
            Some("Here is the <scxml> machine you asked for:")
        );
        let fix = &diagnostic.fixes[0];
        assert_eq!(fix.applicability, Applicability::MachineApplicable);
        assert_eq!(
            apply(&source, fix),
            "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"/></scxml>\n"
        );
    }

    #[test]
    fn invalid_utf8_is_located_in_the_replacement_decoded_source() {
        let bytes = b"<s>\n caf\xE9</s>".to_vec();
        let error = std::str::from_utf8(&bytes).expect_err("fixture is not UTF-8");
        let source = SourceText::new(
            SOURCE,
            "latin1.scxml",
            String::from_utf8_lossy(&bytes).into_owned(),
        );
        let diagnostic = invalid_utf8_diagnostic(&source, &bytes, &error);
        assert_eq!(diagnostic.code, "XML001");
        assert_eq!(source.slice(diagnostic.primary.span), Some("\u{FFFD}"));
        assert_eq!(source.line_column(diagnostic.primary.span.start).line, 2);
        assert!(
            diagnostic
                .primary
                .message
                .as_deref()
                .is_some_and(|label| label.contains("0xE9")),
            "{diagnostic:?}"
        );
    }

    #[test]
    fn invalid_character_reference_labels_the_reference() {
        let (source, diagnostic) = diagnose("<s>&#0;</s>");
        assert_eq!(diagnostic.code, "XML004");
        assert_eq!(source.slice(diagnostic.primary.span), Some("&#0;"));
    }
}
