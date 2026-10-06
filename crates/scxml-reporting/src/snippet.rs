//! Rustc-style source snippets, grouped labels, and inline suggestions.
//!
//! The renderer consumes only validated canonical diagnostics. It never
//! recomputes producer decisions: labels, notes, help, specification
//! references, and structured fixes are projected exactly as stored. Every
//! untrusted string is escaped through [`TerminalText`] or
//! [`TerminalSourceLine`], so only renderer-owned formatting emits newlines or
//! terminal control bytes.

use std::collections::BTreeMap;

use super::{
    ArtifactError, ByteSpan, ColorChoice, Counts, Diagnostic, Fix, Label, RenderBuffer,
    SourceIndex, TerminalSourceLine, TerminalText, append_format, display_source_name,
    display_width, paint, severity_color,
};

/// Maximum source lines shown for one suggestion before it is summarized.
///
/// Suggestions are bounded independently from [`super::MAX_RENDERED_BYTES`] so
/// a single large replacement cannot dominate the report.
const MAX_SUGGESTION_LINES: usize = 12;
/// Maximum separately rendered blocks of one suggestion; later blocks are
/// counted in an explicit elision line.
const MAX_SUGGESTION_BLOCKS: usize = 32;

/// Source lines wider than this many display columns are shown as a window
/// around their labels, elided with `...` on either side (as rustc does).
const MAX_SOURCE_LINE_COLUMNS: usize = 120;
/// Display columns of context kept on each side of a long line's labels.
const LONG_LINE_CONTEXT_COLUMNS: usize = 24;
/// Marker drawn where a long source line is elided.
const ELISION: &str = "...";

/// Renders complete source diagnostics followed by the aggregate summary.
pub(super) fn render_source_diagnostics(
    output: &mut RenderBuffer,
    source_name: &str,
    source_text: &str,
    diagnostics: &[Diagnostic],
    counts: Counts,
    color: ColorChoice,
) -> Result<(), ArtifactError> {
    let source = SourceIndex::new(source_text);
    let colored = color == ColorChoice::Always;
    for diagnostic in diagnostics {
        render_diagnostic(output, &source, source_name, diagnostic, colored)?;
        output.push_char('\n')?;
    }
    append_format(
        output,
        format_args!(
            "diagnostic summary: {} error(s), {} warning(s), {} note(s), {} total",
            counts.errors, counts.warnings, counts.notes, counts.total
        ),
    )?;
    output.push_char('\n')?;
    Ok(())
}

fn render_diagnostic(
    output: &mut RenderBuffer,
    source: &SourceIndex<'_>,
    source_name: &str,
    diagnostic: &Diagnostic,
    colored: bool,
) -> Result<(), ArtifactError> {
    let annotations = annotations(source, diagnostic)?;
    let suggestion_lines = diagnostic
        .fixes
        .iter()
        .map(|fix| suggestion_last_line(source, fix))
        .max()
        .unwrap_or(0);
    let max_line = annotations
        .keys()
        .copied()
        .max()
        .unwrap_or(1)
        .max(suggestion_lines);
    let gutter = max_line.to_string().len();
    // The `--> file:line:column` header uses the one-based Unicode scalar
    // (character) column, as rustc does, so editors and agents land on the
    // exact character; display width is used only to align carets.
    let position = source.position(diagnostic.primary.span.start);
    let severity = paint(
        diagnostic.severity.as_str(),
        severity_color(diagnostic.severity),
        colored,
    );
    append_format(
        output,
        format_args!(
            "{severity}[{}]: {}\n{:gutter$}{} {}:{}:{}\n",
            TerminalText(&diagnostic.code),
            TerminalText(&diagnostic.message),
            "",
            paint("-->", "34", colored),
            TerminalText(display_source_name(source_name)),
            position.line,
            position.column
        ),
    )?;
    let bar = paint("|", "34", colored);
    append_format(output, format_args!("{:gutter$} {bar}\n", ""))?;
    render_annotated_lines(
        output,
        source,
        gutter,
        &bar,
        &annotations,
        diagnostic.primary.span,
        colored,
    )?;
    let has_footer = !diagnostic.notes.is_empty()
        || diagnostic.help.is_some()
        || !diagnostic.spec_references.is_empty();
    if has_footer {
        append_format(output, format_args!("{:gutter$} {bar}\n", ""))?;
    }
    for note in &diagnostic.notes {
        append_format(
            output,
            format_args!("{:gutter$} = note: {}\n", "", TerminalText(note)),
        )?;
    }
    if let Some(help) = &diagnostic.help {
        append_format(
            output,
            format_args!("{:gutter$} = help: {}\n", "", TerminalText(help)),
        )?;
    }
    for reference in &diagnostic.spec_references {
        append_format(
            output,
            format_args!("{:gutter$} = spec: {}\n", "", TerminalText(reference)),
        )?;
    }
    for fix in &diagnostic.fixes {
        render_suggestion(output, source, gutter, &bar, fix, colored)?;
    }
    Ok(())
}

/// Renders every annotated source line with its underline rows, bridging a
/// one-line gap with the skipped line and longer gaps with `...`.
fn render_annotated_lines(
    output: &mut RenderBuffer,
    source: &SourceIndex<'_>,
    gutter: usize,
    bar: &str,
    annotations: &BTreeMap<usize, Vec<Annotation>>,
    primary: ByteSpan,
    colored: bool,
) -> Result<(), ArtifactError> {
    let lanes = Lanes::of(annotations);
    let mut previous_line: Option<usize> = None;
    for (line, line_annotations) in annotations {
        if let Some(previous) = previous_line {
            if line.saturating_sub(previous) == 2 {
                let skipped = previous + 1;
                let text = source
                    .line(skipped)
                    .ok_or(ArtifactError::InvalidSpan(primary))?;
                append_format(
                    output,
                    format_args!(
                        "{} {bar} {}{}\n",
                        paint(&format!("{skipped:>gutter$}"), "34", colored),
                        lanes.margin(skipped, Stage::Inside, colored),
                        TerminalSourceLine(text)
                    ),
                )?;
            } else if line.saturating_sub(previous) > 2 {
                output.push_str("...\n")?;
            }
        }
        let text = source
            .line(*line)
            .ok_or(ArtifactError::InvalidSpan(primary))?;
        let window = LineWindow::for_line(text, line_annotations);
        let shown = window.as_ref().map_or_else(
            || TerminalSourceLine(text).to_string(),
            |window| window.render(text),
        );
        append_format(
            output,
            format_args!(
                "{} {bar} {}{shown}\n",
                paint(&format!("{line:>gutter$}"), "34", colored),
                lanes.margin(*line, Stage::Source, colored),
            ),
        )?;
        let shifted;
        let line_annotations = match &window {
            Some(window) => {
                shifted = window.shift(line_annotations);
                &shifted
            }
            None => line_annotations,
        };
        let context = RowContext {
            gutter,
            bar,
            lanes: &lanes,
            line: *line,
            colored,
        };
        render_multiline_starts(output, &context, line_annotations)?;
        let single: Vec<Annotation> = line_annotations
            .iter()
            .filter(|annotation| annotation.kind == Kind::Single)
            .cloned()
            .collect();
        render_line_annotations(output, &context, &single)?;
        render_multiline_ends(output, &context, line_annotations)?;
        previous_line = Some(*line);
    }
    Ok(())
}

/// Where a margin is drawn relative to one source line's rows.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Stage {
    /// The source line itself.
    Source,
    /// A line inside a multi-line label (a bridged skipped line).
    Inside,
    /// An underline row, after the multi-line starts of the line and before
    /// its multi-line ends.
    Row,
}

/// Left-margin lanes of the multi-line labels of one diagnostic (rustc's
/// `|` and `_` markers). Each multi-line label owns one lane, assigned in
/// order of its first line.
struct Lanes {
    /// Per lane: first line, last line, and whether the label is primary.
    spans: Vec<(usize, usize, bool)>,
}

impl Lanes {
    fn of(annotations: &BTreeMap<usize, Vec<Annotation>>) -> Self {
        let mut spans: Vec<(usize, usize, bool)> = Vec::new();
        for (line, line_annotations) in annotations {
            for annotation in line_annotations {
                if let Kind::Start(lane) = annotation.kind {
                    if spans.len() <= lane {
                        spans.resize(lane + 1, (0, 0, false));
                    }
                    if let Some(span) = spans.get_mut(lane) {
                        span.0 = *line;
                        span.2 = annotation.primary;
                    }
                }
                if let Kind::End(lane) = annotation.kind {
                    if spans.len() <= lane {
                        spans.resize(lane + 1, (0, 0, false));
                    }
                    if let Some(span) = spans.get_mut(lane) {
                        span.1 = *line;
                    }
                }
            }
        }
        Self { spans }
    }

    /// Margin cells for `line` at `stage` (empty when no label spans lines).
    fn margin(&self, line: usize, stage: Stage, colored: bool) -> String {
        if self.spans.is_empty() {
            return String::new();
        }
        let mut margin = String::new();
        for (first, last, primary) in &self.spans {
            let active = match stage {
                Stage::Source | Stage::Inside => *first < line && line <= *last,
                Stage::Row => *first <= line && line <= *last,
            };
            if active {
                margin.push_str(&paint("|", if *primary { "31" } else { "34" }, colored));
            } else {
                margin.push(' ');
            }
        }
        margin.push(' ');
        margin
    }

    /// Margin width in display columns.
    fn width(&self) -> usize {
        if self.spans.is_empty() {
            0
        } else {
            self.spans.len() + 1
        }
    }
}

/// What every underline row of one source line shares.
struct RowContext<'a> {
    gutter: usize,
    bar: &'a str,
    lanes: &'a Lanes,
    line: usize,
    colored: bool,
}

impl RowContext<'_> {
    fn emit(
        &self,
        output: &mut RenderBuffer,
        margin: &str,
        row: &str,
    ) -> Result<(), ArtifactError> {
        append_format(
            output,
            format_args!(
                "{:gutter$} {} {margin}{row}\n",
                "",
                self.bar,
                gutter = self.gutter
            ),
        )
    }
}

/// `  ______^` rows: a multi-line label starting on this line, drawn from
/// its lane to its first column.
fn render_multiline_starts(
    output: &mut RenderBuffer,
    context: &RowContext<'_>,
    annotations: &[Annotation],
) -> Result<(), ArtifactError> {
    for annotation in annotations {
        let Kind::Start(lane) = annotation.kind else {
            continue;
        };
        let mut margin = context
            .lanes
            .margin(context.line, Stage::Source, context.colored);
        // A space in the lane, then a `_` run to the first marked column.
        let marker = if annotation.primary { "^" } else { "-" };
        let run = context
            .lanes
            .width()
            .saturating_sub(lane + 2)
            .saturating_add(annotation.start);
        margin = cut_margin(&margin, lane);
        let text = format!(" {}{marker}", "_".repeat(run));
        context.emit(
            output,
            &margin,
            &paint(
                &text,
                if annotation.primary { "31" } else { "34" },
                context.colored,
            ),
        )?;
    }
    Ok(())
}

/// `|_____^ message` rows: a multi-line label ending on this line.
fn render_multiline_ends(
    output: &mut RenderBuffer,
    context: &RowContext<'_>,
    annotations: &[Annotation],
) -> Result<(), ArtifactError> {
    for annotation in annotations {
        let Kind::End(lane) = annotation.kind else {
            continue;
        };
        let margin = context
            .lanes
            .margin(context.line, Stage::Source, context.colored);
        let marker = if annotation.primary { "^" } else { "-" };
        let run = context
            .lanes
            .width()
            .saturating_sub(lane + 2)
            .saturating_add(annotation.start);
        let text = format!("|{}{marker}", "_".repeat(run));
        let mut row = paint(
            &text,
            if annotation.primary { "31" } else { "34" },
            context.colored,
        );
        if let Some(message) = &annotation.message {
            row.push(' ');
            row.push_str(&TerminalText(message).to_string());
        }
        context.emit(output, &cut_margin(&margin, lane), &row)?;
    }
    Ok(())
}

/// The margin cells before `lane` (the rest of the row is drawn by the
/// caller). Margin cells are one display column each, possibly painted.
fn cut_margin(margin: &str, lane: usize) -> String {
    let mut kept = String::new();
    let mut cells = 0;
    let mut rest = margin;
    while cells < lane {
        // A painted cell is `ESC[..m|ESC[0m`; a plain one is one character.
        let cell_end = if rest.starts_with('\u{1b}') {
            rest.find("\u{1b}[0m")
                .map_or(rest.len(), |at| at + "\u{1b}[0m".len())
        } else {
            rest.chars().next().map_or(0, char::len_utf8)
        };
        if cell_end == 0 {
            break;
        }
        kept.push_str(rest.get(..cell_end).unwrap_or_default());
        rest = rest.get(cell_end..).unwrap_or_default();
        cells += 1;
    }
    kept
}

/// Visible part of a source line wider than [`MAX_SOURCE_LINE_COLUMNS`].
#[derive(Debug)]
struct LineWindow {
    /// Byte range of the visible text.
    start_byte: usize,
    end_byte: usize,
    /// One-based display column of the first visible character.
    start_column: usize,
    /// Exclusive one-based display column after the last visible character.
    end_column: usize,
    elided_before: bool,
    elided_after: bool,
}

impl LineWindow {
    /// A window around the labels of a long, tab-free line; `None` when the
    /// whole line fits (tabs change width with position, so tabbed lines are
    /// never windowed).
    fn for_line(text: &str, annotations: &[Annotation]) -> Option<Self> {
        let width = display_width(text);
        if width <= MAX_SOURCE_LINE_COLUMNS || text.contains('\t') {
            return None;
        }
        let low = annotations
            .iter()
            .map(|annotation| annotation.start)
            .min()?;
        let high = annotations.iter().map(|annotation| annotation.end).max()?;
        let want_start = low.saturating_sub(LONG_LINE_CONTEXT_COLUMNS).max(1);
        let want_end = high
            .saturating_add(LONG_LINE_CONTEXT_COLUMNS)
            .min(want_start.saturating_add(MAX_SOURCE_LINE_COLUMNS))
            .min(width.saturating_add(1));
        let mut column = 1_usize;
        let mut window: Option<Self> = None;
        for (offset, character) in text.char_indices() {
            // Characters past the window never change it; stopping here keeps
            // the scan proportional to the window's end, not the line length.
            if column >= want_end {
                break;
            }
            let next = column.saturating_add(display_width(character.encode_utf8(&mut [0; 4])));
            if column >= want_start && next <= want_end {
                let current = window.get_or_insert(Self {
                    start_byte: offset,
                    end_byte: offset,
                    start_column: column,
                    end_column: column,
                    elided_before: offset > 0,
                    elided_after: false,
                });
                current.end_byte = offset + character.len_utf8();
                current.end_column = next;
            }
            column = next;
        }
        let mut window = window?;
        window.elided_after = window.end_byte < text.len();
        Some(window)
    }

    fn render(&self, text: &str) -> String {
        let visible = text.get(self.start_byte..self.end_byte).unwrap_or_default();
        format!(
            "{}{}{}",
            if self.elided_before { ELISION } else { "" },
            TerminalSourceLine(visible),
            if self.elided_after { ELISION } else { "" }
        )
    }

    /// Moves annotations into window coordinates, clipping them to it.
    fn shift(&self, annotations: &[Annotation]) -> Vec<Annotation> {
        let lead = if self.elided_before { ELISION.len() } else { 0 };
        let map = |column: usize| {
            column
                .clamp(self.start_column, self.end_column)
                .saturating_sub(self.start_column)
                .saturating_add(1)
                .saturating_add(lead)
        };
        annotations
            .iter()
            .map(|annotation| {
                let start = map(annotation.start);
                Annotation {
                    start,
                    end: map(annotation.end).max(start.saturating_add(1)),
                    primary: annotation.primary,
                    message: annotation.message.clone(),
                    kind: annotation.kind,
                }
            })
            .collect()
    }
}

/// One underline projected onto one source line.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Annotation {
    /// One-based display column of the first marked cell.
    start: usize,
    /// Exclusive one-based display column after the last marked cell.
    end: usize,
    primary: bool,
    /// Present only on the final line of a label.
    message: Option<String>,
    /// Whether this marks a single-line label, or the first or last line of
    /// a multi-line label (with its margin lane).
    kind: Kind,
}

/// How one annotation is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    /// An underline on one line.
    Single,
    /// The first column of a multi-line label in the given lane.
    Start(usize),
    /// The last column of a multi-line label in the given lane.
    End(usize),
}

/// Whether a label is the diagnostic's primary label (`^`) or a secondary
/// one (`-`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LabelRole {
    Primary,
    Secondary,
}

fn annotations(
    source: &SourceIndex<'_>,
    diagnostic: &Diagnostic,
) -> Result<BTreeMap<usize, Vec<Annotation>>, ArtifactError> {
    let mut lines: BTreeMap<usize, Vec<Annotation>> = BTreeMap::new();
    let mut next_lane = 0_usize;
    project_label(
        source,
        &diagnostic.primary,
        LabelRole::Primary,
        &mut next_lane,
        &mut lines,
    )?;
    for label in &diagnostic.secondary {
        project_label(
            source,
            label,
            LabelRole::Secondary,
            &mut next_lane,
            &mut lines,
        )?;
    }
    for annotations in lines.values_mut() {
        annotations.sort_by(|left, right| {
            (left.start, left.end, !left.primary).cmp(&(right.start, right.end, !right.primary))
        });
        annotations.dedup();
    }
    Ok(lines)
}

/// Projects one label onto its lines: an underline when it covers one line;
/// otherwise, as rustc does, a start marker on its first line and an end
/// marker with the message on its last, joined by a margin lane.
fn project_label(
    source: &SourceIndex<'_>,
    label: &Label,
    role: LabelRole,
    next_lane: &mut usize,
    lines: &mut BTreeMap<usize, Vec<Annotation>>,
) -> Result<(), ArtifactError> {
    let primary = role == LabelRole::Primary;
    let start = source.position_display(label.span.start);
    let end = source.position_display(label.span.end);
    let end_line = if label.span.end > label.span.start && end.column == 1 {
        end.line.saturating_sub(1).max(start.line)
    } else {
        end.line
    };
    let last = source
        .line(end_line)
        .ok_or(ArtifactError::InvalidSpan(label.span))?;
    if end_line == start.line {
        let marker_end = if end_line == end.line {
            end.column
        } else {
            display_width(last).saturating_add(1)
        };
        lines.entry(start.line).or_default().push(Annotation {
            start: start.column,
            end: marker_end.max(start.column.saturating_add(1)),
            primary,
            message: label.message.clone(),
            kind: Kind::Single,
        });
        return Ok(());
    }
    let lane = *next_lane;
    *next_lane = next_lane.saturating_add(1);
    lines.entry(start.line).or_default().push(Annotation {
        start: start.column,
        end: start.column.saturating_add(1),
        primary,
        message: None,
        kind: Kind::Start(lane),
    });
    // The last marked column: the character before the exclusive end, or
    // the end of the line when the span runs through its line break.
    let last_column = if end_line == end.line {
        end.column.saturating_sub(1).max(1)
    } else {
        display_width(last).max(1)
    };
    lines.entry(end_line).or_default().push(Annotation {
        start: last_column,
        end: last_column.saturating_add(1),
        primary,
        message: label.message.clone(),
        kind: Kind::End(lane),
    });
    Ok(())
}

/// Draws rustc-style underline rows for one source line.
///
/// Non-overlapping underlines share one row; the rightmost message is placed
/// inline and the remaining messages hang below their underline on
/// `|`-connected rows, right to left. Overlapping underlines fall back to one
/// row per annotation so no marker is hidden.
fn render_line_annotations(
    output: &mut RenderBuffer,
    context: &RowContext<'_>,
    annotations: &[Annotation],
) -> Result<(), ArtifactError> {
    if annotations.is_empty() {
        return Ok(());
    }
    let colored = context.colored;
    let margin = context.lanes.margin(context.line, Stage::Row, colored);
    let overlapping = annotations
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if right.start < left.end));
    if overlapping {
        for annotation in annotations {
            let mut row = Row::default();
            row.underline(annotation, colored);
            if let Some(message) = &annotation.message {
                row.push_text(" ");
                row.push_text(&TerminalText(message).to_string());
            }
            context.emit(output, &margin, &row.text)?;
        }
        return Ok(());
    }
    let mut row = Row::default();
    for annotation in annotations {
        row.underline(annotation, colored);
    }
    let mut hanging: Vec<&Annotation> = annotations
        .iter()
        .filter(|annotation| annotation.message.is_some())
        .collect();
    if let Some(last) = annotations.last()
        && let Some(message) = &last.message
    {
        row.push_text(" ");
        row.push_text(&TerminalText(message).to_string());
        hanging.retain(|annotation| !std::ptr::eq(*annotation, last));
    }
    context.emit(output, &margin, &row.text)?;
    let mut first_hanging = true;
    while let Some(current) = hanging.pop() {
        if first_hanging {
            let mut connectors = Row::default();
            for annotation in hanging.iter().copied().chain(std::iter::once(current)) {
                connectors.pad_to(annotation.start);
                connectors.push_marker("|", annotation.primary, colored);
            }
            context.emit(output, &margin, &connectors.text)?;
            first_hanging = false;
        }
        let mut message_row = Row::default();
        for annotation in &hanging {
            message_row.pad_to(annotation.start);
            message_row.push_marker("|", annotation.primary, colored);
        }
        message_row.pad_to(current.start);
        if let Some(message) = &current.message {
            message_row.push_text(&TerminalText(message).to_string());
        }
        context.emit(output, &margin, &message_row.text)?;
    }
    Ok(())
}

/// Accumulates one underline row while tracking its display column.
#[derive(Default)]
struct Row {
    text: String,
    /// Next one-based display column.
    column: usize,
}

impl Row {
    fn pad_to(&mut self, column: usize) {
        let current = self.column.max(1);
        for _ in current..column {
            self.text.push(' ');
        }
        self.column = current.max(column);
    }

    fn underline(&mut self, annotation: &Annotation, colored: bool) {
        self.pad_to(annotation.start);
        let width = annotation.end.saturating_sub(annotation.start).max(1);
        let marker = if annotation.primary { "^" } else { "-" };
        self.push_marker(&marker.repeat(width), annotation.primary, colored);
    }

    fn push_marker(&mut self, marker: &str, primary: bool, colored: bool) {
        self.column = self.column.max(1).saturating_add(marker.chars().count());
        self.text
            .push_str(&paint(marker, if primary { "31" } else { "34" }, colored));
    }

    fn push_text(&mut self, text: &str) {
        self.column = self.column.max(1).saturating_add(display_width(text));
        self.text.push_str(text);
    }
}

/// Returns an upper bound on the line numbers a fix displays, for gutter
/// sizing: the last original line it touches plus every line it adds.
fn suggestion_last_line(source: &SourceIndex<'_>, fix: &Fix) -> usize {
    let last = fix
        .edits
        .iter()
        .map(|edit| source.position(edit.span.end).line)
        .max()
        .unwrap_or(1);
    let added: usize = fix
        .edits
        .iter()
        .map(|edit| edit.replacement.matches('\n').count())
        .sum();
    last.saturating_add(added)
}

/// One run of a fix's edits whose original lines touch or are at most one
/// unchanged line apart, rendered as one block of the suggestion.
struct Hunk {
    /// First original line of the block.
    first_line: usize,
    /// Original text of the block's complete lines.
    original: String,
    /// The same lines with the block's edits applied.
    rewritten: String,
    /// Byte ranges of changed text inside `rewritten`, with insertion flags.
    changes: Vec<(usize, usize, bool)>,
}

impl Hunk {
    /// Lines the block adds (positive) or removes (negative).
    fn line_delta(&self) -> isize {
        let count = |text: &str| isize::try_from(text.matches('\n').count()).unwrap_or(isize::MAX);
        count(&self.rewritten).saturating_sub(count(&self.original))
    }
}

/// Groups a fix's sorted, non-overlapping edits into [`Hunk`]s: an edit
/// joins the previous block when at most one unchanged line separates it
/// from that block's last original line (the line is then shown, as for
/// labels), so the elided lines between two blocks are never changed.
fn suggestion_hunks(source: &SourceIndex<'_>, fix: &Fix) -> Result<Vec<Hunk>, ArtifactError> {
    let mut groups: Vec<(usize, usize, Vec<&super::TextEdit>)> = Vec::new();
    for edit in &fix.edits {
        let start_line = source.position(edit.span.start).line;
        let end_line = source.position(edit.span.end).line;
        match groups.last_mut() {
            Some((_, last_line, edits)) if start_line <= last_line.saturating_add(2) => {
                *last_line = (*last_line).max(end_line);
                edits.push(edit);
            }
            _ => groups.push((start_line, end_line, vec![edit])),
        }
    }
    groups
        .into_iter()
        .map(|(first_line, last_line, edits)| {
            let first = edits.first().ok_or(ArtifactError::OverlappingEdits)?;
            let region_start = source
                .line_start(first_line)
                .ok_or(ArtifactError::InvalidSpan(first.span))?;
            let region_end = source
                .line_end(last_line)
                .ok_or(ArtifactError::InvalidSpan(first.span))?;
            let original = source
                .text
                .get(region_start..region_end)
                .ok_or(ArtifactError::InvalidSpan(first.span))?;
            let mut rewritten = String::with_capacity(original.len());
            let mut changes = Vec::with_capacity(edits.len());
            let mut cursor = region_start;
            for edit in &edits {
                let start = offset(edit.span.start)?;
                let end = offset(edit.span.end)?;
                rewritten.push_str(
                    source
                        .text
                        .get(cursor..start)
                        .ok_or(ArtifactError::InvalidSpan(edit.span))?,
                );
                let changed_start = rewritten.len();
                rewritten.push_str(&edit.replacement);
                changes.push((changed_start, rewritten.len(), start == end));
                cursor = end;
            }
            rewritten.push_str(
                source
                    .text
                    .get(cursor..region_end)
                    .ok_or(ArtifactError::InvalidSpan(first.span))?,
            );
            Ok(Hunk {
                first_line,
                original: original.to_owned(),
                rewritten,
                changes,
            })
        })
        .collect()
}

/// Renders one structured fix as a rustc-style `help:` suggestion.
///
/// Each run of edits on nearby lines is one block, and blocks
/// are separated by `...`, as rustc separates the parts of a multipart
/// suggestion, so an edit far from the first one is still shown. A fix that
/// deletes anything renders every block as a `-`/`+` diff of its changed
/// lines; insertions and replacements render the resulting lines with `+`
/// under inserted text and `~` under replaced text. Any block longer than
/// [`MAX_SUGGESTION_LINES`], and any block after [`MAX_SUGGESTION_BLOCKS`],
/// is elided with an explicit `...` marker, never silently. Line numbers
/// after the first block account for the lines earlier blocks add or remove.
/// Edits were validated as sorted, non-overlapping, and on UTF-8 boundaries
/// before rendering.
fn render_suggestion(
    output: &mut RenderBuffer,
    source: &SourceIndex<'_>,
    gutter: usize,
    bar: &str,
    fix: &Fix,
    colored: bool,
) -> Result<(), ArtifactError> {
    if fix.edits.is_empty() {
        return Err(ArtifactError::OverlappingEdits);
    }
    let hunks = suggestion_hunks(source, fix)?;
    append_format(
        output,
        format_args!(
            "{}: {}\n{:gutter$} {bar}\n",
            paint("help", "36", colored),
            TerminalText(&fix.message),
            ""
        ),
    )?;
    // A fix that deletes anything (alone, or with other edits such as the
    // insertion of a moved element) is shown as a `-`/`+` diff, so no edit
    // is invisible; pure insertions and replacements are marked inline.
    let deletes = fix
        .edits
        .iter()
        .any(|edit| edit.replacement.is_empty() && edit.span.end > edit.span.start);
    let mut line_delta = 0_isize;
    for (index, hunk) in hunks.iter().enumerate() {
        if index >= MAX_SUGGESTION_BLOCKS {
            append_format(
                output,
                format_args!(
                    "... {} more changed region(s) not shown\n",
                    hunks.len() - index
                ),
            )?;
            break;
        }
        if index > 0 {
            output.push_str("...\n")?;
        }
        let rewritten_first = hunk.first_line.saturating_add_signed(line_delta);
        if deletes {
            render_deletion_diff(output, gutter, hunk, rewritten_first, colored)?;
        } else {
            let mut line_start = 0_usize;
            for (line_index, segment) in hunk.rewritten.split('\n').enumerate() {
                if line_index >= MAX_SUGGESTION_LINES {
                    output.push_str("...\n")?;
                    break;
                }
                let line = segment.strip_suffix('\r').unwrap_or(segment);
                render_suggestion_line(
                    output,
                    gutter,
                    bar,
                    rewritten_first + line_index,
                    line,
                    (line_start, &hunk.rewritten, &hunk.changes),
                    colored,
                )?;
                line_start = line_start.saturating_add(segment.len()).saturating_add(1);
            }
        }
        line_delta = line_delta.saturating_add(hunk.line_delta());
    }
    Ok(())
}

/// One rewritten suggestion line with its `+`/`~` change markers, windowed
/// around the changes when the line is too wide to print whole.
fn render_suggestion_line(
    output: &mut RenderBuffer,
    gutter: usize,
    bar: &str,
    number: usize,
    line: &str,
    (line_start, rewritten, changes): (usize, &str, &[(usize, usize, bool)]),
    colored: bool,
) -> Result<(), ArtifactError> {
    let line_end = line_start + line.len();
    // Changed cells of this line, as annotations; `primary` marks an
    // insertion (`+`) and its absence a replacement (`~`).
    let mut marks = Vec::new();
    for (changed_start, changed_end, insertion) in changes {
        let start = (*changed_start).max(line_start);
        let end = (*changed_end).min(line_end);
        if start > end || (start == end && changed_start != changed_end) {
            continue;
        }
        let prefix = rewritten.get(line_start..start).unwrap_or("");
        let changed_text = rewritten.get(start..end).unwrap_or("");
        let column = display_width(prefix).saturating_add(1);
        let width = display_width(changed_text);
        if width == 0 {
            continue;
        }
        marks.push(Annotation {
            start: column,
            end: column.saturating_add(width),
            primary: *insertion,
            message: None,
            kind: Kind::Single,
        });
    }
    let window = LineWindow::for_line(line, &marks);
    let (shown, marks) = match &window {
        Some(window) => (window.render(line), window.shift(&marks)),
        None => (TerminalSourceLine(line).to_string(), marks),
    };
    append_format(
        output,
        format_args!(
            "{} {bar} {shown}\n",
            paint(&format!("{number:>gutter$}"), "34", colored),
        ),
    )?;
    let mut row = Row::default();
    for mark in &marks {
        row.pad_to(mark.start);
        let width = mark.end.saturating_sub(mark.start).max(1);
        let marker = if mark.primary { "+" } else { "~" };
        row.column = row.column.max(1).saturating_add(width);
        row.text
            .push_str(&paint(&marker.repeat(width), "32", colored));
    }
    if !row.text.is_empty() {
        append_format(output, format_args!("{:gutter$} {bar} {}\n", "", row.text))?;
    }
    Ok(())
}

/// Renders one block of a deleting fix as a `-`/`+` diff of the lines that
/// change: lines the block leaves identical at its start or end are not
/// repeated, and more than [`MAX_SUGGESTION_LINES`] lines on either side are
/// elided with `...`.
fn render_deletion_diff(
    output: &mut RenderBuffer,
    gutter: usize,
    hunk: &Hunk,
    rewritten_first: usize,
    colored: bool,
) -> Result<(), ArtifactError> {
    let original: Vec<&str> = hunk.original.split('\n').map(trim_cr).collect();
    let rewritten: Vec<&str> = hunk.rewritten.split('\n').map(trim_cr).collect();
    let prefix = original
        .iter()
        .zip(&rewritten)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = original
        .iter()
        .rev()
        .zip(rewritten.iter().rev())
        .take(original.len().min(rewritten.len()) - prefix)
        .take_while(|(left, right)| left == right)
        .count();
    let removed = original
        .get(prefix..original.len() - suffix)
        .unwrap_or_default();
    let added = rewritten
        .get(prefix..rewritten.len() - suffix)
        .unwrap_or_default();
    for (marker, color, first, lines, other) in [
        ("-", "31", hunk.first_line + prefix, removed, added),
        ("+", "32", rewritten_first + prefix, added, removed),
    ] {
        for (index, line) in lines.iter().enumerate() {
            if index >= MAX_SUGGESTION_LINES {
                output.push_str("...\n")?;
                break;
            }
            let shown = diff_line_view(line, other.get(index).copied().unwrap_or(""));
            append_format(
                output,
                format_args!(
                    "{} {} {shown}\n",
                    paint(&format!("{:>gutter$}", first + index), "34", colored),
                    paint(marker, color, colored),
                ),
            )?;
        }
    }
    Ok(())
}

fn trim_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// One deletion-diff line, windowed around its first difference from the
/// paired line when it is too wide to print whole.
fn diff_line_view(line: &str, other: &str) -> String {
    let common = line
        .char_indices()
        .zip(other.chars())
        .find(|((_, left), right)| left != right)
        .map_or_else(|| line.len().min(other.len()), |((offset, _), _)| offset);
    let column = display_width(line.get(..common).unwrap_or("")).saturating_add(1);
    let mark = Annotation {
        start: column,
        end: column.saturating_add(1),
        primary: true,
        message: None,
        kind: Kind::Single,
    };
    LineWindow::for_line(line, std::slice::from_ref(&mark)).map_or_else(
        || TerminalSourceLine(line).to_string(),
        |window| window.render(line),
    )
}

fn offset(value: u64) -> Result<usize, ArtifactError> {
    usize::try_from(value).map_err(|_| {
        ArtifactError::InvalidSpan(ByteSpan {
            start: value,
            end: value,
        })
    })
}
