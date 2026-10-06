//! Stable, source-oriented validator diagnostics.

use scxml_reporting::{Applicability, ArtifactEnvelope, ByteSpan, DiagnosticCategory, TextEdit};
use scxml_syntax::{SourceId, SourceText, Span};

/// Diagnostic severity ordered from least to most severe.
pub use scxml_reporting::Severity;

/// One source annotation attached to a diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Label {
    /// Labeled source range.
    pub span: Span,
    /// Optional explanation of the range.
    pub message: Option<String>,
    /// Whether the report-wide precision pass keeps [`Self::span`] as given
    /// instead of narrowing a start-tag span to the element name (see
    /// [`Self::whole_element`]).
    exact: bool,
}

impl Label {
    /// Creates an unlabeled source range.
    #[must_use]
    pub const fn new(span: Span) -> Self {
        Self {
            span,
            message: None,
            exact: false,
        }
    }

    /// Creates an unlabeled range that covers a whole element. The
    /// precision pass narrows a label equal to a start tag to the element
    /// name; a self-closing element's span equals its start tag, so a label
    /// meant for the whole element is kept as given.
    #[must_use]
    pub const fn whole_element(span: Span) -> Self {
        Self {
            span,
            message: None,
            exact: true,
        }
    }

    /// Whether the precision pass keeps this label's span as given.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        self.exact
    }

    /// Adds a message to this label.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

/// One source replacement edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixEdit {
    /// Byte range replaced by [`Self::replacement`].
    pub span: Span,
    /// Exact replacement text.
    pub replacement: String,
}

/// One clippy-style suggested repair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fix {
    /// Human-readable repair summary.
    pub message: String,
    /// Confidence in applying the edit automatically.
    pub applicability: Applicability,
    /// Sorted non-overlapping source edits.
    pub edits: Vec<FixEdit>,
}

impl Fix {
    /// Creates a single-edit suggested repair.
    #[must_use]
    pub fn replace(
        message: impl Into<String>,
        applicability: Applicability,
        span: Span,
        replacement: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            applicability,
            edits: vec![FixEdit {
                span,
                replacement: replacement.into(),
            }],
        }
    }
}

/// Rustc-style, machine-friendly validation diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// Severity class.
    pub severity: Severity,
    /// Stable validator or backend code.
    pub code: String,
    /// Stable diagnostic category.
    pub category: DiagnosticCategory,
    /// Primary summary.
    pub message: String,
    /// Primary source label.
    pub primary: Label,
    /// Related source labels.
    pub secondary: Vec<Label>,
    /// Supplemental notes.
    pub notes: Vec<String>,
    /// Actionable help text.
    pub help: Option<String>,
    /// Specification anchors or citations.
    pub spec_references: Vec<String>,
    /// Structured suggested repairs.
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    /// Creates an error diagnostic with a category inferred from its stable code.
    #[must_use]
    pub fn error(code: impl Into<String>, message: impl Into<String>, span: Span) -> Self {
        let code = code.into();

        Self {
            severity: Severity::Error,
            category: category_for_code(&code),
            code,
            message: message.into(),
            primary: Label::new(span),
            secondary: Vec::new(),
            notes: Vec::new(),
            help: None,
            spec_references: Vec::new(),
            fixes: Vec::new(),
        }
    }

    /// Creates a warning diagnostic.
    #[must_use]
    pub fn warning(code: impl Into<String>, message: impl Into<String>, span: Span) -> Self {
        let mut diagnostic = Self::error(code, message, span);
        diagnostic.severity = Severity::Warning;
        diagnostic.category = DiagnosticCategory::Lint;
        diagnostic
    }

    /// Sets the stable category explicitly.
    #[must_use]
    pub const fn with_category(mut self, category: DiagnosticCategory) -> Self {
        self.category = category;
        self
    }

    /// Adds a message to the primary label.
    #[must_use]
    pub fn with_primary_message(mut self, message: impl Into<String>) -> Self {
        self.primary.message = Some(message.into());
        self
    }

    /// Adds a secondary source label.
    #[must_use]
    pub fn with_secondary(mut self, label: Label) -> Self {
        self.secondary.push(label);
        self
    }

    /// Adds a note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Adds actionable help.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Adds a specification reference.
    #[must_use]
    pub fn with_spec_reference(mut self, reference: impl Into<String>) -> Self {
        self.spec_references.push(reference.into());
        self
    }

    /// Adds a structured repair suggestion.
    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fixes.push(fix);
        self
    }
}

/// Deterministically ordered diagnostic collection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Report {
    diagnostics: Vec<Diagnostic>,
}

impl Report {
    /// Creates an empty report.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
        }
    }

    /// Adds one diagnostic.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Appends another report.
    pub fn extend(&mut self, other: Self) {
        self.diagnostics.extend(other.diagnostics);
    }

    /// Sorts diagnostics and their related data deterministically.
    pub fn sort(&mut self) {
        for diagnostic in &mut self.diagnostics {
            diagnostic.secondary.sort_by(|left, right| {
                (left.span, left.message.as_deref()).cmp(&(right.span, right.message.as_deref()))
            });
            diagnostic.notes.sort();
            diagnostic.notes.dedup();
            diagnostic.spec_references.sort();
            diagnostic.spec_references.dedup();
            diagnostic.fixes.sort_by(|left, right| {
                (
                    left.edits.first().map(|edit| edit.span),
                    left.applicability,
                    left.message.as_str(),
                )
                    .cmp(&(
                        right.edits.first().map(|edit| edit.span),
                        right.applicability,
                        right.message.as_str(),
                    ))
            });
        }
        self.diagnostics.sort_by(|left, right| {
            (
                left.primary.span.source,
                left.primary.span.start,
                left.primary.span.end,
                left.severity,
                left.code.as_str(),
                left.message.as_str(),
            )
                .cmp(&(
                    right.primary.span.source,
                    right.primary.span.start,
                    right.primary.span.end,
                    right.severity,
                    right.code.as_str(),
                    right.message.as_str(),
                ))
        });
    }

    /// Applies `edit` to every diagnostic in place (crate-internal precision
    /// passes only; callers must re-sort afterwards).
    pub(crate) fn map_diagnostics(&mut self, mut edit: impl FnMut(&mut Diagnostic)) {
        for diagnostic in &mut self.diagnostics {
            edit(diagnostic);
        }
    }

    /// Keeps only diagnostics for which `keep` returns true (crate-internal
    /// cascade suppression; order is preserved).
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&Diagnostic) -> bool) {
        self.diagnostics.retain(|diagnostic| keep(diagnostic));
    }

    /// Returns whether any error is present.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// Returns diagnostics in deterministic order after [`Self::sort`].
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Creates and validates the canonical validator artifact for one source.
    ///
    /// # Errors
    ///
    /// Returns an artifact invariant failure if an internal diagnostic span is
    /// inconsistent with the preserved source.
    pub fn canonical_artifact(
        &self,
        source: &SourceText,
    ) -> Result<ArtifactEnvelope, scxml_reporting::ArtifactError> {
        for diagnostic in &self.diagnostics {
            let spans = core::iter::once(diagnostic.primary.span)
                .chain(diagnostic.secondary.iter().map(|label| label.span))
                .chain(
                    diagnostic
                        .fixes
                        .iter()
                        .flat_map(|fix| fix.edits.iter().map(|edit| edit.span)),
                );
            if let Some(foreign) = spans.into_iter().find(|item| item.source != source.id()) {
                return Err(scxml_reporting::ArtifactError::InvalidSpan(ByteSpan {
                    start: foreign.start as u64,
                    end: foreign.end as u64,
                }));
            }
        }
        let diagnostics = self
            .diagnostics
            .iter()
            .map(to_canonical_diagnostic)
            .collect();
        ArtifactEnvelope::validator(source.name(), source.text(), diagnostics)
    }
}

fn category_for_code(code: &str) -> DiagnosticCategory {
    if code.starts_with("XML") {
        DiagnosticCategory::Syntax
    } else if code.starts_with("XSD") {
        DiagnosticCategory::Schema
    } else if code.starts_with("PKG") {
        DiagnosticCategory::Package
    } else if code.starts_with("SCXML") {
        DiagnosticCategory::Semantic
    } else {
        DiagnosticCategory::Internal
    }
}

fn to_canonical_diagnostic(diagnostic: &Diagnostic) -> scxml_reporting::Diagnostic {
    scxml_reporting::Diagnostic {
        severity: diagnostic.severity,
        code: diagnostic.code.clone(),
        category: diagnostic.category,
        message: diagnostic.message.clone(),
        primary: to_canonical_label(&diagnostic.primary),
        secondary: diagnostic
            .secondary
            .iter()
            .map(to_canonical_label)
            .collect(),
        notes: diagnostic.notes.clone(),
        help: diagnostic.help.clone(),
        spec_references: diagnostic.spec_references.clone(),
        fixes: diagnostic
            .fixes
            .iter()
            .map(|fix| scxml_reporting::Fix {
                message: fix.message.clone(),
                applicability: fix.applicability,
                edits: fix
                    .edits
                    .iter()
                    .map(|edit| TextEdit {
                        span: ByteSpan {
                            start: edit.span.start as u64,
                            end: edit.span.end as u64,
                        },
                        replacement: edit.replacement.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn to_canonical_label(label: &Label) -> scxml_reporting::Label {
    scxml_reporting::Label {
        span: ByteSpan {
            start: label.span.start as u64,
            end: label.span.end as u64,
        },
        message: label.message.clone(),
    }
}

/// Maps a one-based line/column from a backend to a source byte span.
#[must_use]
pub fn line_column_span(
    source: &SourceText,
    source_id: SourceId,
    line: usize,
    column: usize,
) -> Span {
    let Some(line_span) = source.line_span(line) else {
        return Span::new(source_id, 0, 0);
    };
    let Some(text) = source.slice(line_span) else {
        return Span::new(source_id, line_span.start, line_span.start);
    };
    let byte_in_line = text
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(text.len(), |(offset, _)| offset);
    let start = line_span.start + byte_in_line;
    let end = source.text()[start..]
        .chars()
        .next()
        .map_or(start, |character| start + character.len_utf8());

    Span::new(source_id, start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);
    /// The source id of the documents these tests parse.
    const FOREIGN_SOURCE: SourceId = SourceId::new(41);

    #[derive(Clone, Copy)]
    enum ForeignSpanLocation {
        Primary,
        Secondary,
        Fix,
    }

    fn report_with_foreign_span(location: ForeignSpanLocation, source: &SourceText) -> Report {
        let matching = Span::new(source.id(), 0, 1);
        let foreign = Span::new(SourceId::new(source.id().get() + 1), 0, 1);
        let primary = if matches!(location, ForeignSpanLocation::Primary) {
            foreign
        } else {
            matching
        };
        let secondary = if matches!(location, ForeignSpanLocation::Secondary) {
            foreign
        } else {
            matching
        };
        let fix = if matches!(location, ForeignSpanLocation::Fix) {
            foreign
        } else {
            matching
        };
        let mut report = Report::new();
        report.push(
            Diagnostic::error("SCXML999", "hostile source identity", primary)
                .with_secondary(Label::new(secondary))
                .with_fix(Fix::replace(
                    "hostile edit source identity",
                    Applicability::MachineApplicable,
                    fix,
                    "x",
                )),
        );
        report.sort();
        report
    }

    #[test]
    fn canonical_conversion_preserves_unicode_byte_spans_and_fixes() {
        let source = SourceText::new(SOURCE, "demo.scxml", "<state id=\"café\"/>");
        let span = Span::new(source.id(), 11, 16);
        let mut report = Report::new();
        report.push(
            Diagnostic::error("SCXML999", "demonstration", span)
                .with_primary_message("invalid identifier")
                .with_fix(Fix::replace(
                    "replace it",
                    Applicability::MaybeIncorrect,
                    span,
                    "ready",
                )),
        );
        report.sort();
        let artifact = report
            .canonical_artifact(&source)
            .expect("well-formed diagnostic fixture must produce a canonical artifact");
        let payload = artifact.validator_payload().expect("validator payload");
        assert_eq!(payload.diagnostics[0].primary.span.start, 11);
        assert_eq!(payload.diagnostics[0].primary.span.end, 16);
        assert_eq!(
            payload.diagnostics[0].fixes[0].edits[0].replacement,
            "ready"
        );
    }

    #[test]
    fn canonical_conversion_transactionally_rejects_every_foreign_source_span() {
        use scxml_syntax::telemetry::{Recorder, install_recorder};

        let source = SourceText::new(FOREIGN_SOURCE, "demo.scxml", "x");
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());

        for location in [
            ForeignSpanLocation::Primary,
            ForeignSpanLocation::Secondary,
            ForeignSpanLocation::Fix,
        ] {
            let report = report_with_foreign_span(location, &source);
            let original = report.clone();

            assert!(matches!(
                report.canonical_artifact(&source),
                Err(scxml_reporting::ArtifactError::InvalidSpan(ByteSpan {
                    start: 0,
                    end: 1
                }))
            ));
            assert_eq!(report, original, "rejection must not mutate the report");
        }

        // Canonical conversion is a conversion inside the caller's boundary
        // span; rejecting a foreign span records no telemetry of its own.
        assert!(recorder.observations().is_empty());
    }
}
