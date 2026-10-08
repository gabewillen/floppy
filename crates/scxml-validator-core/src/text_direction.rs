//! `LINT016`: Unicode bidirectional control characters in the source.
//!
//! # Contract
//!
//! * `LINT016` (warning, lint): the document contains an explicit
//!   bidirectional embedding, override, or isolate control (U+202A-U+202E,
//!   U+2066-U+2069), the set rustc's `text_direction_codepoint_in_literal`
//!   lint rejects. These characters reorder how the surrounding text is
//!   displayed, so an editor or review tool can show a condition, target, or
//!   script differently from what the machine executes (Trojan Source,
//!   CVE-2021-42574).
//! * One finding per source line: the first control is the primary label and
//!   every other one on the line a secondary label. At most
//!   [`MAX_FINDINGS`] lines are reported.
//! * The repair writes each control as a hexadecimal character reference
//!   (`&#x202E;`), which XML decodes to the same character inside an
//!   attribute value or ordinary text, so the document means exactly the
//!   same while the control stays visible: the fix is machine-applicable when
//!   every control on the line is in such a position. A control inside a
//!   comment, CDATA section, or processing instruction, where references are
//!   not decoded, is removed instead, and the fix is then maybe-incorrect.
//! * Pure and bounded by the validated source text; it records no telemetry
//!   of its own.

use scxml_syntax::{Document, Element, Node, Span};

use crate::semantic::Rule;
use crate::{Applicability, Diagnostic, DiagnosticCategory, Fix, FixEdit, Label, Report};

/// Maximum `LINT016` findings (source lines) per document.
const MAX_FINDINGS: usize = 64;

const SPEC_BIDI: &str = "Unicode Standard Annex #9 §2 Directional Formatting Characters";

/// Unicode name of one explicit directional formatting character, or `None`
/// for any other character.
const fn control_name(character: char) -> Option<&'static str> {
    Some(match character {
        '\u{202a}' => "LEFT-TO-RIGHT EMBEDDING",
        '\u{202b}' => "RIGHT-TO-LEFT EMBEDDING",
        '\u{202c}' => "POP DIRECTIONAL FORMATTING",
        '\u{202d}' => "LEFT-TO-RIGHT OVERRIDE",
        '\u{202e}' => "RIGHT-TO-LEFT OVERRIDE",
        '\u{2066}' => "LEFT-TO-RIGHT ISOLATE",
        '\u{2067}' => "RIGHT-TO-LEFT ISOLATE",
        '\u{2068}' => "FIRST STRONG ISOLATE",
        '\u{2069}' => "POP DIRECTIONAL ISOLATE",
        _ => return None,
    })
}

/// `LINT016`: bidirectional control characters in the source text.
pub(crate) struct TextDirectionRule;

impl Rule for TextDirectionRule {
    fn code(&self) -> &'static str {
        "LINT016"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        let text = document.source.text();
        if !text
            .chars()
            .any(|character| control_name(character).is_some())
        {
            return;
        }
        let decoded = decoded_regions(document);
        let source = document.root.span.source;
        let mut findings = 0_usize;
        let mut line_start = 0_usize;
        for line in text.split_inclusive('\n') {
            let controls: Vec<(usize, char)> = line
                .char_indices()
                .filter(|(_, character)| control_name(*character).is_some())
                .map(|(offset, character)| (line_start + offset, character))
                .collect();
            line_start += line.len();
            let Some(&(first, first_character)) = controls.first() else {
                continue;
            };
            if findings == MAX_FINDINGS {
                return;
            }
            findings += 1;
            let span_of = |offset: usize, character: char| {
                Span::new(source, offset, offset + character.len_utf8())
            };
            let label = |character: char| {
                format!(
                    "U+{:04X} {}",
                    u32::from(character),
                    control_name(character).unwrap_or_default()
                )
            };
            let mut all_decoded = true;
            let edits: Vec<FixEdit> = controls
                .iter()
                .map(|&(offset, character)| {
                    let in_decoded = decoded.iter().any(|region| {
                        region.start <= offset && offset + character.len_utf8() <= region.end
                    });
                    all_decoded &= in_decoded;
                    FixEdit {
                        span: span_of(offset, character),
                        replacement: if in_decoded {
                            format!("&#x{:X};", u32::from(character))
                        } else {
                            String::new()
                        },
                    }
                })
                .collect();
            let count = controls.len();
            let any_decoded = edits.iter().any(|edit| !edit.replacement.is_empty());
            let mut diagnostic = Diagnostic::warning(
                self.code(),
                if count == 1 {
                    "a Unicode bidirectional control character changes how this line is displayed"
                        .to_owned()
                } else {
                    format!(
                        "{count} Unicode bidirectional control characters change how this line is displayed"
                    )
                },
                span_of(first, first_character),
            )
            .with_category(DiagnosticCategory::Lint)
            .with_primary_message(label(first_character));
            for &(offset, character) in controls.iter().skip(1) {
                diagnostic = diagnostic.with_secondary(
                    Label::new(span_of(offset, character)).with_message(label(character)),
                );
            }
            report.push(
                diagnostic
                    .with_note("an explicit embedding, override, or isolate reorders how the text around it is displayed, so an editor or review can show a condition, ID, or script differently from what the machine executes (Trojan Source, CVE-2021-42574)")
                    .with_help("if the character is intended, write it as a character reference such as `&#x202E;`, which XML decodes to the same character and keeps visible; otherwise remove it")
                    .with_fix(Fix {
                        message: match (count, all_decoded, any_decoded) {
                            (1, true, _) => "write the control character as a character reference",
                            (_, true, _) => "write the control characters as character references",
                            (1, false, _) => "remove the control character (a comment or CDATA section cannot escape it)",
                            (_, false, false) => "remove the control characters (a comment or CDATA section cannot escape them)",
                            (_, false, true) => "write the control characters as character references, and remove those a comment or CDATA section cannot escape",
                        }
                        .to_owned(),
                        applicability: if all_decoded {
                            Applicability::MachineApplicable
                        } else {
                            Applicability::MaybeIncorrect
                        },
                        edits,
                    })
                    .with_spec_reference(SPEC_BIDI),
            );
        }
    }
}

/// Source ranges where XML decodes character references to the same
/// character: attribute and namespace-declaration values, and text runs
/// that contain no CDATA section.
fn decoded_regions(document: &Document) -> Vec<Span> {
    let text = document.source.text();
    let mut regions = Vec::new();
    let mut pending: Vec<&Element> = vec![&document.root];
    while let Some(element) = pending.pop() {
        regions.extend(
            element
                .attributes
                .iter()
                .map(|attribute| attribute.value_span),
        );
        regions.extend(
            element
                .namespaces
                .iter()
                .map(|namespace| namespace.value_span),
        );
        for child in &element.children {
            match child {
                Node::Element(child) => pending.push(child),
                Node::Text(run) => {
                    let lexical = text.get(run.span.start..run.span.end).unwrap_or_default();
                    if !lexical.contains("<![CDATA[") && !lexical.contains("<!--") {
                        regions.push(run.span);
                    }
                }
            }
        }
    }
    regions
}
