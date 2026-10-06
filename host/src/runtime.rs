//! One live bot session on its own thread.
//!
//! `scxml_host_wasmtime::SessionHost` and `LiveSession` are single-threaded
//! (`!Send`): the session thread links the host and opens the session itself,
//! and owns both until it closes. Every other thread talks to it through an
//! ingress channel ([`Ingress`]: events and close) and reads what it produces
//! from an egress channel ([`Egress`]). The thread waits on the ingress
//! channel until the session's next delayed-send deadline on a wall clock
//! ([`ClockMode::Wall`]), or fires every pending deadline after each event on
//! the deterministic logical clock ([`ClockMode::Logical`]).
//!
//! The session is never recorded: a recording would need every package to
//! declare no WASI permission ([`crate::packages::Packages::declares_wasi`]),
//! and nemobot does not record yet.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use scxml_host_wasmtime::{
    DocumentKind, EmbeddedDocument, ExtensionHttp, LiveClock, LiveHttp, LiveSession, LiveStatus,
    MachineScripts, ScriptedEvent, SessionEmbedder, SessionHost, SessionHostError, SessionOpen,
    SessionOutput, WallClock,
};

use crate::compile::Compiled;
use crate::config::Config;
use crate::env_file::Values;
use crate::packages::Packages;
use crate::telemetry::{Operation, observe};
use crate::{FailureKind, HostFailure};

/// How the session's logical clock moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockMode {
    /// The process's monotonic wall clock: delayed sends fire at their real
    /// deadline.
    Wall,
    /// The deterministic logical clock: after each event every pending
    /// delayed send fires at once, in deadline order.
    Logical,
}

/// What to open.
#[derive(Clone, Debug)]
pub struct SessionSpec {
    /// The compiled root document.
    pub compiled: Compiled,
    /// The root session's `_sessionid` (a valid SCXML name).
    pub session_id: String,
    /// How its clock moves.
    pub clock: ClockMode,
    /// Documents `<invoke src>` and `<data src>` may load, by exact URI.
    pub documents: Vec<(String, String)>,
}

/// One input to the session thread.
#[derive(Clone, Debug)]
pub enum Ingress {
    /// One external event for the root session.
    Event(ScriptedEvent),
    /// Close the session.
    Close,
}

/// The session's status, as egress reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    /// It accepts events.
    Running,
    /// The root reached a top-level final state.
    Finished,
    /// A call failed the session.
    Failed,
}

impl SessionStatus {
    /// The stable label (also the `nemobot.session.status` telemetry value).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }

    const fn from_live(status: LiveStatus) -> Self {
        match status {
            LiveStatus::Running => Self::Running,
            LiveStatus::Finished => Self::Finished,
            LiveStatus::Failed => Self::Failed,
        }
    }
}

/// One record from the session thread, in the order it happened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Egress {
    /// The session opened and ran to its first stable configuration.
    Opened {
        /// The root's active state ids.
        configuration: Vec<String>,
    },
    /// Outputs of the sessions (`<log>` records, `done` of invoked children).
    Outputs(Vec<SessionOutput>),
    /// An event the session refused without failing (an invalid name, an
    /// event after the root finished, or one the machine rejected).
    Rejected {
        /// The refused event's name.
        event: String,
        /// The session host's failure, as it displays it.
        reason: String,
    },
    /// A call failed the session; only the close follows.
    Failed {
        /// The session host's failure, as it displays it.
        reason: String,
    },
    /// The session closed; nothing follows.
    Closed {
        /// Its status when it closed.
        status: SessionStatus,
        /// Delayed sends it cancelled.
        cancelled_sends: usize,
        /// Invoked children it cancelled.
        cancelled_children: usize,
    },
}

/// The caller's side of one live session.
#[derive(Debug)]
pub struct SessionHandle {
    ingress: Sender<Ingress>,
    egress: Receiver<Egress>,
    thread: Option<JoinHandle<Result<(), HostFailure>>>,
}

/// Documents keyed by exact URI; no scripts.
struct Documents(Vec<(String, String)>);

impl SessionEmbedder for Documents {
    fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        self.0
            .iter()
            .find(|(key, _)| key == uri)
            .map(|(_, text)| text.clone().into())
    }

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

fn session_error(message: impl Into<String>) -> HostFailure {
    HostFailure::new(FailureKind::Session, message)
}

fn host_failure(error: &SessionHostError) -> HostFailure {
    session_error(error.to_string())
}

/// What the session thread needs to link the host.
struct Binding {
    components_dir: PathBuf,
    manifest_sha256: String,
    fuel: u64,
    max_commands: u64,
    extension_call_fuel: u64,
    outbound_http: bool,
    packages: Packages,
    values: Values,
}

impl Binding {
    fn host(&self) -> Result<SessionHost, HostFailure> {
        let http: Option<Arc<dyn ExtensionHttp>> = if self.outbound_http {
            let client = scxml_host_wasmtime::ExtensionHttpClient::new(
                scxml_host_wasmtime::ExtensionHttpConfig::new(),
            )
            .map_err(|error| {
                session_error(format!("the outbound HTTP client is invalid: {error}"))
            })?;
            Some(Arc::new(client))
        } else {
            None
        };
        let extensions = self
            .packages
            .extension_components(&self.values, http.as_ref())?
            .with_call_fuel(self.extension_call_fuel)
            .map_err(|error| {
                session_error(format!("the extension call fuel is invalid: {error}"))
            })?;
        let services = self
            .packages
            .service_components(&self.values, http.as_ref())?;
        SessionHost::from_release_dir(&self.components_dir, &self.manifest_sha256, self.fuel)
            .map_err(|error| host_failure(&error))?
            .with_extension_components(extensions)
            .map_err(|error| {
                session_error(format!("the session host refuses the extensions: {error}"))
            })?
            .with_service_components(services)
            .map_err(|error| {
                session_error(format!("the session host refuses the services: {error}"))
            })
    }
}

impl SessionHandle {
    /// Opens one live session of `spec` with `config`'s components, packages,
    /// and values, on a new thread. Returns once the session opened (or
    /// failed to).
    ///
    /// # Errors
    ///
    /// [`FailureKind::Channel`] when the thread cannot start, and any failure
    /// of linking the host or opening the session.
    pub fn open(
        config: &Config,
        packages: Packages,
        values: Values,
        spec: SessionSpec,
    ) -> Result<Self, HostFailure> {
        let binding = Binding {
            components_dir: config.components.dir.clone(),
            manifest_sha256: config.components.manifest_sha256.clone(),
            fuel: config.fuel,
            max_commands: config.max_commands,
            extension_call_fuel: config.extension_call_fuel,
            outbound_http: config.outbound_http,
            packages,
            values,
        };
        let (ingress, ingress_receiver) = mpsc::channel();
        let (egress_sender, egress) = mpsc::channel();
        let (opened_sender, opened) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("nemobot-session".into())
            .spawn(move || {
                session_thread(
                    &binding,
                    &spec,
                    &ingress_receiver,
                    &egress_sender,
                    &opened_sender,
                )
            })
            .map_err(|error| {
                HostFailure::new(
                    FailureKind::Channel,
                    format!("cannot start the session thread: {error}"),
                )
            })?;
        match opened.recv() {
            Ok(Ok(())) => Ok(Self {
                ingress,
                egress,
                thread: Some(thread),
            }),
            Ok(Err(failure)) => {
                // The thread returns right after reporting the failure.
                let _joined = thread.join();
                Err(failure)
            }
            Err(_) => Err(match thread.join() {
                Ok(Err(failure)) => failure,
                Ok(Ok(())) | Err(_) => HostFailure::new(
                    FailureKind::Channel,
                    "the session thread ended before the session opened",
                ),
            }),
        }
    }

    /// A sender for this session's ingress (cloneable, `Send`).
    #[must_use]
    pub fn ingress(&self) -> Sender<Ingress> {
        self.ingress.clone()
    }

    /// Sends one external event.
    ///
    /// # Errors
    ///
    /// [`FailureKind::Channel`] when the session thread is gone.
    pub fn send(&self, event: ScriptedEvent) -> Result<(), HostFailure> {
        self.ingress
            .send(Ingress::Event(event))
            .map_err(|_| HostFailure::new(FailureKind::Channel, "the session thread is gone"))
    }

    /// The next egress record, waiting at most `timeout`; `None` when none
    /// arrived in time or the thread is gone.
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Egress> {
        self.egress.recv_timeout(timeout).ok()
    }

    /// Closes the session and returns every egress record not yet read,
    /// ending with [`Egress::Closed`].
    ///
    /// # Errors
    ///
    /// [`FailureKind::Channel`] when the thread panicked, and the thread's
    /// own failure (closing the session).
    pub fn close(mut self) -> Result<Vec<Egress>, HostFailure> {
        // A closed ingress also closes the session, so a failed send (the
        // thread already ended) needs nothing more.
        let _sent = self.ingress.send(Ingress::Close);
        let joined = match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| {
                HostFailure::new(FailureKind::Channel, "the session thread panicked")
            })?,
            None => Ok(()),
        };
        let remaining: Vec<Egress> = self.egress.try_iter().collect();
        joined.map(|()| remaining)
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _sent = self.ingress.send(Ingress::Close);
            let _joined = thread.join();
        }
    }
}

/// Runs the session to its close. Every egress send that fails means the
/// caller dropped its handle, which closes the session.
fn session_thread(
    binding: &Binding,
    spec: &SessionSpec,
    ingress: &Receiver<Ingress>,
    egress: &Sender<Egress>,
    opened: &Sender<Result<(), HostFailure>>,
) -> Result<(), HostFailure> {
    let open_span = observe(Operation::SessionOpen);
    let open = open_session(binding, spec);
    open_span.finish(&open);
    let (host, mut live, configuration) = match open {
        Ok(opened_session) => opened_session,
        Err(failure) => {
            // The caller is waiting on `opened`; a dropped receiver means it
            // is gone too, and the failure is returned either way.
            let _reported = opened.send(Err(failure.clone()));
            return Err(failure);
        }
    };
    if opened.send(Ok(())).is_err() || egress.send(Egress::Opened { configuration }).is_err() {
        return close(live, egress);
    }
    loop {
        let outputs = live.take_outputs();
        if !outputs.is_empty() && egress.send(Egress::Outputs(outputs)).is_err() {
            break;
        }
        if live.status() != LiveStatus::Running {
            break;
        }
        let wait = match spec.clock {
            ClockMode::Logical => None,
            ClockMode::Wall => match live.next_deadline() {
                Ok(deadline) => deadline
                    .map(|deadline| deadline.saturating_sub(live.clock_ms().unwrap_or(deadline))),
                Err(error) => {
                    if egress
                        .send(Egress::Failed {
                            reason: error.to_string(),
                        })
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
            },
        };
        let message = match wait {
            None => ingress.recv().ok(),
            Some(ms) => match ingress.recv_timeout(Duration::from_millis(ms)) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => {
                    let mut span = observe(Operation::SessionStep);
                    if let Err(error) = live.advance() {
                        span.fail(FailureKind::Session);
                        if egress
                            .send(Egress::Failed {
                                reason: error.to_string(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            },
        };
        let Some(Ingress::Event(event)) = message else {
            break;
        };
        let mut span = observe(Operation::SessionStep);
        let result = live.send(&event).and_then(|step| match spec.clock {
            ClockMode::Logical if step.status == LiveStatus::Running => live.run_until_idle(),
            _ => Ok(step),
        });
        if let Err(error) = result {
            span.fail(FailureKind::Session);
            let record = if live.status() == LiveStatus::Failed {
                Egress::Failed {
                    reason: error.to_string(),
                }
            } else {
                Egress::Rejected {
                    event: event.name.clone(),
                    reason: error.to_string(),
                }
            };
            if egress.send(record).is_err() {
                break;
            }
        }
    }
    let closed = close(live, egress);
    drop(host);
    closed
}

/// Links the host and opens the root session at its first stable
/// configuration.
fn open_session(
    binding: &Binding,
    spec: &SessionSpec,
) -> Result<(SessionHost, LiveSession, Vec<String>), HostFailure> {
    let host = binding.host()?;
    let clock = match spec.clock {
        ClockMode::Wall => LiveClock::Timed(Box::new(WallClock::start())),
        ClockMode::Logical => LiveClock::Logical,
    };
    let mut live = host
        .open(SessionOpen {
            machine_ir: &spec.compiled.ir,
            data_model: spec.compiled.data_model.as_deref(),
            session_id: &spec.session_id,
            embedder: Rc::new(Documents(spec.documents.clone())),
            max_commands: binding.max_commands,
            clock,
            record: false,
            replay: None,
            http: LiveHttp::default(),
        })
        .map_err(|error| host_failure(&error))?;
    let configuration = live.configuration().map_err(|error| host_failure(&error))?;
    Ok((host, live, configuration))
}

/// Flushes the session's outputs, closes it, and reports the close.
fn close(mut live: LiveSession, egress: &Sender<Egress>) -> Result<(), HostFailure> {
    let mut span = observe(Operation::SessionClose);
    let outputs = live.take_outputs();
    // The caller may have dropped its handle; the session closes either way.
    let _sent = (!outputs.is_empty()).then(|| egress.send(Egress::Outputs(outputs)));
    match live.close() {
        Ok(report) => {
            let status = SessionStatus::from_live(report.status);
            span.status(status.name());
            if !report.outputs.is_empty() {
                let _sent = egress.send(Egress::Outputs(report.outputs));
            }
            let _sent = egress.send(Egress::Closed {
                status,
                cancelled_sends: report.cancelled_sends,
                cancelled_children: report.cancelled_children,
            });
            Ok(())
        }
        Err(error) => {
            span.fail(FailureKind::Session);
            Err(host_failure(&error))
        }
    }
}
