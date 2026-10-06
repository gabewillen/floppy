//! Read-only session snapshots at scheduler checkpoints (see
//! `docs/session-snapshot.md`, "When a snapshot can be taken").
//!
//! At a requested checkpoint the host gathers, in invocation order (the root, then
//! each session it invoked, in ascending invoke-id order, followed by the
//! sessions that one invoked, at any depth), each session's machine,
//! data-model, I/O-processor, and clock `snapshot` exports plus its external
//! event queue, then encodes the capture with the native
//! [`scxml_snapshot::encode`] inside one `session-snapshot` span. The
//! exports are pure reads: gathering charges no command, records no trace
//! entry, and changes no session state. Excluded sections are never
//! collected. With [`SnapshotPoints::None`] nothing here runs.

use std::collections::{BTreeMap, VecDeque};

use scxml_scheduler::{Checkpoint, Delivery, Invoked, SessionKey};
use scxml_snapshot::{
    BasicHttpMessage, Capture, ComponentIdentity, ComponentRole, DataModelCapture, Event,
    EventType, HistoryValue, InvocationCapture, InvokedBy, MachineCapture, Options, OversizedValue,
    Param, PendingSend, Section, SessionCapture, SnapshotError, SnapshotErrorCode, SnapshotPoint,
    Status as SnapshotStatus, System, TextValue, TraceContext, TraceContextPolicy,
    TransitionCapture, TransitionType, Value, Variable, VariableValue,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};

use super::RunFailure;
use super::run::{MachineHandle, RunInvokedSessions, RunSessions};
use crate::ComponentMeasurement;
use crate::bindings::exports::stateforward::scxml::machine_api::{
    MachineSnapshot, SnapshotOptions as MachineSnapshotOptions, Status,
    TransitionType as WitTransitionType,
};
use crate::bindings::stateforward::scxml::machine_capability_types::EventSchema as WitEventSchema;
use crate::bindings::stateforward::scxml::types::{
    BaggagePolicy as WitBaggagePolicy, ComponentError, DataValue as WitDataValue,
    Event as WitEvent, EventType as WitEventType, TraceContext as WitTraceContext,
};
use crate::data_model::{DataModelRead, DataModelSnapshot};
use crate::io_bindings::exports::stateforward::scxml::io_processor_api::{
    DelayedSend as WitDelayedSend, PendingSend as WitPendingSend,
};

/// Which checkpoints of a run are snapshotted (the checkpoint ordinals of
/// `docs/session-snapshot.md`, "When a snapshot can be taken": 0 after
/// `start`, one per scripted input or external-client arrival, one at the
/// top of each drain iteration, and a final one at the end).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SnapshotPoints {
    /// No snapshot; the run calls nothing snapshot-related.
    #[default]
    None,
    /// Exactly these ordinals, strictly ascending. The run fails with
    /// [`SnapshotFailure::Unreached`] when one is never reached.
    At(Vec<u32>),
    /// Every checkpoint.
    Every,
}

/// What a run snapshots and how the documents are produced.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SnapshotRequest {
    /// The checkpoints to snapshot.
    pub points: SnapshotPoints,
    /// Exclusions and trace-carrier policy of every document.
    pub options: Options,
}

impl SnapshotRequest {
    /// No snapshots (the default).
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }
}

/// One canonical `scxml.session-snapshot/v2` document taken at a checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointSnapshot {
    /// The checkpoint ordinal.
    pub checkpoint: u32,
    /// Canonical document bytes.
    pub bytes: Vec<u8>,
}

/// A requested checkpoint snapshot that could not be taken. The run goes
/// on: a snapshot failure never fails or ends a session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointSnapshotFailure {
    /// The checkpoint ordinal.
    pub checkpoint: u32,
    /// Why the snapshot failed (for example a session over a bound of the
    /// `scxml.session-snapshot/v2` document).
    pub failure: SnapshotFailure,
}

/// A component's read-only snapshot export.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotExport {
    /// `machine-api.session.snapshot`.
    Machine,
    /// `data-model-api.snapshot`.
    DataModel,
    /// `io-processor-api.snapshot`.
    IoProcessor,
}

impl SnapshotExport {
    /// The WIT function name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Machine => "machine-api.session.snapshot",
            Self::DataModel => "data-model-api.snapshot",
            Self::IoProcessor => "io-processor-api.snapshot",
        }
    }
}

/// Why a snapshot failed. A requested checkpoint that is never reached
/// ([`Self::Unreached`]) and invalid points ([`Self::InvalidPoints`]) fail
/// the run; any other failure is that one snapshot's alone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapshotFailure {
    /// A component export answered an error: the closed WIT code label and
    /// the component's bounded static detail.
    Export {
        /// The failing export.
        export: SnapshotExport,
        /// Closed WIT `error-code` label.
        code: &'static str,
        /// The component's bounded detail.
        detail: Option<String>,
    },
    /// The gathered records do not form a valid document (the codec's, or
    /// the host's own check of the same invariant).
    Codec(SnapshotError),
    /// A requested checkpoint was never reached.
    Unreached(u32),
    /// [`SnapshotPoints::At`] was not strictly ascending.
    InvalidPoints,
    /// The run holds a live invoked service (SCXML 6.4), whose state
    /// `scxml.session-snapshot/v2` cannot represent: the snapshot is refused
    /// (`unavailable`) rather than leaving the service out.
    ServiceInvocation,
}

impl std::fmt::Display for SnapshotFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Export { export, code, .. } => {
                write!(formatter, "{} failed: {code}", export.as_str())
            }
            Self::Codec(error) => error.fmt(formatter),
            Self::Unreached(checkpoint) => {
                write!(formatter, "checkpoint {checkpoint} was never reached")
            }
            Self::InvalidPoints => {
                formatter.write_str("snapshot checkpoints must be strictly ascending")
            }
            Self::ServiceInvocation => formatter.write_str(
                "the session holds an invoked service, which a session snapshot cannot represent",
            ),
        }
    }
}

impl From<SnapshotFailure> for RunFailure {
    fn from(failure: SnapshotFailure) -> Self {
        Self::Snapshot(failure)
    }
}

fn codec(code: SnapshotErrorCode) -> RunFailure {
    SnapshotFailure::Codec(SnapshotError {
        code,
        pointer: None,
    })
    .into()
}

fn export_failure(export: SnapshotExport, error: ComponentError) -> RunFailure {
    SnapshotFailure::Export {
        export,
        code: crate::component::error_code_label(error.code),
        detail: error.detail,
    }
    .into()
}

/// Release digests of the components a snapshot names.
pub(crate) struct ComponentDigests {
    pub(crate) machine: String,
    pub(crate) clock: String,
    pub(crate) io_scxml: String,
    pub(crate) ecmascript: String,
    pub(crate) xpath: String,
    pub(crate) null: String,
}

/// Takes the requested snapshots of one run, one checkpoint at a time.
pub(crate) struct Snapshotter<'a> {
    request: &'a SnapshotRequest,
    digests: &'a ComponentDigests,
    providers: &'a crate::TelemetryProviders,
    /// Index of the next unreached ordinal in [`SnapshotPoints::At`].
    next_at: usize,
    taken: Vec<CheckpointSnapshot>,
    failed: Vec<CheckpointSnapshotFailure>,
}

impl<'a> Snapshotter<'a> {
    /// A snapshotter for `request`; `None` for [`SnapshotPoints::None`].
    ///
    /// # Errors
    ///
    /// [`SnapshotFailure::InvalidPoints`] for ordinals that are not strictly
    /// ascending.
    pub(crate) fn new(
        request: &'a SnapshotRequest,
        digests: &'a ComponentDigests,
        providers: &'a crate::TelemetryProviders,
    ) -> Result<Option<Self>, RunFailure> {
        match &request.points {
            SnapshotPoints::None => return Ok(None),
            SnapshotPoints::At(points) if points.windows(2).any(|pair| pair[0] >= pair[1]) => {
                return Err(SnapshotFailure::InvalidPoints.into());
            }
            SnapshotPoints::At(_) | SnapshotPoints::Every => {}
        }
        Ok(Some(Self {
            request,
            digests,
            providers,
            next_at: 0,
            taken: Vec::new(),
            failed: Vec::new(),
        }))
    }

    fn requested(&mut self, ordinal: u32) -> bool {
        match &self.request.points {
            SnapshotPoints::None => false,
            SnapshotPoints::Every => true,
            SnapshotPoints::At(points) => {
                if points.get(self.next_at) == Some(&ordinal) {
                    self.next_at += 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// The scheduler hook body: snapshots `checkpoint` when requested. A
    /// [`RunFailure::Snapshot`] is recorded for that checkpoint and the run
    /// goes on; only a host failure of an export call ends the run.
    pub(crate) fn at(
        &mut self,
        sessions: &mut RunSessions,
        invoked_sessions: &RunInvokedSessions,
        checkpoint: &Checkpoint<'_, WitEvent>,
    ) -> Result<(), RunFailure> {
        if !self.requested(checkpoint.ordinal) {
            return Ok(());
        }
        match take(
            self.digests,
            self.providers,
            &self.request.options,
            sessions,
            invoked_sessions,
            checkpoint.arrivals,
        ) {
            Ok(bytes) => self.taken.push(CheckpointSnapshot {
                checkpoint: checkpoint.ordinal,
                bytes,
            }),
            Err(RunFailure::Snapshot(failure)) => self.failed.push(CheckpointSnapshotFailure {
                checkpoint: checkpoint.ordinal,
                failure,
            }),
            Err(failure) => return Err(failure),
        }
        Ok(())
    }

    /// The documents taken and the snapshots that failed, after checking
    /// every requested ordinal was reached.
    pub(crate) fn finish(
        self,
    ) -> Result<(Vec<CheckpointSnapshot>, Vec<CheckpointSnapshotFailure>), RunFailure> {
        if let SnapshotPoints::At(points) = &self.request.points
            && let Some(unreached) = points.get(self.next_at)
        {
            return Err(SnapshotFailure::Unreached(*unreached).into());
        }
        Ok((self.taken, self.failed))
    }
}

/// Takes one read-only session snapshot of a stable run inside one
/// `session-snapshot` span: the root (with `arrivals` as its host-held
/// external queue ahead of its own), and every session it invoked, each
/// with its external event queue.
///
/// # Errors
///
/// [`RunFailure::Snapshot`] for a failed export or an invalid capture, or a
/// host failure of an export call.
pub(crate) fn take(
    digests: &ComponentDigests,
    providers: &crate::TelemetryProviders,
    options: &Options,
    sessions: &mut RunSessions,
    invoked_sessions: &RunInvokedSessions,
    arrivals: Option<&VecDeque<WitEvent>>,
) -> Result<Vec<u8>, RunFailure> {
    let span =
        scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeSessionSnapshot);
    match capture(digests, options, sessions, invoked_sessions, arrivals) {
        Ok(bytes) => {
            span.event(SignalEvent::ArtifactProduced);
            providers.record_component_measurement(
                ComponentMeasurement::SnapshotBytes,
                u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            );
            Ok(bytes)
        }
        Err(failure) => {
            span.fail(match &failure {
                RunFailure::Snapshot(SnapshotFailure::Export { .. }) => ErrorCategory::Execution,
                RunFailure::Snapshot(_) => ErrorCategory::Validation,
                _ => ErrorCategory::Platform,
            });
            Err(failure)
        }
    }
}

fn capture(
    digests: &ComponentDigests,
    options: &Options,
    sessions: &mut RunSessions,
    invoked_sessions: &RunInvokedSessions,
    arrivals: Option<&VecDeque<WitEvent>>,
) -> Result<Vec<u8>, RunFailure> {
    let gather = Gather { options };
    // Invocation order: every session after its invoking session.
    let order = if gather.excluded(Section::Invocations) {
        vec![SessionKey::ROOT]
    } else {
        within_document_bounds(invoked_sessions)?
    };
    let finished: Vec<bool> = {
        let records = sessions.shared().sessions.borrow();
        order
            .iter()
            .map(|key| !key.is_root() && records.get(*key).is_some_and(|record| record.finished))
            .collect()
    };
    let mut handles = sessions.synced_handles()?;
    let mut captured = Vec::with_capacity(order.len());
    let mut index_of: BTreeMap<SessionKey, u32> = BTreeMap::new();
    let mut now_ms = 0;
    for (key, finished) in order.into_iter().zip(finished) {
        let parent = invoked_sessions
            .parent(key)
            .map(|(parent, _)| {
                index_of
                    .get(&parent)
                    .copied()
                    .ok_or_else(|| codec(SnapshotErrorCode::Inconsistent))
            })
            .transpose()?;
        let handle = handles
            .get_mut(&key)
            .ok_or_else(|| codec(SnapshotErrorCode::Inconsistent))?;
        let mut capture = gather.session(handle, parent, &mut now_ms)?;
        capture.finished = finished;
        if !gather.excluded(Section::Queues) {
            let queued = gather.queue(invoked_sessions, key);
            capture.external = if key.is_root() {
                arrivals
                    .into_iter()
                    .flat_map(VecDeque::iter)
                    .map(|event| gather.event(event))
                    .chain(queued)
                    .collect()
            } else {
                queued
            };
        }
        let index =
            u32::try_from(captured.len()).map_err(|_| codec(SnapshotErrorCode::BoundsExceeded))?;
        index_of.insert(key, index);
        captured.push(capture);
    }
    let capture = Capture {
        components: components(digests, &captured),
        now_ms,
        sessions: captured,
    };
    scxml_snapshot::encode(&capture, options).map_err(|error| SnapshotFailure::Codec(error).into())
}

/// The run's sessions in invocation order when a
/// `scxml.session-snapshot/v2` document can hold them: at most
/// [`scxml_snapshot::MAX_INVOKED_SESSIONS`] invoked sessions, none deeper
/// than [`scxml_snapshot::MAX_INVOKE_DEPTH`] (the format's bounds, with the
/// error [`scxml_snapshot::encode`] reports for them). Checked before any
/// session is read, so a run of any size fails at once.
fn within_document_bounds(
    invoked_sessions: &RunInvokedSessions,
) -> Result<Vec<SessionKey>, RunFailure> {
    if invoked_sessions.invoked_len() > scxml_snapshot::MAX_INVOKED_SESSIONS {
        return Err(SnapshotFailure::Codec(SnapshotError {
            code: SnapshotErrorCode::BoundsExceeded,
            pointer: Some("/sessions".into()),
        })
        .into());
    }
    let order = invoked_sessions.keys();
    if let Some(index) = order.iter().position(|key| {
        invoked_sessions
            .depth(*key)
            .is_some_and(|depth| depth > scxml_snapshot::MAX_INVOKE_DEPTH)
    }) {
        return Err(SnapshotFailure::Codec(SnapshotError {
            code: SnapshotErrorCode::DepthExceeded,
            pointer: Some(format!("/sessions/{index}/parent")),
        })
        .into());
    }
    Ok(order)
}

/// Provenance: clock, the data models linked in the root session and its invoked sessions, the SCXML
/// processor, and the machine, sorted by role.
fn components(digests: &ComponentDigests, sessions: &[SessionCapture]) -> Vec<ComponentIdentity> {
    let mut roles = BTreeMap::from([
        (ComponentRole::Clock, &digests.clock),
        (ComponentRole::IoProcessorScxml, &digests.io_scxml),
        (ComponentRole::Machine, &digests.machine),
    ]);
    for name in sessions
        .iter()
        .filter_map(|session| session.data_model_name.as_deref())
    {
        match name {
            "ecmascript" => roles.insert(ComponentRole::DataModelEcmascript, &digests.ecmascript),
            "xpath" => roles.insert(ComponentRole::DataModelXpath, &digests.xpath),
            "null" => roles.insert(ComponentRole::DataModelNull, &digests.null),
            _ => None,
        };
    }
    roles
        .into_iter()
        .map(|(role, sha256)| ComponentIdentity {
            role,
            sha256: sha256.clone(),
        })
        .collect()
}

/// What one machine store adds to the machine's executable-content image for
/// an extension component's `session-snapshot-host.snapshot`
/// (`docs/session-snapshot.md`, "Snapshots during executable content").
pub(crate) struct ExtensionImage {
    /// The machine's image at the executing effect.
    pub(crate) machine: MachineSnapshot,
    /// SHA-256 of the Machine IR bytes the session loaded.
    pub(crate) ir_sha256: String,
    /// Whether the machine ever initialized the linked data model.
    pub(crate) data_model_initialized: bool,
}

/// The executing session's state an executable-content snapshot reads besides
/// the machine image.
pub(crate) struct ExtensionState<'a> {
    /// The session's data model, lent to the extension component.
    pub(crate) data_model: Option<&'a mut Box<dyn crate::session::MachineDataModel>>,
    /// The session's links, lent to the extension component.
    pub(crate) links: &'a mut super::SessionLinks,
    /// Internal events the extension queued so far, in call order.
    pub(crate) internal: &'a [WitEvent],
    /// Self-addressed external events the extension sent so far.
    pub(crate) external: &'a [WitEvent],
}

/// The canonical `scxml.session-snapshot/v2` document of one executing session
/// (`docs/session-snapshot.md`, "Snapshots during executable content"): its
/// session object only, `invocations` and `previews` always excluded besides
/// `exclude`, trace carriers stripped. The data model, pending sends, and clock
/// are read now; the queues list the extension's own queued events after the
/// session's.
///
/// # Errors
///
/// [`RunFailure::Snapshot`] for a failed export or a capture outside the
/// format's bounds, or a host failure of an export call.
pub(crate) fn extension_document(
    image: &ExtensionImage,
    exclude: &[Section],
    state: ExtensionState<'_>,
) -> Result<Vec<u8>, RunFailure> {
    let mut sections: Vec<Section> = exclude
        .iter()
        .copied()
        .chain([Section::Invocations, Section::Previews])
        .collect();
    sections.sort_unstable();
    sections.dedup();
    let options = Options {
        exclude: sections,
        trace_context: TraceContextPolicy::Strip,
        point: SnapshotPoint::ExecutableContent,
    };
    let gather = Gather { options: &options };
    let mut machine = gather.machine(image.machine.clone())?;
    if gather.excluded(Section::Transitions) {
        machine.transitions.clear();
    } else {
        // The image always lists the walk; an executable-content image has no
        // previews.
        for transition in &mut machine.transitions {
            transition.exit.clear();
            transition.entry.clear();
        }
    }
    machine
        .internal
        .extend(state.internal.iter().map(|event| gather.event(event)));
    let wants_data_model =
        !(gather.excluded(Section::System) && gather.excluded(Section::DataModel));
    let (data_model_name, data_model) =
        match state.data_model.and_then(|model| model.as_component()) {
            Some(component) if image.data_model_initialized => {
                let name = component.component_name().to_owned();
                if wants_data_model && machine.status != SnapshotStatus::Ready {
                    let snapshot = component
                        .forward(|api, store| api.call_snapshot(store))
                        .map_err(|error| export_failure(SnapshotExport::DataModel, error))?;
                    (Some(name), Some(data_model(snapshot)))
                } else {
                    (Some(name), None)
                }
            }
            _ => (None, None),
        };
    let links = state.links;
    let now_ms = links.clock_snapshot()?.now_ms;
    let pending_sends = if gather.excluded(Section::PendingSends) {
        Vec::new()
    } else {
        links
            .io_snapshot()?
            .map_err(|error| export_failure(SnapshotExport::IoProcessor, error))?
            .into_iter()
            .map(|pending| gather.pending_send(pending))
            .collect::<Result<_, _>>()?
    };
    let external = if gather.excluded(Section::Queues) {
        Vec::new()
    } else {
        links
            .with_invoked_sessions(|invoked_sessions, key| gather.queue(invoked_sessions, key))
            .into_iter()
            .chain(state.external.iter().map(|event| gather.event(event)))
            .collect()
    };
    let session = SessionCapture {
        parent: None,
        session_id: links.session_id().to_owned(),
        ir_sha256: image.ir_sha256.clone(),
        data_model_name,
        machine,
        data_model,
        pending_sends,
        external,
        finished: false,
    };
    let sessions = vec![session];
    let capture = Capture {
        components: components(links.digests(), &sessions),
        now_ms,
        sessions,
    };
    scxml_snapshot::encode(&capture, &options).map_err(|error| SnapshotFailure::Codec(error).into())
}

/// Converts component records into capture records under one options set.
struct Gather<'a> {
    options: &'a Options,
}

impl Gather<'_> {
    fn excluded(&self, section: Section) -> bool {
        self.options.exclude.contains(&section)
    }

    /// One session's component records; its external queue is filled by
    /// the caller.
    /// `now_ms` receives the root's clock and is checked for a child.
    fn session(
        &self,
        handle: &mut MachineHandle,
        parent: Option<u32>,
        now_ms: &mut u64,
    ) -> Result<SessionCapture, RunFailure> {
        // A `failed` or `shut-down` session is never snapshotted. Its status
        // is read first, so the failure is `status-invalid` and never the
        // export's own answer for a session it no longer serves.
        match handle.status()? {
            Status::Failed | Status::ShutDown => {
                return Err(codec(SnapshotErrorCode::StatusInvalid));
            }
            Status::Ready | Status::Running | Status::Finished => {}
        }
        let transitions = !self.excluded(Section::Transitions);
        let machine = handle
            .snapshot(MachineSnapshotOptions {
                transitions,
                previews: transitions && !self.excluded(Section::Previews),
            })?
            .map_err(|error| export_failure(SnapshotExport::Machine, error))?;
        let machine = self.machine(machine)?;
        let wants_data_model =
            !(self.excluded(Section::System) && self.excluded(Section::DataModel));
        let read = if wants_data_model && machine.status != SnapshotStatus::Ready {
            DataModelRead::Snapshot
        } else {
            DataModelRead::NameOnly
        };
        let (data_model_name, data_model) = match handle.data_model_snapshot(read) {
            None => (None, None),
            Some((name, None)) => (Some(name), None),
            Some((name, Some(result))) => (
                Some(name),
                Some(data_model(result.map_err(|error| {
                    export_failure(SnapshotExport::DataModel, error)
                })?)),
            ),
        };
        let ir_sha256 = handle.machine_ir_sha256();
        let links = handle.links_mut()?;
        let clock = links.clock_snapshot()?;
        match parent {
            None => *now_ms = clock.now_ms,
            Some(_) if clock.now_ms != *now_ms => {
                return Err(codec(SnapshotErrorCode::ClockInvariant));
            }
            Some(_) => {}
        }
        let pending_sends = if self.excluded(Section::PendingSends) {
            Vec::new()
        } else {
            links
                .io_snapshot()?
                .map_err(|error| export_failure(SnapshotExport::IoProcessor, error))?
                .into_iter()
                .map(|pending| self.pending_send(pending))
                .collect::<Result<_, _>>()?
        };
        Ok(SessionCapture {
            parent,
            session_id: links.session_id().to_owned(),
            ir_sha256,
            data_model_name,
            machine,
            data_model,
            pending_sends,
            external: Vec::new(),
            finished: false,
        })
    }

    /// The external event queue of the session `key`, oldest first:
    /// a message from an invocation appears as the event the session will
    /// process (stamped with its `invokeid`; `done.invoke.<id>` for a
    /// completion; `error.communication` for a retired service).
    fn queue(&self, invoked_sessions: &RunInvokedSessions, key: SessionKey) -> Vec<Event> {
        invoked_sessions
            .external(key)
            .into_iter()
            .flatten()
            .map(|delivery| match delivery {
                Delivery::External(event) => self.event(event),
                Delivery::FromInvocation {
                    invoke_id,
                    message: Invoked::Event(event),
                } => {
                    let mut event = event.clone();
                    event.invoke_id = Some(invoke_id.clone());
                    self.event(&event)
                }
                Delivery::FromInvocation {
                    invoke_id,
                    message: Invoked::Done(done),
                } => self.event(&WitEvent {
                    name: format!("done.invoke.{invoke_id}"),
                    event_type: WitEventType::External,
                    send_id: None,
                    origin: None,
                    origin_type: None,
                    invoke_id: Some(invoke_id.clone()),
                    data: done
                        .as_ref()
                        .and_then(|done| done.data.clone())
                        .unwrap_or(WitDataValue::Absent),
                    raw: None,
                    trace_context: done.as_ref().and_then(|done| done.trace_context.clone()),
                }),
                // A retired invoked service's failure: the session raises
                // `error.communication` carrying the invoke id.
                Delivery::FromInvocation {
                    invoke_id,
                    message: Invoked::Failed,
                } => self.event(&WitEvent {
                    name: "error.communication".to_owned(),
                    event_type: WitEventType::Platform,
                    send_id: None,
                    origin: None,
                    origin_type: None,
                    invoke_id: Some(invoke_id.clone()),
                    data: WitDataValue::Absent,
                    raw: None,
                    trace_context: None,
                }),
            })
            .collect()
    }

    fn machine(&self, snapshot: MachineSnapshot) -> Result<MachineCapture, RunFailure> {
        Ok(MachineCapture {
            status: match snapshot.status {
                Status::Ready => SnapshotStatus::Ready,
                Status::Running => SnapshotStatus::Running,
                Status::Finished => SnapshotStatus::Finished,
                Status::Failed | Status::ShutDown => {
                    return Err(codec(SnapshotErrorCode::StatusInvalid));
                }
            },
            name: snapshot.name,
            atomic: snapshot.atomic,
            configuration: snapshot.configuration,
            internal: snapshot
                .internal
                .iter()
                .map(|event| self.event(event))
                .collect(),
            history: snapshot
                .history
                .into_iter()
                .map(|entry| HistoryValue {
                    state: entry.state,
                    value: entry.value,
                })
                .collect(),
            bound_states: snapshot.bound_states,
            invocations: snapshot
                .invocations
                .into_iter()
                .map(|invocation| InvocationCapture {
                    invoke_id: invocation.invoke_id,
                    state: invocation.state,
                    index: invocation.index,
                    type_uri: invocation.type_uri,
                    autoforward: invocation.autoforward,
                })
                .collect(),
            invoked_by: snapshot.invoked_by.map(|parent| InvokedBy {
                parent_session_id: parent.parent_session_id,
                invoke_id: parent.invoke_id,
            }),
            send_counter: snapshot.send_counter,
            invoke_counter: snapshot.invoke_counter,
            transitions: snapshot
                .transitions
                .into_iter()
                .map(|transition| TransitionCapture {
                    source: transition.source,
                    index: transition.index,
                    events: transition.events,
                    targets: transition.targets,
                    transition_type: match transition.type_ {
                        WitTransitionType::External => TransitionType::External,
                        WitTransitionType::Internal => TransitionType::Internal,
                    },
                    cond: transition.cond,
                    domain: transition.domain,
                    exit: transition.exit,
                    entry: transition.entry,
                    event_schema: transition.event_schema.map(|schema| match schema {
                        WitEventSchema::Schema(text) => scxml_snapshot::EventSchema::Schema {
                            schema: text.schema,
                            sha256: text.sha256,
                        },
                        WitEventSchema::Schemaexpr(expr) => {
                            scxml_snapshot::EventSchema::Schemaexpr { expr }
                        }
                    }),
                })
                .collect(),
        })
    }

    fn pending_send(&self, pending: WitPendingSend) -> Result<PendingSend, RunFailure> {
        match pending.request {
            WitDelayedSend::Scxml(request) => {
                let Some(sendid) = request.send_id else {
                    // A delayed send always carries its id (WIT `send-request`).
                    return Err(codec(SnapshotErrorCode::Inconsistent));
                };
                Ok(PendingSend {
                    sendid,
                    event: Some(self.event(&request.event)),
                    basichttp: None,
                    target: request.target,
                    send_type: request.processor_type,
                    delay_ms: request.delay_ms,
                    deadline_ms: pending.deadline_ms,
                })
            }
            WitDelayedSend::Basichttp(send) => Ok(PendingSend {
                sendid: send.send_id,
                event: None,
                basichttp: Some(BasicHttpMessage {
                    event: send.event_name,
                    params: send
                        .params
                        .iter()
                        .map(|field| Param {
                            name: field.name.clone(),
                            value: value(&field.value),
                        })
                        .collect(),
                    content: send.content.as_ref().map(value),
                    trace_context: send
                        .trace_context
                        .as_ref()
                        .and_then(|context| self.carrier(context)),
                }),
                target: send.target,
                send_type: None,
                delay_ms: send.delay_ms,
                deadline_ms: pending.deadline_ms,
            }),
        }
    }

    /// A held carrier, only under [`TraceContextPolicy::Preserve`].
    fn carrier(&self, context: &WitTraceContext) -> Option<TraceContext> {
        match self.options.trace_context {
            TraceContextPolicy::Strip => None,
            TraceContextPolicy::Preserve => Some(TraceContext {
                traceparent: context.traceparent.clone(),
                tracestate: context.tracestate.clone(),
                baggage: context.baggage.clone(),
                baggage_policy: match context.baggage_policy {
                    WitBaggagePolicy::Drop => scxml_snapshot::BaggagePolicy::Drop,
                    WitBaggagePolicy::Propagate => scxml_snapshot::BaggagePolicy::Propagate,
                },
            }),
        }
    }

    /// One event; its carrier only under [`TraceContextPolicy::Preserve`].
    fn event(&self, event: &WitEvent) -> Event {
        Event {
            name: event.name.clone(),
            event_type: match event.event_type {
                WitEventType::Internal => EventType::Internal,
                WitEventType::External => EventType::External,
                WitEventType::Platform => EventType::Platform,
            },
            origin: event.origin.clone(),
            origintype: event.origin_type.clone(),
            sendid: event.send_id.clone(),
            invokeid: event.invoke_id.clone(),
            data: value(&event.data),
            raw: event.raw.clone(),
            trace_context: event
                .trace_context
                .as_ref()
                .and_then(|context| self.carrier(context)),
        }
    }
}

/// One data-model image as capture records. A value the provider reports
/// as oversized keeps only its byte length.
fn data_model(snapshot: DataModelSnapshot) -> DataModelCapture {
    use crate::data_model::VariableValue as WitVariableValue;
    DataModelCapture {
        system: System {
            sessionid: snapshot.system.session_id,
            name: snapshot.system.machine_name,
            ioprocessors: value(&snapshot.system.io_processors),
        },
        variables: snapshot
            .variables
            .into_iter()
            .map(|variable| Variable {
                name: variable.name,
                value: match variable.value {
                    WitVariableValue::Value(data) => VariableValue::Value(value(&data)),
                    WitVariableValue::Text(text) => {
                        VariableValue::Text(TextValue::new(text.kind, text.text))
                    }
                    WitVariableValue::Oversized(bytes) => {
                        VariableValue::Oversized(OversizedValue::new(bytes))
                    }
                },
            })
            .collect(),
    }
}

/// One WIT value as a document value; a number goes through
/// [`Value::number`], the one number rendering the codec also uses.
fn value(data: &WitDataValue) -> Value {
    match data {
        WitDataValue::Absent => Value::Absent,
        WitDataValue::Undefined => Value::Undefined,
        WitDataValue::Null => Value::Null,
        WitDataValue::Boolean(value) => Value::Boolean(*value),
        WitDataValue::Number(value) => Value::number(*value),
        WitDataValue::Text(value) => Value::Text(value.clone()),
        WitDataValue::JsonArray(value) => Value::JsonArray(value.clone()),
        WitDataValue::JsonObject(value) => Value::JsonObject(value.clone()),
        WitDataValue::Xml(value) => Value::Xml(value.clone()),
    }
}

#[cfg(test)]
mod codec_parity_tests;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_render_as_display_text_without_exponent() {
        for (number, text) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (1.5, "1.5"),
            (1e21, "1000000000000000000000"),
            (1e-7, "0.0000001"),
            (0.1 + 0.2, "0.30000000000000004"),
        ] {
            assert_eq!(
                value(&WitDataValue::Number(number)),
                Value::Number(text.into())
            );
        }
    }

    #[test]
    fn at_points_must_be_strictly_ascending() {
        let digests = ComponentDigests {
            machine: String::new(),
            clock: String::new(),
            io_scxml: String::new(),
            ecmascript: String::new(),
            xpath: String::new(),
            null: String::new(),
        };
        let providers = crate::TelemetryProviders::new();
        for points in [vec![1, 1], vec![2, 1]] {
            let request = SnapshotRequest {
                points: SnapshotPoints::At(points),
                options: Options::default(),
            };
            assert!(matches!(
                Snapshotter::new(&request, &digests, &providers),
                Err(RunFailure::Snapshot(SnapshotFailure::InvalidPoints))
            ));
        }
        assert!(matches!(
            Snapshotter::new(&SnapshotRequest::none(), &digests, &providers),
            Ok(None)
        ));
        let request = SnapshotRequest {
            points: SnapshotPoints::At(vec![0, 3]),
            options: Options::default(),
        };
        let mut snapshotter = Snapshotter::new(&request, &digests, &providers)
            .expect("ascending points are valid")
            .expect("points are requested");
        assert!(snapshotter.requested(0));
        assert!(!snapshotter.requested(1));
        assert!(matches!(
            snapshotter.finish(),
            Err(RunFailure::Snapshot(SnapshotFailure::Unreached(3)))
        ));
    }
}
