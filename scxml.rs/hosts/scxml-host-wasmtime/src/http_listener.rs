//! The built-in `BasicHTTP` listener of live sessions (feature `http`): a
//! minimal, bounded HTTP/1.1 server that turns each inbound connection into
//! one [`InboundHttp`] for the session thread and writes back the response
//! the `BasicHTTP` component chose (SCXML C.2).
//!
//! It interprets nothing: the method, content type, body encoding, event
//! name, and response status are all the component's decisions, made on the
//! session thread by [`crate::LiveSession::receive_http`] (or
//! [`crate::LiveSession::http_rejection`] for a message this listener
//! could not frame). An embedder with its own server calls those methods
//! directly and never needs this module.
//!
//! # Bounds
//!
//! At most [`ListenerConfig::max_connections`] connections at once (one
//! more is answered `503` and closed), a request head of at most
//! [`MAX_REQUEST_HEAD_BYTES`] and 64 header fields, a body of at most
//! [`MAX_REQUEST_BODY_BYTES`] framed by `Content-Length` (a larger one is
//! rejected as `payload-too-large` without being read; `Transfer-Encoding`
//! is rejected as `invalid-message`), one deadline for reading the whole
//! request (so a client that drips bytes cannot hold a connection), a write
//! timeout, and a bounded wait for the session to claim the message
//! ([`ListenerConfig::response_timeout`]; one it does not claim in time is
//! answered `503`). Once the session claimed a message the listener waits
//! for its answer as long as the session's call takes: no native bound
//! cuts a call short. Every connection
//! carries exactly one request (`Connection: close`). A failed `accept` is
//! classified: a connection that failed before it was accepted is skipped,
//! and a listener failure (for example no file descriptor left) makes the
//! listener wait, 10 ms doubling to 1 s, before it accepts again.
//! [`ListenerHandle::panicked`] reports an accept or connection thread that
//! panicked.
//!
//! The accept loop never blocks: with no connection pending, and while it
//! backs off after a listener failure, it checks the stop flag every
//! [`ACCEPT_POLL`], so [`ListenerHandle::stop`] (and dropping the handle)
//! ends it within that time whatever address it is bound to and whether or
//! not anything can connect to it.
//!
//! Stopping does not join the connection threads in progress: they are
//! detached. A connection may be waiting for a session call that is itself
//! waiting on the thread that stops or drops the handle, so joining them
//! there could deadlock. Each detached thread still ends on its own: within
//! the read timeout, the claim wait ([`ListenerConfig::response_timeout`]),
//! and the write timeout, or, once the session claimed its message, when
//! the session answers it or drops it (closing), and then the write
//! timeout.
//!
//! # Telemetry
//!
//! One `basichttp-listener-request` span per connection, one
//! `basichttp-listener-accept` span per failed `accept`, and one
//! `basichttp-listener-connection` span per accepted connection the
//! listener dropped (`input-rejected`, `platform`) or served on a thread it
//! could not keep (a poisoned thread list: `platform`, no event), each with
//! only the closed outcome and error category; no address, path, header,
//! status, or body is recorded.

use std::fmt::Write as _;
use std::io::{self, Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};

use crate::basichttp_types::{HttpHeader, HttpRequest, HttpResponse, Rejection};

/// Maximum bytes of one request head (request line and header fields).
pub const MAX_REQUEST_HEAD_BYTES: usize = 64 * 1024;
/// Maximum bytes of one request body (the `basichttp.wit` `http-request`
/// body bound).
pub const MAX_REQUEST_BODY_BYTES: usize = 65_536;
/// Maximum header fields of one request (the `basichttp.wit` bound).
pub const MAX_REQUEST_HEADERS: usize = 64;
/// Default maximum connections served at once.
pub const MAX_CONCURRENT_CONNECTIONS: usize = 64;
/// How often an idle accept loop checks whether it was stopped.
pub const ACCEPT_POLL: Duration = Duration::from_millis(10);

/// Configuration of one [`BasicHttpListener`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListenerConfig {
    /// Connections served at once (non-zero).
    pub max_connections: usize,
    /// Time allowed to read one whole request, head and body (non-zero).
    pub read_timeout: Duration,
    /// Time allowed for each write of a connection (non-zero).
    pub write_timeout: Duration,
    /// Time to wait for the session to claim a message before answering
    /// `503` (non-zero); a claimed message waits for the session's answer.
    pub response_timeout: Duration,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            max_connections: MAX_CONCURRENT_CONNECTIONS,
            read_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            response_timeout: Duration::from_secs(30),
        }
    }
}

/// One inbound message waiting for the session thread. Dropping it
/// answers `503`.
///
/// The session thread [`claims`](Self::claim) it to reach its request: the
/// listener answers `503` once [`ListenerConfig::response_timeout`] passes
/// without a claim, and a message it answered that way must not still
/// become an event, so only a [`ClaimedHttp`] gives the request.
#[derive(Debug)]
pub struct InboundHttp {
    request: Result<HttpRequest, Rejection>,
    reply: mpsc::SyncSender<HttpResponse>,
    claim: Arc<AtomicU8>,
}

/// An inbound message the session claimed: its request (or why it could
/// not be framed) and the channel its response goes back on. The listener
/// waits for that response; dropping it unanswered answers `503`.
#[derive(Debug)]
pub struct ClaimedHttp {
    request: Result<HttpRequest, Rejection>,
    reply: mpsc::SyncSender<HttpResponse>,
}

/// [`InboundHttp::claim`] states.
const PENDING: u8 = 0;
const CLAIMED: u8 = 1;
const ABANDONED: u8 = 2;

impl InboundHttp {
    /// Claims the message for the session: `None` when the listener has
    /// already answered `503` because the session did not take it in time,
    /// and the message is then dropped, never delivered. Once claimed, the
    /// listener waits for the session's answer.
    #[must_use]
    pub fn claim(self) -> Option<ClaimedHttp> {
        self.claim
            .compare_exchange(PENDING, CLAIMED, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| ClaimedHttp {
                request: self.request,
                reply: self.reply,
            })
    }
}

impl ClaimedHttp {
    /// The framed request, or the rejection to answer through
    /// [`crate::LiveSession::http_rejection`].
    ///
    /// # Errors
    ///
    /// The listener could not frame the message.
    pub fn request(&self) -> Result<&HttpRequest, Rejection> {
        self.request.as_ref().map_err(|rejection| *rejection)
    }

    /// Sends the response back to the client.
    pub fn respond(self, response: HttpResponse) {
        // The connection may already be gone (timeout); nothing to do then.
        let _ = self.reply.try_send(response);
    }
}

/// A bound, not yet serving listener.
#[derive(Debug)]
pub struct BasicHttpListener {
    listener: TcpListener,
    local_addr: SocketAddr,
    config: ListenerConfig,
}

/// A serving listener; [`Self::stop`], or dropping it, ends its accept
/// loop.
#[derive(Debug)]
pub struct ListenerHandle {
    stop: Arc<AtomicBool>,
    local_addr: SocketAddr,
    /// The accept thread, until it was joined.
    accept: Mutex<Option<JoinHandle<()>>>,
    /// The connection threads not yet joined.
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Whether a joined thread had panicked.
    panicked: Arc<AtomicBool>,
}

impl ListenerHandle {
    /// The bound address.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops accepting connections (connections in progress finish) and
    /// waits for the accept thread to end, which it does within
    /// [`ACCEPT_POLL`] of an idle accept.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        let accept = self.accept.lock().ok().and_then(|mut slot| slot.take());
        if let Some(accept) = accept {
            note_panic(&self.panicked, &accept.join());
        }
    }

    /// Whether the accept thread or a connection thread panicked: an
    /// accept thread that ended before [`Self::stop`] did, or any
    /// connection thread that ended in a panic.
    #[must_use]
    pub fn panicked(&self) -> bool {
        if let Ok(mut slot) = self.accept.lock()
            && slot.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(accept) = slot.take()
        {
            note_panic(&self.panicked, &accept.join());
        }
        join_finished(&self.connections, &self.panicked);
        self.panicked.load(Ordering::Acquire)
    }
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Records a joined thread that panicked.
fn note_panic(panicked: &AtomicBool, joined: &std::thread::Result<()>) {
    if joined.is_err() {
        panicked.store(true, Ordering::Release);
    }
}

/// Joins every finished connection thread, recording any that panicked.
fn join_finished(connections: &Mutex<Vec<JoinHandle<()>>>, panicked: &AtomicBool) {
    let Ok(mut connections) = connections.lock() else {
        panicked.store(true, Ordering::Release);
        return;
    };
    let (finished, running): (Vec<_>, Vec<_>) =
        connections.drain(..).partition(JoinHandle::is_finished);
    *connections = running;
    for connection in finished {
        note_panic(panicked, &connection.join());
    }
}

impl BasicHttpListener {
    /// Binds `addr` (port 0 picks a free port).
    ///
    /// # Errors
    ///
    /// The bind failed, or `config.max_connections` or a timeout is zero
    /// ([`io::ErrorKind::InvalidInput`]).
    pub fn bind(addr: SocketAddr, config: ListenerConfig) -> io::Result<Self> {
        if config.max_connections == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_connections must be greater than zero",
            ));
        }
        if config.read_timeout.is_zero()
            || config.write_timeout.is_zero()
            || config.response_timeout.is_zero()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "listener timeouts must be greater than zero",
            ));
        }
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            local_addr,
            config,
        })
    }

    /// The bound address.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Serves on a background thread: each connection is read on its own
    /// thread and handed to `deliver` as one [`InboundHttp`].
    ///
    /// # Errors
    ///
    /// The accept thread could not start.
    pub fn serve(
        self,
        deliver: impl Fn(InboundHttp) + Send + Sync + 'static,
    ) -> io::Result<ListenerHandle> {
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(Mutex::new(Vec::new()));
        let panicked = Arc::new(AtomicBool::new(false));
        let deliver: Arc<dyn Fn(InboundHttp) + Send + Sync> = Arc::new(deliver);
        let active = Arc::new(AtomicUsize::new(0));
        let Self {
            listener,
            config,
            local_addr,
        } = self;
        listener.set_nonblocking(true)?;
        let handle_stop = Arc::clone(&stop);
        let (accept_connections, accept_panicked) =
            (Arc::clone(&connections), Arc::clone(&panicked));
        let accept_thread = std::thread::Builder::new()
            .name("scxml-basichttp-listener".into())
            .spawn(move || {
                let mut failures = 0_u32;
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        // Idle: check the stop flag again shortly.
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(ACCEPT_POLL);
                        }
                        Ok((stream, _)) => {
                            failures = 0;
                            if stop.load(Ordering::Acquire) {
                                return;
                            }
                            // An accepted socket may inherit the listener's
                            // non-blocking mode; its reads and writes use
                            // timeouts instead.
                            if stream.set_nonblocking(false).is_err() {
                                connection_dropped();
                                continue;
                            }
                            join_finished(&accept_connections, &accept_panicked);
                            if let Some(connection) = accept(stream, &deliver, &active, config) {
                                track(&accept_connections, connection);
                            }
                        }
                        Err(error) => {
                            let failure = classify_accept_failure(&error);
                            let span = scxml_telemetry::observe(
                                Module::HostWasmtime,
                                Operation::HostWasmtimeBasicHttpListenerAccept,
                            );
                            span.event(SignalEvent::InputRejected);
                            span.fail(match failure {
                                AcceptFailure::Connection => ErrorCategory::InputRead,
                                AcceptFailure::Listener => ErrorCategory::Platform,
                            });
                            drop(span);
                            if failure == AcceptFailure::Listener {
                                pause(accept_backoff(failures), &stop);
                                failures = failures.saturating_add(1);
                            }
                        }
                    }
                }
            })?;
        Ok(ListenerHandle {
            stop: handle_stop,
            local_addr,
            accept: Mutex::new(Some(accept_thread)),
            connections,
            panicked,
        })
    }
}

/// How the accept loop treats one failed `accept`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcceptFailure {
    /// The connection failed before it was accepted (aborted, reset,
    /// interrupted, timed out): the next one is accepted at once.
    Connection,
    /// The listener could not take a connection (no file descriptor or
    /// memory left, or another failure): it waits before accepting again.
    Listener,
}

fn classify_accept_failure(error: &io::Error) -> AcceptFailure {
    match error.kind() {
        io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::Interrupted
        | io::ErrorKind::WouldBlock
        | io::ErrorKind::TimedOut => AcceptFailure::Connection,
        _ => AcceptFailure::Listener,
    }
}

/// The wait after `consecutive` listener failures in a row before the next
/// `accept`: 10 ms, doubling, at most 1 s.
fn accept_backoff(consecutive: u32) -> Duration {
    Duration::from_millis(10_u64.saturating_mul(1_u64 << consecutive.min(7)))
        .min(Duration::from_secs(1))
}

/// Waits `total`, in [`ACCEPT_POLL`] slices, unless `stop` is set first:
/// a backing-off listener still stops within one slice.
fn pause(total: Duration, stop: &AtomicBool) {
    pause_with(total, stop, std::thread::sleep);
}

/// [`pause`], sleeping each slice with `sleep`.
fn pause_with(total: Duration, stop: &AtomicBool, mut sleep: impl FnMut(Duration)) {
    let Some(end) = Instant::now().checked_add(total) else {
        return;
    };
    while !stop.load(Ordering::Acquire) {
        let Some(left) = end
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        else {
            return;
        };
        sleep(left.min(ACCEPT_POLL));
    }
}

/// Keeps the thread of one accepted connection, so stopping the listener
/// waits for it. A poisoned list cannot hold it: the connection is still
/// served on its thread, which runs on detached, and that is recorded
/// ([`connection_untracked`]).
fn track(connections: &Mutex<Vec<JoinHandle<()>>>, connection: JoinHandle<()>) {
    match connections.lock() {
        Ok(mut connections) => connections.push(connection),
        Err(_) => connection_untracked(),
    }
}

/// Records a connection served on a thread the listener could not keep (a
/// poisoned thread list): a `platform` failure of a connection span, with
/// no `input-rejected` event, because the connection is answered; stopping
/// the listener does not wait for its thread.
fn connection_untracked() {
    let span = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeBasicHttpListenerConnection,
    );
    span.fail(ErrorCategory::Platform);
}

/// Records a connection the listener dropped after accepting it (a socket
/// mode or timeout it could not set, a thread it could not start) as a
/// connection span: the client sees the connection close unanswered.
fn connection_dropped() {
    let span = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeBasicHttpListenerConnection,
    );
    span.event(SignalEvent::InputRejected);
    span.fail(ErrorCategory::Platform);
}

/// One connection slot, released on drop.
struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Takes one accepted connection: its thread, when one started.
fn accept(
    mut stream: TcpStream,
    deliver: &Arc<dyn Fn(InboundHttp) + Send + Sync>,
    active: &Arc<AtomicUsize>,
    config: ListenerConfig,
) -> Option<JoinHandle<()>> {
    // Without its timeouts a connection could hold its thread forever: it
    // is closed unanswered instead.
    if stream
        .set_read_timeout(Some(config.read_timeout))
        .and_then(|()| stream.set_write_timeout(Some(config.write_timeout)))
        .is_err()
    {
        connection_dropped();
        return None;
    }
    let reserved = active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        (count < config.max_connections).then_some(count + 1)
    });
    if reserved.is_err() {
        let _ = write_response(&mut stream, &unavailable());
        return None;
    }
    let slot = Active(Arc::clone(active));
    let deliver = Arc::clone(deliver);
    // A connection whose thread could not start is dropped (closed).
    let spawned = std::thread::Builder::new()
        .name("scxml-basichttp-connection".into())
        .spawn(move || {
            let _slot = slot;
            serve_connection(stream, &*deliver, config);
        });
    if spawned.is_err() {
        connection_dropped();
    }
    spawned.ok()
}

fn unavailable() -> HttpResponse {
    HttpResponse {
        status: 503,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn serve_connection(
    mut stream: TcpStream,
    deliver: &(dyn Fn(InboundHttp) + Send + Sync),
    config: ListenerConfig,
) {
    let span = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeBasicHttpListenerRequest,
    );
    let Ok(request) = read_request(&mut stream, config.read_timeout) else {
        // The client went away or stalled: nothing to answer.
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::InputRead);
        return;
    };
    let framed = request.is_ok();
    let (reply, answer) = mpsc::sync_channel(1);
    let claim = Arc::new(AtomicU8::new(PENDING));
    deliver(InboundHttp {
        request,
        reply,
        claim: Arc::clone(&claim),
    });
    let response = match answer.recv_timeout(config.response_timeout) {
        Ok(response) => response,
        Err(mpsc::RecvTimeoutError::Disconnected) => unavailable(),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Not taken in time: answered 503, and never delivered after.
            // A message the session already claimed gets its answer.
            if claim
                .compare_exchange(PENDING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                unavailable()
            } else {
                answer.recv().unwrap_or_else(|_| unavailable())
            }
        }
    };
    let written = write_response(&mut stream, &response);
    let _ = stream.shutdown(Shutdown::Both);
    if written.is_ok() && framed {
        span.event(SignalEvent::TransformationCompleted);
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(if written.is_ok() {
            ErrorCategory::InvalidInput
        } else {
            ErrorCategory::Platform
        });
    }
}

/// Reads one request within `timeout`, head and body together: `Ok(Err(_))`
/// for a message the component must reject, `Err(_)` when the connection
/// failed or the deadline passed before the whole request arrived.
fn read_request(
    stream: &mut TcpStream,
    timeout: Duration,
) -> io::Result<Result<HttpRequest, Rejection>> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(io::ErrorKind::InvalidInput)?;
    // Each read waits at most until the deadline, so bytes that trickle in
    // cannot extend it.
    let read_some = |stream: &mut TcpStream, into: &mut [u8]| -> io::Result<usize> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(io::ErrorKind::TimedOut)?;
        stream.set_read_timeout(Some(remaining))?;
        stream.read(into)
    };
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let mut scanned = 0;
    let head_end = loop {
        if let Some(end) = find_head_end(&buffer, scanned) {
            break end;
        }
        // The terminator may straddle the next read: rescan its last 3 bytes.
        scanned = buffer.len().saturating_sub(3);
        if buffer.len() > MAX_REQUEST_HEAD_BYTES {
            return Ok(Err(Rejection::PayloadTooLarge));
        }
        let read = read_some(stream, &mut chunk)?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
    };
    if head_end > MAX_REQUEST_HEAD_BYTES {
        return Ok(Err(Rejection::PayloadTooLarge));
    }
    let head = match parse_head(buffer.get(..head_end).unwrap_or_default()) {
        Ok(head) => head,
        Err(rejection) => return Ok(Err(rejection)),
    };
    let mut body = buffer.get(head_end..).unwrap_or_default().to_vec();
    if head.expect_continue && body.len() < head.content_length {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    while body.len() < head.content_length {
        let wanted = (head.content_length - body.len()).min(chunk.len());
        let read = read_some(stream, chunk.get_mut(..wanted).unwrap_or_default())?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        body.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
    body.truncate(head.content_length);
    Ok(Ok(HttpRequest {
        method: head.method,
        uri: head.target,
        headers: head.headers,
        body,
    }))
}

/// The end of the request head (`\r\n\r\n` included) in `buffer`, looking
/// only from `from` on.
fn find_head_end(buffer: &[u8], from: usize) -> Option<usize> {
    buffer
        .get(from..)?
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| from + position + 4)
}

struct Head {
    method: String,
    target: String,
    headers: Vec<HttpHeader>,
    content_length: usize,
    expect_continue: bool,
}

fn parse_head(bytes: &[u8]) -> Result<Head, Rejection> {
    let mut fields = [httparse::EMPTY_HEADER; MAX_REQUEST_HEADERS];
    let mut request = httparse::Request::new(&mut fields);
    match request.parse(bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        Err(httparse::Error::TooManyHeaders) => return Err(Rejection::PayloadTooLarge),
        Ok(httparse::Status::Partial) | Err(_) => return Err(Rejection::InvalidMessage),
    }
    let (Some(method), Some(target)) = (request.method, request.path) else {
        return Err(Rejection::InvalidMessage);
    };
    let mut headers = Vec::with_capacity(request.headers.len());
    let mut content_length: Option<usize> = None;
    let mut expect_continue = false;
    for field in request.headers.iter() {
        let value = std::str::from_utf8(field.value).map_err(|_| Rejection::InvalidMessage)?;
        if field.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(Rejection::InvalidMessage);
        }
        if field.name.eq_ignore_ascii_case("content-length") {
            let length: usize = value
                .trim()
                .parse()
                .map_err(|_| Rejection::InvalidMessage)?;
            if content_length.is_some_and(|previous| previous != length) {
                return Err(Rejection::InvalidMessage);
            }
            content_length = Some(length);
        }
        if field.name.eq_ignore_ascii_case("expect") {
            if value.trim().eq_ignore_ascii_case("100-continue") {
                expect_continue = true;
            } else {
                return Err(Rejection::InvalidMessage);
            }
        }
        headers.push(HttpHeader {
            name: field.name.to_owned(),
            value: value.to_owned(),
        });
    }
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_REQUEST_BODY_BYTES {
        return Err(Rejection::PayloadTooLarge);
    }
    Ok(Head {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        content_length,
        expect_continue,
    })
}

const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "",
    }
}

fn write_response(stream: &mut TcpStream, response: &HttpResponse) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for header in &response.headers {
        // The component's headers are validated tokens and values; framing
        // headers are the listener's own.
        let framing = ["content-length", "connection", "transfer-encoding"]
            .iter()
            .any(|name| header.name.eq_ignore_ascii_case(name));
        if framing || header.name.contains(['\r', '\n', ':']) || header.value.contains(['\r', '\n'])
        {
            continue;
        }
        head.push_str(&header.name);
        head.push_str(": ");
        head.push_str(&header.value);
        head.push_str("\r\n");
    }
    let _ = write!(
        head,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        response.body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backing-off accept loop sleeps in slices of at most one poll and
    /// sees `stop` after each: stopped, it does not sleep; stopped during a
    /// slice, it sleeps no further; running, it sleeps until the pause ends.
    #[test]
    fn a_backoff_pause_ends_when_the_listener_stops() {
        let stopped = AtomicBool::new(true);
        let mut slices = Vec::new();
        pause_with(Duration::from_secs(1), &stopped, |slice| slices.push(slice));
        assert!(slices.is_empty(), "{slices:?}");

        let stop = AtomicBool::new(false);
        let mut slices = Vec::new();
        pause_with(Duration::from_secs(1), &stop, |slice| {
            slices.push(slice);
            stop.store(true, Ordering::Release);
        });
        assert_eq!(slices, [ACCEPT_POLL]);

        let running = AtomicBool::new(false);
        let mut slices = Vec::new();
        pause_with(Duration::from_millis(30), &running, |slice| {
            slices.push(slice);
            std::thread::sleep(slice);
        });
        assert!(!slices.is_empty());
        assert!(
            slices.iter().all(|slice| *slice <= ACCEPT_POLL),
            "{slices:?}"
        );
    }

    /// A connection whose thread the poisoned connection list cannot hold
    /// is still served: it records a `platform` failure of the accept span,
    /// not the `input-rejected` of a connection the listener dropped.
    #[test]
    fn a_connection_the_poisoned_list_cannot_hold_records_platform_only() {
        let connections = Arc::new(Mutex::new(Vec::new()));
        let poisoner = Arc::clone(&connections);
        let panicked = std::thread::spawn(move || {
            let _held = poisoner.lock().expect("the list locks");
            panic!("poison the connection list");
        })
        .join();
        assert!(panicked.is_err() && connections.is_poisoned());
        let recorder = scxml_telemetry::Recorder::default();
        let _guard = scxml_telemetry::install_recorder(recorder.clone());
        track(&connections, std::thread::spawn(|| {}));
        let observations = recorder.observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].operation,
            Operation::HostWasmtimeBasicHttpListenerConnection
        );
        assert_eq!(observations[0].error, Some(ErrorCategory::Platform));
        assert!(
            observations[0].events.is_empty(),
            "{:?}",
            observations[0].events
        );
    }

    /// A connection dropped after `accept` is recorded, never silent.
    #[test]
    fn a_dropped_connection_records_an_input_rejected_platform_span() {
        let recorder = scxml_telemetry::Recorder::default();
        let _guard = scxml_telemetry::install_recorder(recorder.clone());
        connection_dropped();
        let observations = recorder.observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].operation,
            Operation::HostWasmtimeBasicHttpListenerConnection
        );
        assert_eq!(observations[0].error, Some(ErrorCategory::Platform));
        assert_eq!(observations[0].events, [SignalEvent::InputRejected]);
    }

    #[test]
    fn accept_failures_are_classified_and_listener_failures_back_off() {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::Interrupted,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::TimedOut,
        ] {
            assert_eq!(
                classify_accept_failure(&kind.into()),
                AcceptFailure::Connection
            );
        }
        // EMFILE / ENFILE surface as uncategorized OS errors.
        for error in [
            io::Error::from_raw_os_error(24),
            io::Error::from(io::ErrorKind::OutOfMemory),
            io::Error::other("listener"),
        ] {
            assert_eq!(classify_accept_failure(&error), AcceptFailure::Listener);
        }
        let waits: Vec<u128> = (0..10).map(|n| accept_backoff(n).as_millis()).collect();
        assert_eq!(waits, [10, 20, 40, 80, 160, 320, 640, 1000, 1000, 1000]);
        assert_eq!(accept_backoff(u32::MAX), Duration::from_secs(1));
    }

    #[test]
    fn the_head_end_is_found_from_where_the_last_scan_stopped() {
        let head = b"GET / HTTP/1.1\r\nA: b\r\n\r\nbody";
        assert_eq!(find_head_end(head, 0), Some(24));
        // The terminator straddling the previous read is still found.
        assert_eq!(find_head_end(head, 20), Some(24));
        assert_eq!(find_head_end(head, 21), None);
        assert_eq!(find_head_end(head, head.len() + 1), None);
    }

    #[test]
    fn bind_rejects_zero_timeouts() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("an address");
        for config in [
            ListenerConfig {
                read_timeout: Duration::ZERO,
                ..ListenerConfig::default()
            },
            ListenerConfig {
                write_timeout: Duration::ZERO,
                ..ListenerConfig::default()
            },
            ListenerConfig {
                response_timeout: Duration::ZERO,
                ..ListenerConfig::default()
            },
        ] {
            let error = BasicHttpListener::bind(addr, config).expect_err("zero timeout");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

    /// A message the session did not claim within the response timeout is
    /// answered `503` and can no longer be claimed; one claimed in time gets
    /// the session's answer, however late.
    #[test]
    fn an_unclaimed_message_is_answered_503_and_never_delivered_after() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("an address");
        let config = ListenerConfig {
            response_timeout: Duration::from_millis(100),
            ..ListenerConfig::default()
        };
        let (held, waiting) = mpsc::channel();
        let handle = BasicHttpListener::bind(addr, config)
            .expect("bind")
            .serve(move |inbound| {
                let _ = held.send(inbound);
            })
            .expect("serve");
        let request = |client: &mut TcpStream| {
            client
                .write_all(b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
                .expect("request");
        };
        let status = |client: &mut TcpStream| {
            let mut answer = String::new();
            client.read_to_string(&mut answer).expect("answer");
            answer.split(' ').nth(1).unwrap_or_default().to_owned()
        };
        // Taken too late: 503, and the claim fails.
        let mut late = TcpStream::connect(handle.local_addr()).expect("connect");
        request(&mut late);
        assert_eq!(status(&mut late), "503");
        let inbound: InboundHttp = waiting.recv().expect("delivered");
        assert!(
            inbound.claim().is_none(),
            "a message answered 503 must not be delivered"
        );
        // Claimed in time: the session's answer arrives after the timeout.
        let mut claimed = TcpStream::connect(handle.local_addr()).expect("connect");
        request(&mut claimed);
        let inbound: InboundHttp = waiting.recv().expect("delivered");
        let inbound = inbound.claim().expect("claimed in time");
        std::thread::sleep(Duration::from_millis(250));
        inbound.respond(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: Vec::new(),
        });
        assert_eq!(status(&mut claimed), "200");
        handle.stop();
    }

    /// A connection thread that panicked is reported through the handle;
    /// a listener whose threads ended normally reports none.
    #[test]
    fn a_panicked_thread_is_reported_through_the_handle() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("an address");
        let calm = BasicHttpListener::bind(addr, ListenerConfig::default())
            .expect("bind")
            .serve(drop)
            .expect("serve");
        let mut client = TcpStream::connect(calm.local_addr()).expect("connect");
        client
            .write_all(b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
            .expect("request");
        let mut answer = String::new();
        client.read_to_string(&mut answer).expect("answer");
        calm.stop();
        assert!(!calm.panicked());

        let failing = BasicHttpListener::bind(addr, ListenerConfig::default())
            .expect("bind")
            .serve(|_| panic!("the deliver callback failed"))
            .expect("serve");
        let mut client = TcpStream::connect(failing.local_addr()).expect("connect");
        client
            .write_all(b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
            .expect("request");
        let mut answer = String::new();
        // The panicking thread drops the connection unanswered.
        let _ = client.read_to_string(&mut answer);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !failing.panicked() {
            assert!(Instant::now() < deadline, "the panic is reported");
            std::thread::sleep(Duration::from_millis(10));
        }
        failing.stop();
    }

    /// A client that drips one byte at a time cannot hold a connection past
    /// the request deadline: the listener closes it and delivers nothing.
    #[test]
    fn a_drip_fed_request_ends_at_the_request_deadline() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("an address");
        let listener = BasicHttpListener::bind(
            addr,
            ListenerConfig {
                read_timeout: Duration::from_millis(300),
                ..ListenerConfig::default()
            },
        )
        .expect("bind");
        let delivered = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&delivered);
        let handle = listener
            .serve(move |inbound| {
                counted.fetch_add(1, Ordering::AcqRel);
                drop(inbound);
            })
            .expect("serve");
        let mut client = TcpStream::connect(handle.local_addr()).expect("connect");
        client
            .write_all(b"POST / HTTP/1.1\r\nX: ")
            .expect("request line");
        let started = Instant::now();
        let mut closed = false;
        while started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(30));
            if client.write_all(b"a").is_err() {
                closed = true;
                break;
            }
        }
        let elapsed = started.elapsed();
        handle.stop();
        assert!(closed, "the listener kept a drip-fed connection open");
        assert!(elapsed < Duration::from_millis(1_200), "{elapsed:?}");
        assert_eq!(delivered.load(Ordering::Acquire), 0);
    }

    #[test]
    fn heads_are_framed_by_content_length_only() {
        let head = parse_head(
            b"POST /sessions/root/basichttp HTTP/1.1\r\nHost: x\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 12\r\n\r\n",
        )
        .map_err(|_| ())
        .expect("valid head");
        assert_eq!(head.method, "POST");
        assert_eq!(head.target, "/sessions/root/basichttp");
        assert_eq!(head.content_length, 12);
        assert_eq!(head.headers.len(), 3);
        for (bytes, rejection) in [
            (
                &b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"[..],
                Rejection::InvalidMessage,
            ),
            (
                &b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n"[..],
                Rejection::InvalidMessage,
            ),
            (
                &b"POST / HTTP/1.1\r\nContent-Length: x\r\n\r\n"[..],
                Rejection::InvalidMessage,
            ),
            (
                &b"POST / HTTP/1.1\r\nContent-Length: 65537\r\n\r\n"[..],
                Rejection::PayloadTooLarge,
            ),
            (
                &b"POST / HTTP/1.1\r\nExpect: something\r\n\r\n"[..],
                Rejection::InvalidMessage,
            ),
            (&b"garbage\r\n\r\n"[..], Rejection::InvalidMessage),
        ] {
            assert_eq!(
                parse_head(bytes).err(),
                Some(rejection),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
        let mut many = b"POST / HTTP/1.1\r\n".to_vec();
        for index in 0..=MAX_REQUEST_HEADERS {
            many.extend_from_slice(format!("X-{index}: v\r\n").as_bytes());
        }
        many.extend_from_slice(b"\r\n");
        assert_eq!(parse_head(&many).err(), Some(Rejection::PayloadTooLarge));
    }
}
