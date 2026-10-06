//! The `BasicHTTP` network of a live session (SCXML 1.0 appendix C.2):
//! where each session's `BasicHTTP` location points, how a POST that leaves
//! the in-process loopback reaches another host, and how a send that fails
//! after it was accepted comes back to its sender as `error.communication`.
//!
//! Every `BasicHTTP` decision stays the component's: it encodes every
//! outbound request, decodes every inbound message, and chooses every
//! response status. This module only moves requests between the session
//! thread and the embedder's [`OutboundHttp`] transport.
//!
//! # Late failures
//!
//! An [`OutboundHttp`] transport never blocks the session: it accepts a
//! request (the component is told `202 Accepted`, so the `<send>` itself
//! succeeds) and performs it elsewhere. When the request later fails (no
//! response, or a non-2xx status) the transport calls
//! [`CommunicationFailure::report`], which queues one owed
//! `error.communication` for the sending session and wakes the embedder
//! through [`LiveHttp::on_communication_error`]. The embedder then calls
//! [`super::LiveSession::deliver_communication_errors`], which places each
//! owed event on its sender's internal event queue (SCXML 3.12.2, 6.2.4)
//! at the session's current time and runs the macrostep that processes it,
//! exactly as the browser host delivers a late fetch failure.
//!
//! # Record and replay
//!
//! Whether a transport accepted or refused a request is an input of the
//! session (a refusal raises `error.communication` inside the `<send>`), so
//! a recording session journals every outbound decision
//! ([`OutboundJournal::Recording`]), and a replaying one answers every
//! outbound request from the recorded decisions in order
//! ([`OutboundJournal::Replaying`]) without any transport: an accepted
//! request owes nothing (its late failure, if any, is its own recorded
//! input), and running out of decisions fails the session closed
//! (`replay-diverged`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::TransportUnavailable;
use crate::basichttp_types::HttpRequest;

use super::loopback::LOOPBACK_ORIGIN;

/// Maximum bytes of a public `BasicHTTP` base URL.
pub const MAX_PUBLIC_URL_BYTES: usize = 2_048;

/// Why a public `BasicHTTP` base URL was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicUrlError {
    /// Not an absolute `http` or `https` URL with a non-empty authority.
    NotHttp,
    /// Carries userinfo, a query, or a fragment, or a byte that is not
    /// visible ASCII.
    Malformed,
    /// Longer than [`MAX_PUBLIC_URL_BYTES`].
    TooLong,
}

impl std::fmt::Display for PublicUrlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotHttp => "the public URL must be an absolute http or https URL",
            Self::Malformed => {
                "the public URL must be visible ASCII without userinfo, query, or fragment"
            }
            Self::TooLong => "the public URL exceeds its byte bound",
        })
    }
}

impl std::error::Error for PublicUrlError {}

/// The base every session's `BasicHTTP` location is built on:
/// `{base}/sessions/{_sessionid}/basichttp`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicUrl(String);

impl PublicUrl {
    /// Validates `url` as a base: an absolute `http` or `https` URL of
    /// visible ASCII with a non-empty authority and no userinfo, query, or
    /// fragment. One trailing `/` is dropped.
    ///
    /// # Errors
    ///
    /// [`PublicUrlError`].
    pub fn new(url: &str) -> Result<Self, PublicUrlError> {
        if url.len() > MAX_PUBLIC_URL_BYTES {
            return Err(PublicUrlError::TooLong);
        }
        let rest = ["https://", "http://"]
            .into_iter()
            .find_map(|scheme| {
                url.get(..scheme.len())
                    .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
                    .and_then(|_| url.get(scheme.len()..))
            })
            .ok_or(PublicUrlError::NotHttp)?;
        let authority = rest.split('/').next().unwrap_or_default();
        if authority.is_empty() {
            return Err(PublicUrlError::NotHttp);
        }
        if !url.bytes().all(|byte| byte.is_ascii_graphic())
            || authority.contains('@')
            || url.contains(['?', '#', '\\'])
        {
            return Err(PublicUrlError::Malformed);
        }
        Ok(Self(url.strip_suffix('/').unwrap_or(url).to_owned()))
    }

    /// The base URL, without a trailing `/`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The `<send>` a late failure is owed to: its session's `_sessionid` and
/// invoke-id path from the root (empty for the root), and the send's id,
/// which the owed `error.communication` carries as `_event.sendid` (SCXML
/// 5.10.1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommunicationSender {
    /// The sending session's `_sessionid`.
    pub session_id: String,
    /// Invoke ids from the root to the sending session.
    pub path: Vec<String>,
    /// The failed `<send>`'s id: the authored `id`, else the one the
    /// machine generated (SCXML 3.14).
    pub send_id: String,
}

/// The `<send>` an accepted outbound request belongs to, as the session
/// thread knows it: the sending session's key among the run's sessions
/// (keys are never reused, so it names that very session), its
/// `_sessionid`, and the send's id. The invoke-id path a record carries is
/// computed from the key only when a record is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OwedFailure {
    /// The sending session.
    pub(crate) key: scxml_scheduler::SessionKey,
    /// The sending session's `_sessionid`.
    pub(crate) session_id: String,
    /// The failed `<send>`'s id.
    pub(crate) send_id: String,
}

/// Owed `error.communication` events, shared by the session thread and
/// every transport thread.
pub(crate) struct FailureQueue {
    state: Mutex<FailureState>,
    /// Accepted requests whose outcome is still unknown.
    outstanding: AtomicUsize,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Default)]
struct FailureState {
    owed: Vec<OwedFailure>,
}

impl FailureQueue {
    pub(crate) fn new(notify: Option<Arc<dyn Fn() + Send + Sync>>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FailureState::default()),
            outstanding: AtomicUsize::new(0),
            notify,
        })
    }

    /// One ticket for an accepted request of `sender`.
    pub(crate) fn ticket(self: &Arc<Self>, sender: OwedFailure) -> CommunicationFailure {
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        CommunicationFailure {
            queue: Some(Arc::clone(self)),
            sender: Some(sender),
        }
    }

    /// Takes every owed event in report order.
    pub(crate) fn take(&self) -> Vec<OwedFailure> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut state.owed)
    }

    /// Whether an event is owed or an accepted request may still fail.
    pub(crate) fn pending(&self) -> bool {
        self.outstanding.load(Ordering::Acquire) > 0
            || !self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .owed
                .is_empty()
    }

    fn settle(&self, failed: Option<OwedFailure>) {
        if let Some(sender) = failed {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .owed
                .push(sender);
        }
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
        if let Some(notify) = &self.notify {
            notify();
        }
    }
}

/// The outcome channel of one accepted outbound request: call
/// [`Self::report`] if it failed (no response, or a non-2xx status) and
/// [`Self::succeeded`] otherwise. Dropping it unsettled counts as a
/// success. It is `Send`, so any thread may settle it.
pub struct CommunicationFailure {
    queue: Option<Arc<FailureQueue>>,
    sender: Option<OwedFailure>,
}

impl std::fmt::Debug for CommunicationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommunicationFailure")
            .finish_non_exhaustive()
    }
}

impl CommunicationFailure {
    /// The request failed: its sender is owed one `error.communication`
    /// (SCXML C.2), delivered by
    /// [`super::LiveSession::deliver_communication_errors`].
    pub fn report(mut self) {
        if let Some(queue) = self.queue.take() {
            queue.settle(self.sender.take());
        }
    }

    /// The recipient answered with a 2xx status: nothing is owed.
    pub fn succeeded(mut self) {
        if let Some(queue) = self.queue.take() {
            queue.settle(None);
        }
    }
}

impl Drop for CommunicationFailure {
    fn drop(&mut self) {
        if let Some(queue) = self.queue.take() {
            queue.settle(None);
        }
    }
}

/// The transport a live session uses for a `BasicHTTP` POST whose target
/// lies outside its own `BasicHTTP` address space (every other target is
/// delivered in-process by the loopback).
///
/// Called on the session thread from inside the component's `send`; it
/// must return at once and never call back into the session.
pub trait OutboundHttp {
    /// Accepts `request` for delivery and performs it elsewhere, settling
    /// `outcome` when it completes.
    ///
    /// # Errors
    ///
    /// [`TransportUnavailable`] to refuse the request now (a denied target,
    /// or no capacity): nothing was sent, and the component reports the send
    /// `unavailable`, so the sender raises `error.communication` at once.
    fn begin(
        &self,
        request: HttpRequest,
        outcome: CommunicationFailure,
    ) -> Result<(), TransportUnavailable>;
}

/// The `BasicHTTP` network of one live session. The default is none: every
/// location stays in the unresolvable loopback space and a POST outside it
/// is undeliverable (`error.communication`), as in a batch run.
#[derive(Clone, Default)]
pub struct LiveHttp {
    /// The base of every session's advertised location (see [`PublicUrl`]);
    /// `None` keeps the in-process loopback location.
    pub public_url: Option<PublicUrl>,
    /// The transport of POSTs outside the session's own address space;
    /// `None` refuses them.
    pub outbound: Option<Arc<dyn OutboundHttp>>,
    /// Called from any thread each time an accepted request settles, so
    /// the embedder can wake its loop and call
    /// [`super::LiveSession::deliver_communication_errors`].
    pub on_communication_error: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Called from any thread each time the HTTP response of an invoked
    /// service's request arrives (`outbound-http.fetch`), so the embedder
    /// can wake its loop and call
    /// [`super::LiveSession::deliver_http_responses`].
    pub on_http_response: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for LiveHttp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveHttp")
            .field("public_url", &self.public_url)
            .field("outbound", &self.outbound.is_some())
            .finish_non_exhaustive()
    }
}

/// One outbound transport decision: the transport accepted the request,
/// or refused it at once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboundDecision {
    /// The transport accepted the request.
    Accepted,
    /// The transport refused the request at once.
    Refused,
}

/// The decision an `InputRecord::OutboundHttp` record's `accepted` field
/// carries.
impl From<bool> for OutboundDecision {
    fn from(accepted: bool) -> Self {
        if accepted {
            Self::Accepted
        } else {
            Self::Refused
        }
    }
}

impl OutboundDecision {
    /// The record's `accepted` field for this decision.
    pub(crate) fn accepted(self) -> bool {
        self == Self::Accepted
    }
}

/// The outbound transport decisions of a recording or replaying live
/// session (see the module documentation, "Record and replay").
pub(crate) enum OutboundJournal {
    /// Neither recording nor replaying.
    Off,
    /// Every decision since the session last collected them.
    Recording(Vec<OutboundDecision>),
    /// The recorded decisions still to answer, in order.
    Replaying(VecDeque<OutboundDecision>),
}

/// A replaying session asked for more outbound decisions than were
/// recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReplayExhausted;

/// The `BasicHTTP` network state of one run: batch runs use the loopback
/// base and no transport.
pub(crate) struct RunHttp {
    /// The base of every location, without a trailing `/`.
    pub(crate) base: String,
    pub(crate) outbound: Option<Arc<dyn OutboundHttp>>,
    pub(crate) failures: Arc<FailureQueue>,
    pub(crate) journal: RefCell<OutboundJournal>,
    /// The settled requests of invoked services, waiting for delivery as
    /// external inputs; `None` for a batch run (which performs each request
    /// at once) and a replay (whose responses come from the record).
    pub(crate) http_responses: Option<Arc<crate::service::HttpResponses>>,
    /// Whether this is a live session (batch runs are not).
    live: bool,
}

impl RunHttp {
    pub(crate) fn batch() -> Self {
        Self {
            base: LOOPBACK_ORIGIN.to_owned(),
            outbound: None,
            failures: FailureQueue::new(None),
            journal: RefCell::new(OutboundJournal::Off),
            http_responses: None,
            live: false,
        }
    }

    pub(crate) fn live(http: LiveHttp, journal: OutboundJournal) -> Self {
        Self {
            base: http
                .public_url
                .map_or_else(|| LOOPBACK_ORIGIN.to_owned(), |url| url.0),
            outbound: http.outbound,
            failures: FailureQueue::new(http.on_communication_error),
            http_responses: (!matches!(journal, OutboundJournal::Replaying(_)))
                .then(|| crate::service::HttpResponses::new(http.on_http_response)),
            journal: RefCell::new(journal),
            live: true,
        }
    }

    /// How this run carries out its invoked services' requests through
    /// `transport`: a batch run at once, a live session on transport threads
    /// (delivering each response as an external input), a replay from its
    /// record; with no transport (and no record) they are refused.
    pub(crate) fn service_requests(
        &self,
        transport: Option<&Arc<dyn crate::extension::ExtensionHttp>>,
    ) -> crate::service::RequestMode {
        use crate::service::RequestMode;
        if self.live && matches!(*self.journal.borrow(), OutboundJournal::Replaying(_)) {
            return RequestMode::Recorded;
        }
        match (transport, &self.http_responses) {
            (None, _) => RequestMode::None,
            (Some(transport), Some(responses)) => RequestMode::Threaded {
                transport: Arc::clone(transport),
                responses: Arc::clone(responses),
            },
            (Some(transport), None) => RequestMode::Immediate(Arc::clone(transport)),
        }
    }

    /// The next recorded decision of a replaying session (`None` when it
    /// does not replay).
    pub(crate) fn replayed_decision(&self) -> Option<Result<OutboundDecision, ReplayExhausted>> {
        match &mut *self.journal.borrow_mut() {
            OutboundJournal::Replaying(decisions) => {
                Some(decisions.pop_front().ok_or(ReplayExhausted))
            }
            OutboundJournal::Off | OutboundJournal::Recording(_) => None,
        }
    }

    /// Journals one live decision when the session records.
    pub(crate) fn record_decision(&self, decision: OutboundDecision) {
        if let OutboundJournal::Recording(decisions) = &mut *self.journal.borrow_mut() {
            decisions.push(decision);
        }
    }

    /// Takes the decisions journaled since the last call.
    pub(crate) fn take_decisions(&self) -> Vec<OutboundDecision> {
        match &mut *self.journal.borrow_mut() {
            OutboundJournal::Recording(decisions) => std::mem::take(decisions),
            OutboundJournal::Off | OutboundJournal::Replaying(_) => Vec::new(),
        }
    }

    /// The `BasicHTTP` location of session `session_id` (SCXML C.2).
    pub(crate) fn location(&self, session_id: &str) -> String {
        format!("{}/sessions/{session_id}/basichttp", self.base)
    }
}

/// The `_sessionid` an inbound request target addresses: an origin-form
/// path or an absolute `http`/`https` URI whose path is
/// `<any prefix>/sessions/<id>/basichttp` (so a reverse proxy may mount the
/// listener under a prefix). A query or fragment addresses nothing.
pub(crate) fn inbound_session_id(target: &str) -> Option<&str> {
    let path = if target.starts_with('/') {
        target
    } else {
        let rest = ["https://", "http://"].into_iter().find_map(|scheme| {
            target
                .get(..scheme.len())
                .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
                .and_then(|_| target.get(scheme.len()..))
        })?;
        rest.find('/').and_then(|start| rest.get(start..))?
    };
    if path.contains(['?', '#']) {
        return None;
    }
    let mut segments = path.rsplit('/');
    let (Some("basichttp"), Some(id), Some("sessions")) =
        (segments.next(), segments.next(), segments.next())
    else {
        return None;
    };
    (!id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_urls_are_bounded_absolute_http_bases() {
        assert_eq!(
            PublicUrl::new("https://agent.example/scxml/")
                .expect("valid")
                .as_str(),
            "https://agent.example/scxml"
        );
        assert_eq!(
            PublicUrl::new("HTTP://127.0.0.1:8080")
                .expect("valid")
                .as_str(),
            "HTTP://127.0.0.1:8080"
        );
        for (url, error) in [
            ("ftp://agent.example", PublicUrlError::NotHttp),
            ("https://", PublicUrlError::NotHttp),
            ("https:///path", PublicUrlError::NotHttp),
            ("https://user@agent.example", PublicUrlError::Malformed),
            ("https://agent.example/?q", PublicUrlError::Malformed),
            ("https://agent.example/#f", PublicUrlError::Malformed),
            ("https://agent.example/a b", PublicUrlError::Malformed),
            ("https://agent.example\\x", PublicUrlError::Malformed),
        ] {
            assert_eq!(PublicUrl::new(url), Err(error), "{url}");
        }
        let long = format!("https://a.example/{}", "x".repeat(MAX_PUBLIC_URL_BYTES));
        assert_eq!(PublicUrl::new(&long), Err(PublicUrlError::TooLong));
    }

    #[test]
    fn inbound_targets_name_one_session() {
        for (target, id) in [
            ("/sessions/root/basichttp", Some("root")),
            ("/proxy/prefix/sessions/child1/basichttp", Some("child1")),
            ("http://127.0.0.1:9/sessions/root/basichttp", Some("root")),
            (
                "HTTPS://agent.example/x/sessions/s.1/basichttp",
                Some("s.1"),
            ),
            ("/sessions/root/basichttp?x=1", None),
            ("/sessions/root/basichttp#f", None),
            ("/sessions//basichttp", None),
            ("/session/root/basichttp", None),
            ("/sessions/root/basichttp/", None),
            ("/sessions/root", None),
            ("http://agent.example", None),
            ("mailto:root", None),
            ("", None),
        ] {
            assert_eq!(inbound_session_id(target), id, "{target}");
        }
    }

    #[test]
    fn failures_are_owed_once_per_reported_ticket() {
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let queue = FailureQueue::new(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        let sender = |id: &str| OwedFailure {
            key: scxml_scheduler::SessionKey::ROOT,
            session_id: id.to_owned(),
            send_id: format!("{id}-send"),
        };
        let failed = queue.ticket(sender("a"));
        let succeeded = queue.ticket(sender("b"));
        let dropped = queue.ticket(sender("c"));
        assert!(queue.pending());
        failed.report();
        succeeded.succeeded();
        drop(dropped);
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        assert_eq!(queue.take(), vec![sender("a")]);
        assert!(!queue.pending());
        assert_eq!(queue.take(), Vec::new());
    }

    /// The host owes every failed send its `error.communication`: more
    /// than 10,000 owed at once are all kept, in report order.
    #[test]
    fn owed_failures_are_unbounded() {
        let queue = FailureQueue::new(None);
        for index in 0..=10_000 {
            queue
                .ticket(OwedFailure {
                    key: scxml_scheduler::SessionKey::ROOT,
                    session_id: "root".into(),
                    send_id: format!("sendid_{index}"),
                })
                .report();
        }
        let owed = queue.take();
        assert_eq!(owed.len(), 10_001);
        assert_eq!(
            owed.last().map(|sender| sender.send_id.as_str()),
            Some("sendid_10000")
        );
    }
}
