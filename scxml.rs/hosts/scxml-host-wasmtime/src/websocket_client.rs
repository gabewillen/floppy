//! The host's WebSocket client: a [`crate::WebSocketTransport`] for the
//! connections processor instances open (`outbound-websocket`).
//!
//! * no ambient authority: no cookies, no credentials, and no proxy from
//!   the environment; an embedder that must tunnel names its HTTP proxy
//!   explicitly ([`WebSocketClientConfig::proxy`]), which is reached with
//!   `CONNECT`;
//! * `wss` is rustls (ring) verified against the operating system's trust
//!   store; `ws` is plain TCP;
//! * the handshake is bounded by the connect timeout; once open, a message
//!   either way is at most [`crate::MAX_WEBSOCKET_MESSAGE_BYTES`];
//! * each connection runs on the calling thread, polling its socket in
//!   short reads so the messages the instance hands it go out promptly.
//!
//! The host admits every request before it reaches the transport (the
//! processor package's origins), so this client checks only the URL's form.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::WebSocketConfig;

use crate::{
    MAX_WEBSOCKET_MESSAGE_BYTES, WebSocketAnswer, WebSocketClose, WebSocketFailure,
    WebSocketMessage, WebSocketOutgoing, WebSocketRequest, WebSocketTransport,
};

/// Default bound of a connection's TCP connect, proxy tunnel, TLS, and
/// WebSocket handshake together.
pub const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one read waits before the connection sends what the instance
/// handed it.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Maximum bytes of a proxy's `CONNECT` response head.
const MAX_PROXY_HEAD_BYTES: usize = 16 * 1_024;

/// How [`WebSocketClient`] connects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketClientConfig {
    /// Bound of the TCP connect, proxy tunnel, TLS, and WebSocket handshake
    /// together.
    pub connect_timeout: Duration,
    /// An HTTP proxy (`http://host:port`) every connection tunnels through
    /// with `CONNECT`; `None` connects directly. Never read from the
    /// environment.
    pub proxy: Option<String>,
}

impl Default for WebSocketClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_WEBSOCKET_CONNECT_TIMEOUT,
            proxy: None,
        }
    }
}

/// Why a [`WebSocketClient`] could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSocketClientError {
    /// A zero connect timeout.
    TimeoutInvalid,
    /// A proxy that is not `http://host:port`.
    ProxyInvalid,
    /// The TLS configuration could not be built (no usable trust store).
    Tls,
}

impl std::fmt::Display for WebSocketClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TimeoutInvalid => "the WebSocket connect timeout must be greater than zero",
            Self::ProxyInvalid => "the WebSocket proxy must be http://host:port",
            Self::Tls => "the WebSocket client's TLS configuration could not be built",
        })
    }
}

impl std::error::Error for WebSocketClientError {}

/// The host's WebSocket client (see the module documentation).
#[derive(Clone)]
pub struct WebSocketClient {
    connect_timeout: Duration,
    proxy: Option<(String, u16)>,
    tls: Arc<rustls::ClientConfig>,
}

impl std::fmt::Debug for WebSocketClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebSocketClient")
            .field("connect_timeout", &self.connect_timeout)
            .field("proxy", &self.proxy.is_some())
            .finish_non_exhaustive()
    }
}

/// The `host:port` of an `http://host:port` proxy URL.
fn proxy_address(proxy: &str) -> Option<(String, u16)> {
    let url = url::Url::parse(proxy).ok()?;
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

impl WebSocketClient {
    /// A client connecting as `config` says.
    ///
    /// # Errors
    ///
    /// [`WebSocketClientError`].
    pub fn new(config: &WebSocketClientConfig) -> Result<Self, WebSocketClientError> {
        use rustls_platform_verifier::BuilderVerifierExt as _;
        if config.connect_timeout.is_zero() {
            return Err(WebSocketClientError::TimeoutInvalid);
        }
        let proxy = match &config.proxy {
            Some(proxy) => Some(proxy_address(proxy).ok_or(WebSocketClientError::ProxyInvalid)?),
            None => None,
        };
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| WebSocketClientError::Tls)?
        .with_platform_verifier()
        .map_err(|_| WebSocketClientError::Tls)?
        .with_no_client_auth();
        Ok(Self {
            connect_timeout: config.connect_timeout,
            proxy,
            tls: Arc::new(tls),
        })
    }
}

/// A connection's byte stream: plain TCP, or TLS over it.
enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(stream) => stream,
            Self::Tls(stream) => &stream.sock,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            Self::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buf),
            Self::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

type Socket = tungstenite::WebSocket<Stream>;

/// The time left of `deadline`, at least 1 ms (a zero timeout is refused
/// by the socket).
fn left(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

/// Opens a TCP connection to `host:port` before `deadline`.
fn tcp(host: &str, port: u16, deadline: Instant) -> Option<TcpStream> {
    let addresses = (host, port).to_socket_addrs().ok()?;
    for address in addresses {
        let timeout = left(deadline)?;
        if let Ok(stream) = TcpStream::connect_timeout(&address, timeout) {
            let _ = stream.set_nodelay(true);
            return Some(stream);
        }
    }
    None
}

/// Tunnels `stream` (connected to the proxy) to `host:port` with
/// `CONNECT`; `true` once the proxy answered 2xx.
fn tunnel(stream: &mut TcpStream, host: &str, port: u16) -> bool {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let head = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
    if stream.write_all(head.as_bytes()).is_err() {
        return false;
    }
    let Ok(clone) = stream.try_clone() else {
        return false;
    };
    let mut reader = BufReader::new(clone.take(MAX_PROXY_HEAD_BYTES as u64));
    let mut status = String::new();
    if reader.read_line(&mut status).is_err() {
        return false;
    }
    let ok = status
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code.len() == 3 && code.starts_with('2'));
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => {}
        }
    }
    // A proxy that sent bytes past its head before the tunnel opened would
    // have them lost here; a conforming proxy sends none.
    ok && reader.buffer().is_empty()
}

impl WebSocketClient {
    /// Opens `request`: TCP (through the proxy, when one is named), TLS for
    /// `wss`, and the WebSocket handshake, all before the connect timeout.
    fn open(
        &self,
        request: &WebSocketRequest,
    ) -> Result<(Socket, Option<String>), WebSocketFailure> {
        let url = url::Url::parse(&request.url).map_err(|_| WebSocketFailure::NotAllowed)?;
        let secure = match url.scheme() {
            "wss" => true,
            "ws" => false,
            _ => return Err(WebSocketFailure::NotAllowed),
        };
        let host = url
            .host_str()
            .ok_or(WebSocketFailure::NotAllowed)?
            .to_owned();
        let host = host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .map_or(host.clone(), str::to_owned);
        let port = url
            .port_or_known_default()
            .ok_or(WebSocketFailure::NotAllowed)?;
        let deadline = Instant::now() + self.connect_timeout;
        let mut tcp_stream = match &self.proxy {
            Some((proxy_host, proxy_port)) => {
                let mut stream =
                    tcp(proxy_host, *proxy_port, deadline).ok_or(WebSocketFailure::Handshake)?;
                stream
                    .set_read_timeout(left(deadline))
                    .map_err(|_| WebSocketFailure::Handshake)?;
                if !tunnel(&mut stream, &host, port) {
                    return Err(WebSocketFailure::Handshake);
                }
                stream
            }
            None => tcp(&host, port, deadline).ok_or(WebSocketFailure::Handshake)?,
        };
        let timeout = left(deadline).ok_or(WebSocketFailure::Handshake)?;
        tcp_stream
            .set_read_timeout(Some(timeout))
            .and_then(|()| tcp_stream.set_write_timeout(Some(timeout)))
            .map_err(|_| WebSocketFailure::Handshake)?;
        let stream = if secure {
            let name = rustls::pki_types::ServerName::try_from(host.clone())
                .map_err(|_| WebSocketFailure::NotAllowed)?;
            let connection = rustls::ClientConnection::new(Arc::clone(&self.tls), name)
                .map_err(|_| WebSocketFailure::Handshake)?;
            let mut tls = rustls::StreamOwned::new(connection, tcp_stream);
            while tls.conn.is_handshaking() {
                tls.conn
                    .complete_io(&mut tls.sock)
                    .map_err(|_| WebSocketFailure::Handshake)?;
            }
            Stream::Tls(Box::new(tls))
        } else {
            let _ = &mut tcp_stream;
            Stream::Plain(tcp_stream)
        };
        let mut handshake = request
            .url
            .as_str()
            .into_client_request()
            .map_err(|_| WebSocketFailure::NotAllowed)?;
        for (name, value) in &request.headers {
            let name = tungstenite::http::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| WebSocketFailure::NotAllowed)?;
            let value = tungstenite::http::HeaderValue::from_str(value)
                .map_err(|_| WebSocketFailure::NotAllowed)?;
            handshake.headers_mut().append(name, value);
        }
        if !request.protocols.is_empty() {
            let value = tungstenite::http::HeaderValue::from_str(&request.protocols.join(", "))
                .map_err(|_| WebSocketFailure::NotAllowed)?;
            handshake
                .headers_mut()
                .insert("sec-websocket-protocol", value);
        }
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES));
        let (socket, response) =
            tungstenite::client::client_with_config(handshake, stream, Some(config))
                .map_err(|_| WebSocketFailure::Handshake)?;
        let protocol = response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if protocol
            .as_ref()
            .is_some_and(|protocol| !request.protocols.contains(protocol))
        {
            return Err(WebSocketFailure::Handshake);
        }
        Ok((socket, protocol))
    }
}

/// Whether `error` is a read that only timed out.
fn timed_out(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Io(io)
            if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
    )
}

/// Runs one open connection until it ends: sends what `outgoing` yields,
/// hands each message the peer sends to `answer`, and ends with the close.
fn run(
    socket: &mut Socket,
    outgoing: &mpsc::Receiver<WebSocketOutgoing>,
    answer: &mut dyn FnMut(WebSocketAnswer),
) {
    if socket
        .get_mut()
        .tcp()
        .set_read_timeout(Some(POLL_INTERVAL))
        .is_err()
    {
        answer(WebSocketAnswer::Failed(WebSocketFailure::Transport));
        return;
    }
    let mut closing = false;
    let mut peer_close: Option<WebSocketClose> = None;
    loop {
        if !closing {
            loop {
                let out = match outgoing.try_recv() {
                    Ok(WebSocketOutgoing::Message(message)) => match message {
                        WebSocketMessage::Text(text) => tungstenite::Message::text(text),
                        WebSocketMessage::Binary(bytes) => tungstenite::Message::binary(bytes),
                    },
                    Ok(WebSocketOutgoing::Close) | Err(mpsc::TryRecvError::Disconnected) => {
                        closing = true;
                        if socket.close(None).is_err() {
                            answer(WebSocketAnswer::Closed(None));
                            return;
                        }
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                };
                if socket.send(out).is_err() {
                    answer(WebSocketAnswer::Failed(WebSocketFailure::Transport));
                    return;
                }
            }
        }
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                if !closing {
                    answer(WebSocketAnswer::Message(WebSocketMessage::Text(
                        text.as_str().to_owned(),
                    )));
                }
            }
            Ok(tungstenite::Message::Binary(bytes)) => {
                if !closing {
                    answer(WebSocketAnswer::Message(WebSocketMessage::Binary(
                        bytes.to_vec(),
                    )));
                }
            }
            Ok(tungstenite::Message::Close(frame)) => {
                peer_close = Some(frame.map_or(
                    WebSocketClose {
                        code: 1005,
                        reason: String::new(),
                    },
                    |frame| WebSocketClose {
                        code: u16::from(frame.code),
                        reason: frame.reason.as_str().to_owned(),
                    },
                ));
                closing = true;
            }
            Ok(_) => {}
            Err(error) if timed_out(&error) => {
                // Answers a ping or completes a close queued meanwhile.
                if let Err(error) = socket.flush()
                    && !timed_out(&error)
                {
                    finish(&error, closing, peer_close, answer);
                    return;
                }
            }
            Err(error) => {
                finish(&error, closing, peer_close, answer);
                return;
            }
        }
    }
}

/// The final answer of a connection that ended with `error`.
fn finish(
    error: &tungstenite::Error,
    closing: bool,
    peer_close: Option<WebSocketClose>,
    answer: &mut dyn FnMut(WebSocketAnswer),
) {
    match error {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed
            if closing || peer_close.is_some() =>
        {
            answer(WebSocketAnswer::Closed(peer_close));
        }
        tungstenite::Error::Capacity(_) => {
            answer(WebSocketAnswer::Failed(WebSocketFailure::MessageTooLarge));
        }
        _ if peer_close.is_some() => answer(WebSocketAnswer::Closed(peer_close)),
        _ => answer(WebSocketAnswer::Failed(WebSocketFailure::Transport)),
    }
}

impl WebSocketTransport for WebSocketClient {
    fn connect(
        &self,
        request: &WebSocketRequest,
        outgoing: mpsc::Receiver<WebSocketOutgoing>,
        answer: &mut dyn FnMut(WebSocketAnswer),
    ) {
        match self.open(request) {
            Ok((mut socket, protocol)) => {
                answer(WebSocketAnswer::Opened(protocol));
                run(&mut socket, &outgoing, answer);
            }
            Err(failure) => answer(WebSocketAnswer::Failed(failure)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_is_an_http_host_and_port() {
        assert_eq!(
            proxy_address("http://127.0.0.1:3128"),
            Some(("127.0.0.1".into(), 3128))
        );
        assert_eq!(proxy_address("http://proxy"), Some(("proxy".into(), 80)));
        for bad in [
            "https://proxy:3128",
            "http://user:pass@proxy:3128",
            "http://proxy:3128/path",
            "proxy:3128",
        ] {
            assert_eq!(proxy_address(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_config_is_checked() {
        assert_eq!(
            WebSocketClient::new(&WebSocketClientConfig {
                connect_timeout: Duration::ZERO,
                proxy: None,
            })
            .map(|_| ()),
            Err(WebSocketClientError::TimeoutInvalid)
        );
        assert_eq!(
            WebSocketClient::new(&WebSocketClientConfig {
                connect_timeout: Duration::from_secs(1),
                proxy: Some("socks5://proxy:1080".into()),
            })
            .map(|_| ()),
            Err(WebSocketClientError::ProxyInvalid)
        );
    }
}
