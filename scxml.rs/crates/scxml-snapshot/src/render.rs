//! Human (rustc-like) renderings of documents and diffs. Text only: values
//! longer than [`ELIDE_BYTES`] are elided here and never in JSON.
//!
//! Every string taken from a document (or from a caller-supplied label) is
//! untrusted: it is escaped through [`scxml_reporting::TerminalText`] before
//! it is painted or elided, so control, DEL, C1, line-separator, and bidi
//! characters print as visible `\u{XXXX}` (or `\n`, `\r`, `\t`) escapes.
//! A multi-line value therefore stays on its `+`/`-`/`~` line, and only the
//! renderer's own color codes reach the terminal.

use std::fmt::{self, Write as _};

use scxml_reporting::TerminalText;

use crate::diff::{
    Change, DiffDocument, FifoDiff, InvocationMetadata, SessionDiff, SessionList, SystemKey,
};
use crate::{
    ComponentRole, Document, Event, EventSchema, EventType, InvokedBy, PendingSend, Section,
    Session, Status, Transition, TransitionType, Value, VariableValue,
};

/// Longest value rendered in full, in bytes.
pub const ELIDE_BYTES: usize = 120;

/// Whether human output carries ANSI color.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Color {
    /// ANSI SGR sequences.
    Always,
    /// Plain text.
    #[default]
    Never,
}

#[derive(Clone, Copy)]
enum Style {
    Bold,
    Accent,
    Added,
    Removed,
    Changed,
}

impl Color {
    fn paint(self, style: Style, text: &str) -> String {
        match self {
            Self::Never => text.to_owned(),
            Self::Always => {
                let code = match style {
                    Style::Bold => "1",
                    Style::Accent => "1;34",
                    Style::Added => "32",
                    Style::Removed => "31",
                    Style::Changed => "33",
                };
                format!("\u{1b}[{code}m{text}\u{1b}[0m")
            }
        }
    }
}

/// Appends formatted text to `text`.
fn push(text: &mut String, args: fmt::Arguments<'_>) {
    // Discarded: `fmt::Write for String` never returns an error, so the
    // `fmt::Result` carries no information.
    let _ = text.write_fmt(args);
}

/// The terminal-safe projection of one untrusted string.
fn esc(text: &str) -> String {
    TerminalText(text).to_string()
}

/// Escapes `text`, then elides the escaped form on a `char` boundary; the
/// byte count names the original (unescaped) length.
fn elide(text: &str) -> String {
    let escaped = esc(text);
    if escaped.len() <= ELIDE_BYTES {
        return escaped;
    }
    let mut end = ELIDE_BYTES;
    while !escaped.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes)", &escaped[..end], text.len())
}

/// The first 12 characters of a digest, escaped (a rendered document may
/// still fail the decoder's digest-shape check).
fn short_digest(digest: &str) -> String {
    esc(&digest.chars().take(12).collect::<String>())
}

const fn status_text(status: Status) -> &'static str {
    match status {
        Status::Ready => "ready",
        Status::Running => "running",
        Status::Finished => "finished",
    }
}

const fn section_text(section: Section) -> &'static str {
    match section {
        Section::System => "system",
        Section::DataModel => "data_model",
        Section::Queues => "queues",
        Section::PendingSends => "pending_sends",
        Section::Transitions => "transitions",
        Section::Previews => "previews",
        Section::Invocations => "invocations",
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Absent => "absent".into(),
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Boolean(value) => value.to_string(),
        Value::Number(text) => esc(text),
        Value::Text(text) => elide(&format!("{text:?}")),
        Value::JsonArray(text) | Value::JsonObject(text) => elide(text),
        Value::Xml(text) => format!("xml {}", elide(text)),
    }
}

fn variable_text(value: &VariableValue) -> String {
    match value {
        VariableValue::Value(value) => value_text(value),
        VariableValue::Text(text) => format!("{} {}", esc(&text.kind), elide(&text.text)),
        VariableValue::Oversized(oversized) => {
            format!("<oversized: {} bytes>", oversized.bytes)
        }
    }
}

fn event_text(event: &Event) -> String {
    let kind = match event.event_type {
        EventType::Internal => "internal",
        EventType::External => "external",
        EventType::Platform => "platform",
    };
    let mut text = format!("`{}` ({kind})", esc(&event.name));
    for (label, field) in [
        ("sendid", &event.sendid),
        ("invokeid", &event.invokeid),
        ("origin", &event.origin),
        ("origintype", &event.origintype),
    ] {
        if let Some(field) = field {
            push(&mut text, format_args!(" {label} {}", elide(field)));
        }
    }
    if event.data != Value::Absent {
        push(&mut text, format_args!(" data {}", value_text(&event.data)));
    }
    if event.trace_context.is_some() {
        text.push_str(" [trace context]");
    }
    text
}

fn pending_text(send: &PendingSend, now_ms: u64) -> String {
    let due = i128::from(send.deadline_ms) - i128::from(now_ms);
    let due = if due >= 0 {
        format!("due +{due}ms")
    } else {
        format!("overdue {}ms", -due)
    };
    let mut text = format!(
        "{} {} {due} (deadline {}ms, delay {}ms)",
        esc(&send.sendid),
        message_text(send),
        send.deadline_ms,
        send.delay_ms
    );
    if let Some(target) = &send.target {
        push(&mut text, format_args!(" target {}", elide(target)));
    }
    if let Some(kind) = &send.send_type {
        push(&mut text, format_args!(" type {}", elide(kind)));
    }
    text
}

/// What one pending send delivers: its event, or another processor's
/// message.
fn message_text(send: &PendingSend) -> String {
    match (&send.event, &send.message) {
        (Some(event), _) => event_text(event),
        (None, Some(message)) => {
            let mut text = String::from("message");
            if let Some(name) = &message.event {
                push(&mut text, format_args!(" event {}", esc(name)));
            }
            for param in &message.params {
                push(
                    &mut text,
                    format_args!(" param {}={}", esc(&param.name), value_text(&param.value)),
                );
            }
            if let Some(content) = &message.content {
                push(&mut text, format_args!(" content {}", value_text(content)));
            }
            if message.trace_context.is_some() {
                text.push_str(" [trace context]");
            }
            text
        }
        (None, None) => String::new(),
    }
}

/// `[a, b]` of untrusted ids, each escaped.
fn transition_text(transition: &Transition) -> String {
    let kind = match transition.transition_type {
        TransitionType::External => "external",
        TransitionType::Internal => "internal",
    };
    let mut line = format!(
        "{}[{}] on {} -> {} ({kind})",
        esc(&transition.source),
        transition.index,
        esc(&transition.events.join(" ")),
        list(&transition.targets)
    );
    if let Some(cond) = &transition.cond {
        push(&mut line, format_args!(" cond `{}`", elide(cond)));
    }
    if let Some(domain) = &transition.domain {
        push(&mut line, format_args!(" domain {}", esc(domain)));
    }
    match &transition.event_schema {
        None => {}
        Some(EventSchema::Schema { sha256, .. }) => push(
            &mut line,
            format_args!(" schema sha256:{}", sha256.get(..12).unwrap_or(sha256)),
        ),
        Some(EventSchema::Schemaexpr { expr }) => {
            push(&mut line, format_args!(" schemaexpr `{}`", elide(expr)));
        }
    }
    line
}

fn previews_text(transition: &Transition) -> Option<String> {
    match (&transition.exit, &transition.entry) {
        (Some(exit), Some(entry)) => Some(format!("exit {} entry {}", list(exit), list(entry))),
        _ => None,
    }
}

const fn role_text(role: ComponentRole) -> &'static str {
    match role {
        ComponentRole::Clock => "clock",
        ComponentRole::DataModelEcmascript => "data-model-ecmascript",
        ComponentRole::DataModelNull => "data-model-null",
        ComponentRole::DataModelXpath => "data-model-xpath",
        ComponentRole::IoProcessorScxml => "io-processor-scxml",
        ComponentRole::Machine => "machine",
    }
}

const fn list_text(list: SessionList) -> &'static str {
    match list {
        SessionList::Configuration => "configuration",
        SessionList::History => "history",
        SessionList::BoundStates => "bound states",
        SessionList::DataModel => "data_model",
        SessionList::PendingSends => "pending sends",
        SessionList::Invocations => "invocations",
        SessionList::Transitions => "transitions",
    }
}

fn invoked_by_text(by: &InvokedBy) -> String {
    format!("`{}` as {}", esc(&by.parent_session_id), esc(&by.invoke_id))
}

fn invocation_text(metadata: &InvocationMetadata) -> String {
    let mut line = format!(
        "{} at {}[{}]",
        esc(&metadata.invoke_type),
        esc(&metadata.state),
        metadata.index
    );
    if metadata.autoforward {
        line.push_str(" autoforward");
    }
    if metadata.finished {
        line.push_str(" finished");
    }
    line
}

fn list(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|item| esc(item)).collect();
    format!("[{}]", items.join(", "))
}

struct Out {
    text: String,
    color: Color,
}

impl Out {
    fn line(&mut self, indent: usize, body: &str) {
        for _ in 0..indent {
            self.text.push(' ');
        }
        self.text.push_str(body);
        self.text.push('\n');
    }

    fn block(&mut self, indent: usize, label: &str, body: &str) {
        let marker = self.color.paint(Style::Accent, "=");
        let line = if body.is_empty() {
            format!("{marker} {label}:")
        } else {
            format!("{marker} {label}: {body}")
        };
        self.line(indent, &line);
    }

    fn arrow(&mut self, indent: usize, body: &str) {
        let marker = self.color.paint(Style::Accent, "-->");
        self.line(indent, &format!("{marker} {body}"));
    }

    fn mark(&mut self, indent: usize, change: Change, body: &str) {
        let (style, marker) = match change {
            Change::Added => (Style::Added, "+"),
            Change::Removed => (Style::Removed, "-"),
            Change::Changed => (Style::Changed, "~"),
        };
        let line = format!("{} {body}", self.color.paint(style, marker));
        self.line(indent, &line);
    }
}

/// Renders a per-session human view of one valid document.
#[must_use]
pub fn render_show(document: &Document, color: Color) -> String {
    let mut out = Out {
        text: String::new(),
        color,
    };
    let header = color.paint(
        Style::Bold,
        &format!(
            "snapshot {} sha256 {}",
            esc(&document.version),
            esc(&document.sha256)
        ),
    );
    out.line(0, &header);
    out.block(1, "clock", &format!("{}ms", document.snapshot.clock.now_ms));
    let components: Vec<String> = document
        .snapshot
        .components
        .iter()
        .map(|component| {
            let role = serde_json::to_value(component.role)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            format!("{role} {}", short_digest(&component.sha256))
        })
        .collect();
    out.block(1, "components", &components.join(", "));
    let exclude: Vec<String> = document
        .options
        .exclude
        .iter()
        .map(|section| section_text(*section).to_owned())
        .collect();
    let policy = match document.options.trace_context {
        crate::TraceContextPolicy::Strip => "strip",
        crate::TraceContextPolicy::Preserve => "preserve",
    };
    out.block(
        1,
        "options",
        &format!("exclude {}, trace_context {policy}", list(&exclude)),
    );
    let now_ms = document.snapshot.clock.now_ms;
    let paths = document.snapshot.paths();
    for (index, session) in document.snapshot.sessions.iter().enumerate() {
        // Each invoked session is titled by its invoke-id path (`a`, `a/x`).
        let path = match paths.get(index) {
            Some(path) if index > 0 => path.join("/"),
            _ => "root".to_owned(),
        };
        show_session(&mut out, session, &path, now_ms);
    }
    out.text
}

#[allow(
    clippy::too_many_lines,
    reason = "one linear rendering of the session keys in document order"
)]
fn show_session(out: &mut Out, session: &Session, path: &str, now_ms: u64) {
    out.text.push('\n');
    let mut title = format!("session {} `{}`", esc(path), esc(&session.session_id));
    if let Some(name) = &session.name {
        push(&mut title, format_args!(" name `{}`", esc(name)));
    }
    if let Some(model) = &session.datamodel_name {
        push(&mut title, format_args!(" datamodel {}", esc(model)));
    }
    push(
        &mut title,
        format_args!(
            " ir v{} {}",
            session.ir.version,
            short_digest(&session.ir.sha256)
        ),
    );
    let title = out.color.paint(Style::Bold, &title);
    out.line(0, &title);
    out.arrow(2, &format!("status: {}", status_text(session.status)));
    if let Some(by) = &session.invoked_by {
        out.block(
            3,
            "invoked by",
            &format!("`{}` as {}", esc(&by.parent_session_id), esc(&by.invoke_id)),
        );
    }
    out.block(
        3,
        "configuration",
        &format!(
            "atomic {} full {}",
            list(&session.configuration.atomic),
            list(&session.configuration.full)
        ),
    );
    if !session.history.is_empty() {
        out.block(3, "history", "");
        for value in &session.history {
            out.line(
                7,
                &format!("{} -> {}", esc(&value.state), list(&value.value)),
            );
        }
    }
    if !session.bound_states.is_empty() {
        out.block(3, "bound states", &list(&session.bound_states));
    }
    out.block(
        3,
        "counters",
        &format!(
            "send {}, invoke {}",
            session.counters.send, session.counters.invoke
        ),
    );
    if let Some(system) = &session.system {
        out.block(
            3,
            "system",
            &format!(
                "_sessionid `{}`, _name {}, _ioprocessors {}",
                esc(&system.sessionid),
                system
                    .name
                    .as_deref()
                    .map_or_else(|| "undefined".to_owned(), |name| format!("`{}`", esc(name))),
                value_text(&system.ioprocessors)
            ),
        );
    }
    if let Some(variables) = &session.data_model {
        out.block(
            3,
            "data model",
            if variables.is_empty() { "none" } else { "" },
        );
        for variable in variables {
            out.line(
                7,
                &format!(
                    "{} = {}",
                    esc(&variable.name),
                    variable_text(&variable.value)
                ),
            );
        }
    }
    if let Some(queues) = &session.queues {
        out.block(
            3,
            "queues",
            &format!(
                "internal {}, external {}",
                queues.internal.len(),
                queues.external.len()
            ),
        );
        for (label, events) in [
            ("internal", &queues.internal),
            ("external", &queues.external),
        ] {
            for (index, event) in events.iter().enumerate() {
                out.line(7, &format!("{label}[{index}] {}", event_text(event)));
            }
        }
    }
    if let Some(pending) = &session.pending_sends {
        out.block(
            3,
            "pending sends",
            if pending.is_empty() { "none" } else { "" },
        );
        for send in pending {
            out.line(7, &pending_text(send, now_ms));
        }
    }
    if let Some(transitions) = &session.transitions {
        out.block(
            3,
            "transitions",
            if transitions.is_empty() { "none" } else { "" },
        );
        for transition in transitions {
            out.line(7, &transition_text(transition));
            if let Some(previews) = previews_text(transition) {
                out.line(11, &previews);
            }
        }
    }
    if let Some(invocations) = &session.invocations {
        out.block(
            3,
            "invocations",
            if invocations.is_empty() { "none" } else { "" },
        );
        for invocation in invocations {
            let mut line = format!(
                "{} {} at {}[{}]",
                esc(&invocation.invoke_id),
                esc(&invocation.invoke_type),
                esc(&invocation.state),
                invocation.index
            );
            if invocation.autoforward {
                line.push_str(" autoforward");
            }
            if invocation.finished {
                line.push_str(" finished");
            }
            out.line(7, &line);
        }
    }
}

/// Renders a diff: a `snapshot diff: A → B` header, then per session a
/// `-->` line and `=` blocks with `+ - ~` markers.
#[must_use]
pub fn render_diff(diff: &DiffDocument, before: &str, after: &str, color: Color) -> String {
    let mut out = Out {
        text: String::new(),
        color,
    };
    let header = color.paint(
        Style::Bold,
        &format!("snapshot diff: {} → {}", esc(before), esc(after)),
    );
    out.line(0, &header);
    if diff.identical {
        out.block(1, "identical", "no difference");
        return out.text;
    }
    if !diff.compatible {
        out.block(
            1,
            "note",
            "the documents loaded different IR (incompatible)",
        );
    }
    if let Some(exclude) = &diff.exclude {
        let names = |sections: &[Section]| {
            let names: Vec<&str> = sections
                .iter()
                .map(|section| section_text(*section))
                .collect();
            format!("[{}]", names.join(", "))
        };
        out.block(
            1,
            "exclude",
            &format!("{} -> {}", names(&exclude.from), names(&exclude.to)),
        );
    }
    if let Some(components) = &diff.components {
        out.block(1, "components", "");
        for entry in components {
            let text = change_text(
                role_text(entry.role),
                entry.from.as_deref().map(short_digest),
                entry.to.as_deref().map(short_digest),
            );
            out.mark(5, entry.change, &text);
        }
    }
    if let Some(clock) = &diff.clock {
        out.block(
            1,
            "clock",
            &format!("{}ms -> {}ms", clock.from_ms, clock.to_ms),
        );
    }
    // An empty `skipped` is canonical but says nothing; render it only
    // when a section was actually excluded.
    if !diff.skipped.is_empty() {
        let names: Vec<String> = diff
            .skipped
            .iter()
            .map(|section| section_text(*section).to_owned())
            .collect();
        out.block(1, "skipped", &names.join(", "));
    }
    for session in &diff.sessions {
        render_session_diff(&mut out, session);
    }
    if diff.sessions.is_empty() {
        out.block(1, "sessions", "unchanged");
    }
    out.text
}

#[allow(
    clippy::too_many_lines,
    reason = "one linear rendering of the diff keys in document order"
)]
fn render_session_diff(out: &mut Out, session: &SessionDiff) {
    out.text.push('\n');
    let path = esc(&session.path.join("/"));
    match session.change {
        Change::Added | Change::Removed => {
            out.mark(0, session.change, &format!("session {path}"));
            return;
        }
        Change::Changed => {
            let title = out.color.paint(Style::Bold, &format!("session {path}"));
            out.line(0, &title);
        }
    }
    if let Some(change) = &session.session_id {
        out.block(
            3,
            "session id",
            &format!("`{}` -> `{}`", esc(&change.from), esc(&change.to)),
        );
    }
    if let Some(change) = &session.name {
        let name = |name: Option<&String>| {
            name.map_or_else(|| "none".to_owned(), |name| format!("`{}`", esc(name)))
        };
        out.block(
            3,
            "name",
            &format!(
                "{} -> {}",
                name(change.from.as_ref()),
                name(change.to.as_ref())
            ),
        );
    }
    if let Some(change) = &session.datamodel_name {
        let name = |name: Option<&String>| name.map_or_else(|| "none".to_owned(), |name| esc(name));
        out.block(
            3,
            "datamodel name",
            &format!(
                "{} -> {}",
                name(change.from.as_ref()),
                name(change.to.as_ref())
            ),
        );
    }
    if let Some(change) = &session.ir {
        out.block(
            3,
            "ir",
            &format!(
                "v{} {} -> v{} {}",
                change.from.version,
                short_digest(&change.from.sha256),
                change.to.version,
                short_digest(&change.to.sha256)
            ),
        );
    }
    match &session.status {
        Some(status) => out.arrow(
            2,
            &format!(
                "status: {} -> {}",
                status_text(status.from),
                status_text(status.to)
            ),
        ),
        None => out.arrow(2, "status: unchanged"),
    }
    if let Some(change) = &session.invoked_by {
        let by = |by: Option<&InvokedBy>| by.map_or_else(|| "none".to_owned(), invoked_by_text);
        out.block(
            3,
            "invoked by",
            &format!("{} -> {}", by(change.from.as_ref()), by(change.to.as_ref())),
        );
    }
    if let Some(configuration) = &session.configuration {
        out.block(3, "configuration", "");
        for id in &configuration.exited {
            out.mark(7, Change::Removed, &esc(id));
        }
        for id in &configuration.entered {
            out.mark(7, Change::Added, &esc(id));
        }
        if let Some(atomic) = &configuration.atomic {
            for id in &atomic.exited {
                out.mark(7, Change::Removed, &format!("atomic {}", esc(id)));
            }
            for id in &atomic.entered {
                out.mark(7, Change::Added, &format!("atomic {}", esc(id)));
            }
        }
    }
    if let Some(bound) = &session.bound_states {
        out.block(3, "bound states", "");
        for id in &bound.removed {
            out.mark(7, Change::Removed, &esc(id));
        }
        for id in &bound.added {
            out.mark(7, Change::Added, &esc(id));
        }
    }
    if let Some(system) = &session.system {
        out.block(3, "system", "");
        for entry in system {
            let name = match entry.name {
                SystemKey::Sessionid => "_sessionid",
                SystemKey::Name => "_name",
                SystemKey::Ioprocessors => "_ioprocessors",
            };
            let text = change_text(
                name,
                entry.from.as_ref().map(value_text),
                entry.to.as_ref().map(value_text),
            );
            out.mark(7, entry.change, &text);
        }
    }
    if let Some(datamodel) = &session.data_model {
        out.block(3, "data model", "");
        for entry in datamodel {
            let text = change_text(
                &esc(&entry.name),
                entry.from.as_ref().map(variable_text),
                entry.to.as_ref().map(variable_text),
            );
            out.mark(7, entry.change, &text);
        }
    }
    if let Some(history) = &session.history {
        out.block(3, "history", "");
        for entry in history {
            let text = change_text(
                &esc(&entry.state),
                entry.from.as_deref().map(list),
                entry.to.as_deref().map(list),
            );
            out.mark(7, entry.change, &text);
        }
    }
    if let Some(counters) = &session.counters {
        out.block(3, "counters", "");
        for (label, counter) in [("send", counters.send), ("invoke", counters.invoke)] {
            if let Some(counter) = counter {
                out.mark(
                    7,
                    Change::Changed,
                    &format!("{label}: {} -> {}", counter.from, counter.to),
                );
            }
        }
    }
    if let Some(queues) = &session.queues {
        out.block(3, "queues", "");
        fifo_lines(out, "internal", queues.internal.as_ref(), event_text);
        fifo_lines(out, "external", queues.external.as_ref(), event_text);
    }
    if let Some(pending) = &session.pending_sends {
        out.block(3, "pending sends", "");
        for id in &pending.removed {
            out.mark(7, Change::Removed, &esc(id));
        }
        for send in &pending.added {
            let text = format!(
                "{} {} deadline {}ms",
                esc(&send.sendid),
                message_text(send),
                send.deadline_ms
            );
            out.mark(7, Change::Added, &text);
        }
    }
    if let Some(invocations) = &session.invocations {
        out.block(3, "invocations", "");
        for id in &invocations.removed {
            out.mark(7, Change::Removed, &esc(id));
        }
        for id in &invocations.added {
            out.mark(7, Change::Added, &esc(id));
        }
        for change in invocations.changed.iter().flatten() {
            let text = format!(
                "{}: {} -> {}",
                esc(&change.invoke_id),
                invocation_text(&change.from),
                invocation_text(&change.to)
            );
            out.mark(7, Change::Changed, &text);
        }
    }
    if let Some(transitions) = &session.transitions {
        out.block(3, "transitions", "");
        for key in &transitions.removed {
            out.mark(
                7,
                Change::Removed,
                &format!("{}[{}]", esc(&key.source), key.index),
            );
        }
        for key in &transitions.added {
            out.mark(
                7,
                Change::Added,
                &format!("{}[{}]", esc(&key.source), key.index),
            );
        }
        for change in transitions.changed.iter().flatten() {
            out.mark(
                7,
                Change::Changed,
                &format!("{}[{}]", esc(&change.source), change.index),
            );
            for (marker, transition) in
                [(Change::Removed, &change.from), (Change::Added, &change.to)]
            {
                out.mark(11, marker, &transition_text(transition));
                if let Some(previews) = previews_text(transition) {
                    out.line(13, &previews);
                }
            }
        }
    }
    if let Some(reordered) = &session.reordered {
        let names: Vec<&str> = reordered.iter().map(|list| list_text(*list)).collect();
        out.block(3, "reordered", &names.join(", "));
    }
}

fn change_text(name: &str, from: Option<String>, to: Option<String>) -> String {
    match (from, to) {
        (Some(from), Some(to)) => format!("{name}: {from} -> {to}"),
        (None, Some(to)) => format!("{name} = {to}"),
        (Some(from), None) => format!("{name} (was {from})"),
        (None, None) => name.to_owned(),
    }
}

fn fifo_lines<T>(out: &mut Out, label: &str, fifo: Option<&FifoDiff<T>>, text: fn(&T) -> String) {
    let Some(FifoDiff { consumed, appended }) = fifo else {
        return;
    };
    if *consumed > 0 {
        out.mark(7, Change::Removed, &format!("{label}: {consumed} consumed"));
    }
    for item in appended {
        out.mark(7, Change::Added, &format!("{label}: {}", text(item)));
    }
}
