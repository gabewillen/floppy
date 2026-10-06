//! Deferred outbound HTTP of invoked services (`outbound-http.begin` and
//! `take`, SCXML 6.4).
//!
//! An extension component's `execute` is synchronous executable content, and
//! its `outbound-http.fetch` blocks the session. An invoked service is the
//! place for slow I/O: `begin` admits a request on the session thread exactly
//! as `fetch` does, hands it to the embedder's [`ServiceHttp`] transport,
//! which performs it elsewhere and returns at once, and answers a ticket. The
//! session keeps processing events and timers meanwhile. When the request
//! settles, the transport settles its [`ServiceHttpOutcome`] from any thread:
//! the outcome joins the run's queue of settled requests and the embedder's
//! `LiveHttp::on_service_response` callback wakes its loop, which calls
//! `LiveSession::deliver_service_responses`. That call, on the session
//! thread, hands each outcome to its instance, runs the instance's `wake`
//! (where the service reads it with `take`), and runs every session to a
//! stable configuration at the session's current time.
//!
//! * **Cancellation** — an instance that is cancelled (its invoking state
//!   exited), completed, or retired abandons its requests: their outcomes are
//!   dropped when they arrive and nothing reaches the instance again. The
//!   transport is not interrupted; its own timeouts bound it.
//! * **Bounds** — at most [`MAX_SERVICE_PENDING_REQUESTS`] requests of one
//!   instance are in flight or settled but untaken (an amplification guard,
//!   every build); [`ThreadedServiceHttp`] holds at most its configured
//!   number of requests in flight across the run.
//! * **Where it is offered** — only a live session that neither records nor
//!   replays delivers outcomes (its inputs would not be reproducible), so
//!   only there does `begin` start anything; in a batch run or a recording
//!   or replaying session it answers `unavailable`. A logical-clock live
//!   session does not move its clock past a pending request:
//!   `LiveSession::run_until_idle` returns while one is outstanding.
//!
//! Telemetry: `service-http-begin` (one per `begin` on the session thread),
//! `service-http-request` (one per request [`ThreadedServiceHttp`] performs,
//! off the session thread), and `session-service-response` (one per delivery
//! call that delivers an outcome), each with closed outcome labels only; no
//! URL, header, body, status, ticket, or type URI is ever recorded.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};

use crate::TransportUnavailable;
use crate::extension::{
    ExtensionHttp, ExtensionHttpFailure, ExtensionHttpRequest, ExtensionHttpResponse,
};

/// Maximum requests of one service instance in flight or settled but not yet
/// taken; the `begin` past it fails with `capacity-exhausted`. An
/// amplification guard, so it applies on every build.
pub const MAX_SERVICE_PENDING_REQUESTS: usize = 16;
/// Default maximum requests a [`ThreadedServiceHttp`] holds in flight.
pub const DEFAULT_SERVICE_HTTP_IN_FLIGHT: usize = 64;
/// Upper bound of a [`ThreadedServiceHttp`]'s in-flight bound.
pub const MAX_SERVICE_HTTP_IN_FLIGHT: usize = 1_024;

/// The deferred outbound HTTP transport an embedder grants to invoked
/// services ([`crate::ServiceComponents::with_deferred_http`]).
///
/// The host admits every request before the transport sees it (the
/// service package's declared origins, or an origin a reference's value
/// supplied), so the transport sends exactly the canonical request it is
/// given. [`ServiceHttp::begin`] runs on the session thread and must return
/// at once: the transport performs the request elsewhere, bounds its own
/// time and response body, follows no redirect, adds no ambient
/// credentials, and settles `outcome` exactly once (dropping it unsettled
/// settles it as a transport failure).
pub trait ServiceHttp: std::fmt::Debug + Send + Sync {
    /// Starts one admitted request.
    ///
    /// # Errors
    ///
    /// [`TransportUnavailable`] when it refuses the request at once (nothing
    /// was sent); `begin` then fails with `unavailable`.
    fn begin(
        &self,
        request: ExtensionHttpRequest,
        outcome: ServiceHttpOutcome,
    ) -> Result<(), TransportUnavailable>;
}

/// One settled request, waiting for delivery on the session thread.
pub(crate) struct Settled {
    /// The run-unique id of the instance that began it.
    pub(crate) instance: u64,
    /// Its ticket within that instance.
    pub(crate) ticket: u64,
    /// What the transport answered.
    pub(crate) outcome: Result<ExtensionHttpResponse, ExtensionHttpFailure>,
}

/// The settled requests of one live run, shared by the session thread and
/// every transport thread.
pub(crate) struct ServiceResponses {
    settled: Mutex<VecDeque<Settled>>,
    next_instance: AtomicU64,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl ServiceResponses {
    /// An empty queue that calls `notify` (from the settling thread) each
    /// time a request settles.
    pub(crate) fn new(notify: Option<Arc<dyn Fn() + Send + Sync>>) -> Arc<Self> {
        Arc::new(Self {
            settled: Mutex::new(VecDeque::new()),
            next_instance: AtomicU64::new(1),
            notify,
        })
    }

    /// A fresh run-unique instance id.
    pub(crate) fn instance_id(&self) -> u64 {
        self.next_instance.fetch_add(1, Ordering::Relaxed)
    }

    /// The outcome handle of the request `ticket` of `instance`.
    pub(crate) fn outcome(self: &Arc<Self>, instance: u64, ticket: u64) -> ServiceHttpOutcome {
        ServiceHttpOutcome {
            queue: Some(Arc::clone(self)),
            instance,
            ticket,
        }
    }

    /// Takes every settled request, in settling order.
    pub(crate) fn take(&self) -> Vec<Settled> {
        let mut settled = self.settled.lock().unwrap_or_else(PoisonError::into_inner);
        settled.drain(..).collect()
    }

    /// Whether a settled request waits for delivery.
    pub(crate) fn has_settled(&self) -> bool {
        !self
            .settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    fn push(&self, settled: Settled) {
        self.settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(settled);
        if let Some(notify) = &self.notify {
            notify();
        }
    }
}

/// How one begun request settles: handed to [`ServiceHttp::begin`] and
/// settled exactly once, from any thread. Dropping it unsettled settles it
/// as [`ExtensionHttpFailure::Transport`], so a request is never left
/// pending forever.
pub struct ServiceHttpOutcome {
    queue: Option<Arc<ServiceResponses>>,
    instance: u64,
    ticket: u64,
}

impl std::fmt::Debug for ServiceHttpOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServiceHttpOutcome")
            .field("settled", &self.queue.is_none())
            .finish_non_exhaustive()
    }
}

impl ServiceHttpOutcome {
    /// Settles the request with what the transport answered.
    pub fn settle(mut self, outcome: Result<ExtensionHttpResponse, ExtensionHttpFailure>) {
        self.finish(outcome);
    }

    fn finish(&mut self, outcome: Result<ExtensionHttpResponse, ExtensionHttpFailure>) {
        if let Some(queue) = self.queue.take() {
            queue.push(Settled {
                instance: self.instance,
                ticket: self.ticket,
                outcome,
            });
        }
    }
}

impl Drop for ServiceHttpOutcome {
    fn drop(&mut self) {
        self.finish(Err(ExtensionHttpFailure::Transport));
    }
}

/// An invalid [`ThreadedServiceHttp`] bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceHttpInFlightInvalid;

impl std::fmt::Display for ServiceHttpInFlightInvalid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "service requests in flight must be between 1 and {MAX_SERVICE_HTTP_IN_FLIGHT}"
        )
    }
}

impl std::error::Error for ServiceHttpInFlightInvalid {}

/// A [`ServiceHttp`] that performs each request through a blocking
/// [`ExtensionHttp`] transport (for example the `http` feature's
/// `ExtensionHttpClient`) on its own short-lived thread, at most
/// `max_in_flight` at once (one more is refused at once). The embedder
/// chooses it; the host itself starts no thread.
pub struct ThreadedServiceHttp {
    transport: Arc<dyn ExtensionHttp>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: usize,
}

impl std::fmt::Debug for ThreadedServiceHttp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ThreadedServiceHttp")
            .field("max_in_flight", &self.max_in_flight)
            .finish_non_exhaustive()
    }
}

impl ThreadedServiceHttp {
    /// Performs requests through `transport`, at most `max_in_flight` at
    /// once.
    ///
    /// # Errors
    ///
    /// [`ServiceHttpInFlightInvalid`] for a bound outside
    /// `1..=`[`MAX_SERVICE_HTTP_IN_FLIGHT`].
    pub fn new(
        transport: Arc<dyn ExtensionHttp>,
        max_in_flight: usize,
    ) -> Result<Self, ServiceHttpInFlightInvalid> {
        if !(1..=MAX_SERVICE_HTTP_IN_FLIGHT).contains(&max_in_flight) {
            return Err(ServiceHttpInFlightInvalid);
        }
        Ok(Self {
            transport,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight,
        })
    }

    /// Requests in flight now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// One reserved in-flight slot, released on drop.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ServiceHttp for ThreadedServiceHttp {
    fn begin(
        &self,
        request: ExtensionHttpRequest,
        outcome: ServiceHttpOutcome,
    ) -> Result<(), TransportUnavailable> {
        let reserved = self
            .in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.max_in_flight).then_some(count + 1)
            });
        if reserved.is_err() {
            let span = scxml_telemetry::observe(
                Module::HostWasmtime,
                Operation::HostWasmtimeServiceHttpRequest,
            );
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Budget);
            return Err(TransportUnavailable);
        }
        let slot = Slot(Arc::clone(&self.in_flight));
        let transport = Arc::clone(&self.transport);
        let parent = Context::current();
        let spawned = std::thread::Builder::new()
            .name("scxml-service-http".into())
            .spawn(move || {
                let _slot = slot;
                let _parent = parent.attach();
                let span = scxml_telemetry::observe(
                    Module::HostWasmtime,
                    Operation::HostWasmtimeServiceHttpRequest,
                );
                let answered = transport.fetch(&request);
                if answered.is_ok() {
                    span.event(SignalEvent::TransformationCompleted);
                } else {
                    span.event(SignalEvent::InputRejected);
                    span.fail(ErrorCategory::Platform);
                }
                outcome.settle(answered);
            });
        // A thread that could not start sent nothing: refuse now (the slot
        // was released and the outcome settled as a transport failure when
        // the closure was dropped; the instance never holds that ticket).
        spawned.map(|_| ()).map_err(|_| TransportUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::{ServiceResponses, ThreadedServiceHttp};
    use crate::extension::{
        ExtensionHttp, ExtensionHttpFailure, ExtensionHttpRequest, ExtensionHttpResponse,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn an_outcome_settles_once_and_a_dropped_one_settles_as_a_transport_failure() {
        let notified = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&notified);
        let queue = ServiceResponses::new(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::AcqRel);
        })));
        let instance = queue.instance_id();
        assert_ne!(queue.instance_id(), instance, "instance ids are unique");
        queue.outcome(instance, 1).settle(Ok(ExtensionHttpResponse {
            status: 200,
            body: b"ok".to_vec(),
        }));
        drop(queue.outcome(instance, 2));
        assert!(queue.has_settled());
        let settled = queue.take();
        assert_eq!(settled.len(), 2);
        assert_eq!((settled[0].instance, settled[0].ticket), (instance, 1));
        assert!(settled[0].outcome.is_ok());
        assert_eq!(settled[1].outcome, Err(ExtensionHttpFailure::Transport));
        assert!(!queue.has_settled());
        assert_eq!(notified.load(Ordering::Acquire), 2);
    }

    #[derive(Debug)]
    struct Answer;

    impl ExtensionHttp for Answer {
        fn fetch(
            &self,
            _request: &ExtensionHttpRequest,
        ) -> Result<ExtensionHttpResponse, ExtensionHttpFailure> {
            Ok(ExtensionHttpResponse {
                status: 204,
                body: Vec::new(),
            })
        }
    }

    #[test]
    fn the_threaded_transport_bounds_its_requests_in_flight() {
        assert!(ThreadedServiceHttp::new(Arc::new(Answer), 0).is_err());
        assert!(
            ThreadedServiceHttp::new(Arc::new(Answer), super::MAX_SERVICE_HTTP_IN_FLIGHT + 1)
                .is_err()
        );
        let transport = ThreadedServiceHttp::new(Arc::new(Answer), 1).expect("a valid bound");
        assert_eq!(transport.in_flight(), 0);
    }
}
