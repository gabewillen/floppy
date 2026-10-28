//! Capture → canonical document bytes.

use sha2::{Digest as _, Sha256};

use crate::validate::{Pointer, validate_document};
use crate::{
    Capture, Children, Clock, Configuration, Counters, Document, Event, Invocation, IrIdentity,
    MAX_DOCUMENT_BYTES, MAX_INVOKE_DEPTH, MAX_INVOKED_SESSIONS, Options, Queues,
    SESSION_SNAPSHOT_VERSION, Section, Session, SessionCapture, Snapshot, SnapshotError,
    SnapshotErrorCode, TraceContextPolicy, Transition,
};

/// Machine IR schema version every session loads (`ir.version`).
pub(crate) const MACHINE_IR_VERSION: u32 = 5;

/// Encodes one checkpoint's capture as canonical document bytes.
///
/// `options.exclude` is normalized (sorted, duplicates dropped). Excluded
/// sections are omitted; a data-model image the host did not gather (the
/// session is `ready`, or no data model is linked) omits `system` and
/// `data_model`. Every session's invocations are listed in ascending
/// invoke-id order, each with the session it invoked, at any depth up to
/// [`MAX_INVOKE_DEPTH`].
/// Under [`TraceContextPolicy::Strip`] every carrier is dropped.
///
/// The produced document passes exactly the checks [`crate::decode`] runs,
/// so a capture that could not be decoded is rejected here instead. Error
/// pointers name the capture (`/sessions/<i>/…`) for invocation-shape errors and
/// the document that would have been produced (`/snapshot/…`) otherwise.
///
/// # Errors
///
/// Any [`SnapshotErrorCode`] the decoder checks, plus
/// [`SnapshotErrorCode::DepthExceeded`] and
/// [`SnapshotErrorCode::Inconsistent`] for malformed invocations.
pub fn encode(capture: &Capture, options: &Options) -> Result<Vec<u8>, SnapshotError> {
    let mut options = options.clone();
    options.exclude.sort_unstable();
    options.exclude.dedup();
    let snapshot = build_snapshot(capture, &options)?;
    let document = seal(options, snapshot)?;
    let mut issues = Vec::new();
    validate_document(&document, &mut issues);
    if let Some(first) = issues.into_iter().next() {
        return Err(first);
    }
    let bytes = serde_json::to_vec(&document).map_err(|_| schema_violation())?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(SnapshotError::new(SnapshotErrorCode::TooLarge, None));
    }
    Ok(bytes)
}

/// Wraps a snapshot in its envelope with the digest of its canonical bytes.
fn seal(options: Options, snapshot: Snapshot) -> Result<Document, SnapshotError> {
    let bytes = serde_json::to_vec(&snapshot).map_err(|_| schema_violation())?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(SnapshotError::new(SnapshotErrorCode::TooLarge, None));
    }
    Ok(Document {
        version: SESSION_SNAPSHOT_VERSION.into(),
        sha256: sha256_hex(&bytes),
        options,
        snapshot,
    })
}

/// Lowercase hex SHA-256.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest {
        // Discarded: `fmt::Write for String` never returns an error.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

const fn schema_violation() -> SnapshotError {
    SnapshotError::new(SnapshotErrorCode::SchemaViolation, None)
}

fn capture_error(code: SnapshotErrorCode, index: usize, key: &'static str) -> SnapshotError {
    let mut pointer = Pointer::root();
    pointer.push_key("sessions");
    pointer.push_index(index);
    pointer.push_key(key);
    SnapshotError::new(code, Some(pointer.into_string()))
}

fn build_snapshot(capture: &Capture, options: &Options) -> Result<Snapshot, SnapshotError> {
    let Some(root_capture) = capture.sessions.first() else {
        return Err(SnapshotError::new(
            SnapshotErrorCode::Inconsistent,
            Some("/sessions".into()),
        ));
    };
    if capture.sessions.len().saturating_sub(1) > MAX_INVOKED_SESSIONS {
        return Err(SnapshotError::new(
            SnapshotErrorCode::BoundsExceeded,
            Some("/sessions".into()),
        ));
    }
    if root_capture.parent.is_some() {
        return Err(capture_error(SnapshotErrorCode::Inconsistent, 0, "parent"));
    }
    let children_section = Children::under(&options.exclude);
    // Pre-order: every session's parent comes before it; depths follow.
    let mut depths = vec![0_usize; capture.sessions.len()];
    for (index, session) in capture.sessions.iter().enumerate().skip(1) {
        let parent = match session.parent {
            Some(parent) if children_section == Children::Included => usize::try_from(parent)
                .ok()
                .filter(|parent| *parent < index)
                .ok_or_else(|| capture_error(SnapshotErrorCode::Inconsistent, index, "parent"))?,
            _ => {
                return Err(capture_error(
                    SnapshotErrorCode::Inconsistent,
                    index,
                    "parent",
                ));
            }
        };
        let depth = depths.get(parent).copied().unwrap_or(0) + 1;
        if depth > MAX_INVOKE_DEPTH {
            return Err(capture_error(
                SnapshotErrorCode::DepthExceeded,
                index,
                "parent",
            ));
        }
        if let Some(slot) = depths.get_mut(index) {
            *slot = depth;
        }
    }
    let mut used = vec![false; capture.sessions.len()];
    let root = build_invoked(capture, 0, options, children_section, &mut used)?;
    if let Some(orphan) = used.iter().skip(1).position(|used| !used) {
        return Err(capture_error(
            SnapshotErrorCode::Inconsistent,
            orphan + 1,
            "machine",
        ));
    }
    let mut components = capture.components.clone();
    components.sort_by_key(|component| component.role);
    Ok(Snapshot {
        components,
        clock: Clock {
            now_ms: capture.now_ms,
        },
        root,
    })
}

/// The session at `index` with the sessions it invoked, matched to its
/// machine's live invocations by invoke id, ascending, at any depth.
fn build_invoked(
    capture: &Capture,
    index: usize,
    options: &Options,
    children_section: Children,
    used: &mut [bool],
) -> Result<Session, SnapshotError> {
    let Some(session_capture) = capture.sessions.get(index) else {
        return Err(capture_error(
            SnapshotErrorCode::Inconsistent,
            index,
            "parent",
        ));
    };
    let mut session = build_session(session_capture, options);
    if children_section == Children::Excluded {
        return Ok(session);
    }
    let children: Vec<usize> = capture
        .sessions
        .iter()
        .enumerate()
        .filter(|(_, child)| {
            child.parent.and_then(|parent| usize::try_from(parent).ok()) == Some(index)
        })
        .map(|(child, _)| child)
        .collect();
    let mut live = session_capture.machine.invocations.clone();
    live.sort_by(|left, right| left.invoke_id.cmp(&right.invoke_id));
    let mut invocations = Vec::with_capacity(live.len());
    for invocation in live {
        let found = children.iter().copied().find(|child| {
            capture.sessions.get(*child).is_some_and(|child| {
                child
                    .machine
                    .invoked_by
                    .as_ref()
                    .is_some_and(|by| by.invoke_id == invocation.invoke_id)
            })
        });
        let Some(child) = found else {
            let mut pointer = Pointer::root();
            pointer.push_key("sessions");
            pointer.push_index(index);
            pointer.push_key("machine");
            pointer.push_key("invocations");
            return Err(SnapshotError::new(
                SnapshotErrorCode::Inconsistent,
                Some(pointer.into_string()),
            ));
        };
        if used.get(child).copied().unwrap_or(true) {
            return Err(capture_error(
                SnapshotErrorCode::Inconsistent,
                child,
                "machine",
            ));
        }
        if let Some(slot) = used.get_mut(child) {
            *slot = true;
        }
        let finished = capture
            .sessions
            .get(child)
            .is_some_and(|child| child.finished);
        invocations.push(Invocation {
            invoke_id: invocation.invoke_id,
            invoke_type: invocation.type_uri,
            state: invocation.state,
            index: invocation.index,
            autoforward: invocation.autoforward,
            finished,
            session: Box::new(build_invoked(
                capture,
                child,
                options,
                children_section,
                used,
            )?),
        });
    }
    session.invocations = Some(invocations);
    Ok(session)
}

#[allow(
    clippy::too_many_lines,
    reason = "one struct literal over the session keys in document order"
)]
fn build_session(capture: &SessionCapture, options: &Options) -> Session {
    let excluded = |section| options.exclude.contains(&section);
    let strip = options.trace_context == TraceContextPolicy::Strip;
    let event = |event: &Event| {
        let mut event = event.clone();
        if strip {
            event.trace_context = None;
        }
        event
    };
    let events = |list: &[Event]| list.iter().map(event).collect::<Vec<_>>();
    let machine = &capture.machine;
    let data_model = capture.data_model.as_ref();
    let pending_sends: Vec<_> = capture
        .pending_sends
        .iter()
        .map(|pending| {
            let mut pending = pending.clone();
            pending.event = pending.event.as_ref().map(event);
            if strip && let Some(message) = &mut pending.message {
                message.trace_context = None;
            }
            pending
        })
        .collect();
    let previews = !excluded(Section::Previews);
    Session {
        session_id: capture.session_id.clone(),
        name: machine.name.clone(),
        datamodel_name: capture.data_model_name.clone(),
        ir: IrIdentity {
            version: MACHINE_IR_VERSION,
            sha256: capture.ir_sha256.clone(),
        },
        status: machine.status,
        invoked_by: machine.invoked_by.clone(),
        configuration: Configuration {
            atomic: machine.atomic.clone(),
            full: machine.configuration.clone(),
        },
        history: machine.history.clone(),
        bound_states: machine.bound_states.clone(),
        counters: Counters {
            send: machine.send_counter,
            invoke: machine.invoke_counter,
        },
        system: data_model
            .filter(|_| !excluded(Section::System))
            .map(|model| model.system.clone()),
        data_model: data_model
            .filter(|_| !excluded(Section::DataModel))
            .map(|model| model.variables.clone()),
        queues: (!excluded(Section::Queues)).then(|| Queues {
            internal: events(&machine.internal),
            external: events(&capture.external),
        }),
        transitions: (!excluded(Section::Transitions)).then(|| {
            machine
                .transitions
                .iter()
                .map(|transition| Transition {
                    source: transition.source.clone(),
                    index: transition.index,
                    events: transition.events.clone(),
                    targets: transition.targets.clone(),
                    transition_type: transition.transition_type,
                    cond: transition.cond.clone(),
                    has_cond: transition.cond.is_some(),
                    domain: transition.domain.clone(),
                    exit: previews.then(|| transition.exit.clone()),
                    entry: previews.then(|| transition.entry.clone()),
                    event_schema: transition.event_schema.clone(),
                })
                .collect()
        }),
        pending_sends: (!excluded(Section::PendingSends)).then_some(pending_sends),
        invocations: None,
    }
}
