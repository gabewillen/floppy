//! Phase 7 spike "Spike `PGlite` In A Component"
//! (`docs/plans/phase7-pglite-defaults.mdscript.md`): the smallest invoked
//! service that runs SQL on `PGlite` from inside a `wasm32-wasip2` component.
//!
//! `PGlite` serves the Postgres wire protocol from a process beside `scx`
//! (`pglite-server` from `@electric-sql/pglite-socket`, or any Postgres);
//! this component connects to it with `wasi:sockets/tcp`, which the run
//! grants with `--wasi URI=inherit-network`, so `scx` needs no change.
//! The spike's measured answer, and why `PGlite` is not compiled into the
//! component, is recorded in the plan's Done So Far.
//!
//! # Parameters (`<param>`)
//!
//! * `address`: the server's IPv4 `IP:PORT` (a literal address: the component
//!   imports no name lookup grant it uses).
//! * `sql`: one simple-query string (Postgres `Query` message).
//!
//! # Events
//!
//! * `spike.pglite.result` `{"columns": [...], "rows": [[...]]}`: the last
//!   result set's column names and text values (`null` for SQL NULL).
//! * `spike.pglite.failed` `{"message": M}`: the server's error message, or
//!   the connection's failure.
//!
//! This is spike code: no bounds, no telemetry, no transaction contract.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::io::{Read, Write};

use serde_json::{Value as Json, json};

/// The spike service's type URI (test-only, never released).
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/pglite-spike";
/// The event carrying a query's rows.
pub const RESULT_EVENT: &str = "spike.pglite.result";
/// The event reporting a failed query or connection.
pub const FAILED_EVENT: &str = "spike.pglite.failed";

/// One returned event: its name and data.
#[derive(Debug, PartialEq)]
pub struct Returned {
    /// The event name.
    pub name: &'static str,
    /// The event data.
    pub data: Json,
}

/// Connects to `address`, runs `sql` as one simple query, and returns its
/// event.
#[must_use]
pub fn run(address: &str, sql: &str) -> Returned {
    match query(address, sql) {
        Ok(data) => Returned {
            name: RESULT_EVENT,
            data,
        },
        Err(message) => Returned {
            name: FAILED_EVENT,
            data: json!({ "message": message }),
        },
    }
}

/// The Postgres v3 wire exchange: startup (trust authentication), one
/// `Query`, `Terminate`.
fn query(address: &str, sql: &str) -> Result<Json, String> {
    let mut stream = connect(address)?;
    let mut startup = Vec::new();
    startup.extend_from_slice(&196_608_u32.to_be_bytes());
    for text in ["user", "postgres", "database", "postgres", ""] {
        startup.extend_from_slice(text.as_bytes());
        startup.push(0);
    }
    send(&mut stream, None, &startup)?;
    loop {
        let (kind, body) = receive(&mut stream)?;
        match kind {
            b'R' if body.get(..4) == Some(&[0, 0, 0, 0]) => {}
            b'R' => return Err("the server asks for a password".into()),
            b'E' => return Err(error_message(&body)),
            b'Z' => break,
            _ => {}
        }
    }
    let mut text = sql.as_bytes().to_vec();
    text.push(0);
    send(&mut stream, Some(b'Q'), &text)?;
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    let mut failure = None;
    loop {
        let (kind, body) = receive(&mut stream)?;
        match kind {
            b'T' => {
                columns = row_description(&body)?;
                rows.clear();
            }
            b'D' => rows.push(data_row(&body)?),
            b'E' => failure = Some(error_message(&body)),
            b'Z' => break,
            _ => {}
        }
    }
    let _ = send(&mut stream, Some(b'X'), &[]);
    match failure {
        Some(message) => Err(message),
        None => Ok(json!({ "columns": columns, "rows": rows })),
    }
}

fn send(stream: &mut impl Write, kind: Option<u8>, body: &[u8]) -> Result<(), String> {
    let length = u32::try_from(body.len() + 4).map_err(|_| "message too long".to_owned())?;
    let mut message = Vec::with_capacity(body.len() + 5);
    message.extend(kind);
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(body);
    stream
        .write_all(&message)
        .map_err(|error| error.to_string())
}

fn receive(stream: &mut impl Read) -> Result<(u8, Vec<u8>), String> {
    let mut header = [0_u8; 5];
    stream
        .read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    let [kind, length @ ..] = header;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| "message too long".to_owned())?
        .checked_sub(4)
        .ok_or_else(|| "malformed message length".to_owned())?;
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    Ok((kind, body))
}

/// A native connection (the unit tests and a host-side check).
#[cfg(not(target_arch = "wasm32"))]
fn connect(address: &str) -> Result<std::net::TcpStream, String> {
    std::net::TcpStream::connect(address).map_err(|error| error.to_string())
}

/// The component's connection: `wasi:sockets/tcp` directly (the standard
/// library's sockets go through wasi-libc, which also links the
/// filesystem and UDP).
#[cfg(target_arch = "wasm32")]
fn connect(address: &str) -> Result<socket::Tcp, String> {
    socket::Tcp::connect(address)
}

#[cfg(target_arch = "wasm32")]
mod socket {
    use std::io;
    use std::net::SocketAddr;

    use wasip2::io::streams::{InputStream, OutputStream, StreamError};
    use wasip2::sockets::instance_network::instance_network;
    use wasip2::sockets::network::{
        ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress,
    };
    use wasip2::sockets::tcp::TcpSocket;
    use wasip2::sockets::tcp_create_socket::create_tcp_socket;

    /// One connected TCP socket and its streams.
    pub struct Tcp {
        input: InputStream,
        output: OutputStream,
        _socket: TcpSocket,
    }

    impl Tcp {
        pub fn connect(address: &str) -> Result<Self, String> {
            let address: SocketAddr = address
                .parse()
                .map_err(|_| "address is not IP:PORT".to_owned())?;
            let SocketAddr::V4(v4) = address else {
                return Err("address is not IPv4".to_owned());
            };
            let [a, b, c, d] = v4.ip().octets();
            let remote = IpSocketAddress::Ipv4(Ipv4SocketAddress {
                port: v4.port(),
                address: (a, b, c, d),
            });
            let socket =
                create_tcp_socket(IpAddressFamily::Ipv4).map_err(|code| format!("{code:?}"))?;
            let network = instance_network();
            socket
                .start_connect(&network, remote)
                .map_err(|code| format!("{code:?}"))?;
            let ready = socket.subscribe();
            let (input, output) = loop {
                match socket.finish_connect() {
                    Ok(streams) => break streams,
                    Err(ErrorCode::WouldBlock) => ready.block(),
                    Err(code) => return Err(format!("{code:?}")),
                }
            };
            drop(ready);
            Ok(Self {
                input,
                output,
                _socket: socket,
            })
        }
    }

    fn failed(error: &StreamError) -> io::Error {
        io::Error::other(match error {
            StreamError::Closed => "the connection closed".to_owned(),
            StreamError::LastOperationFailed(error) => error.to_debug_string(),
        })
    }

    impl io::Read for Tcp {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let wanted = u64::try_from(buf.len()).unwrap_or(u64::MAX);
            match self.input.blocking_read(wanted) {
                Ok(bytes) => {
                    buf[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Err(StreamError::Closed) => Ok(0),
                Err(error) => Err(failed(&error)),
            }
        }
    }

    impl io::Write for Tcp {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let chunk = &buf[..buf.len().min(4096)];
            self.output
                .blocking_write_and_flush(chunk)
                .map_err(|error| failed(&error))?;
            Ok(chunk.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.output.blocking_flush().map_err(|error| failed(&error))
        }
    }
}

/// A cursor over one message body.
struct Body<'a>(&'a [u8]);

impl Body<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], String> {
        if self.0.len() < count {
            return Err("truncated message".into());
        }
        let (head, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(head)
    }

    fn int16(&mut self) -> Result<i16, String> {
        let bytes = self.take(2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn int32(&mut self) -> Result<i32, String> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn cstring(&mut self) -> Result<String, String> {
        let end = self
            .0
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| "unterminated string".to_owned())?;
        let text = String::from_utf8_lossy(self.take(end)?).into_owned();
        self.take(1)?;
        Ok(text)
    }
}

fn row_description(body: &[u8]) -> Result<Vec<String>, String> {
    let mut body = Body(body);
    let count = body.int16()?;
    (0..count)
        .map(|_| {
            let name = body.cstring()?;
            body.take(18)?;
            Ok(name)
        })
        .collect()
}

fn data_row(body: &[u8]) -> Result<Vec<Json>, String> {
    let mut body = Body(body);
    let count = body.int16()?;
    (0..count)
        .map(|_| {
            let length = body.int32()?;
            Ok(match usize::try_from(length) {
                Ok(length) => {
                    Json::String(String::from_utf8_lossy(body.take(length)?).into_owned())
                }
                Err(_) => Json::Null,
            })
        })
        .collect()
}

/// The `M` (message) field of an `ErrorResponse`.
fn error_message(body: &[u8]) -> String {
    let mut body = Body(body);
    while let Some(&code) = body.0.first() {
        if code == 0 || body.take(1).is_err() {
            break;
        }
        match body.cstring() {
            Ok(text) if code == b'M' => return text,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    "the server reported an error".into()
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "invoked-service",
        path: "../../../scxml.rs/crates/scxml-wit/wit/scxml",
        generate_all,
    });

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, InvokeRequest,
    };

    type Done = Result<Option<DataValue>, ComponentError>;

    struct Component;

    fn error(detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code: ErrorCode::InvalidInput,
            detail: Some(detail.into()),
        }
    }

    fn plan(request: InvokeRequest) -> Result<(String, String), ComponentError> {
        if request.type_uri != super::SERVICE_URI {
            return Err(error("service type URI differs"));
        }
        let mut address = None;
        let mut sql = None;
        for entry in request.parameters {
            let DataValue::Text(text) = entry.value else {
                return Err(error("parameters are strings"));
            };
            match entry.name.as_str() {
                "address" => address = Some(text),
                "sql" => sql = Some(text),
                _ => return Err(error("unknown parameter")),
            }
        }
        Ok((
            address.ok_or_else(|| error("address is required"))?,
            sql.ok_or_else(|| error("sql is required"))?,
        ))
    }

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its task is spawned"
    )]
    impl Guest for Component {
        async fn invoke(
            request: InvokeRequest,
            _events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            let (address, sql) = plan(request)?;
            let (mut returned, returned_reader) = wit_stream::new::<Event>();
            let (done, done_reader) = wit_future::new::<Done>(|| Ok(None));
            wit_bindgen::spawn(async move {
                let reply = super::run(&address, &sql);
                let event = Event {
                    name: reply.name.to_owned(),
                    event_type: EventType::External,
                    send_id: None,
                    origin: None,
                    origin_type: None,
                    invoke_id: None,
                    data: DataValue::JsonObject(reply.data.to_string()),
                    raw: None,
                    trace_context: None,
                };
                if returned.write_one(event).await.is_some() {
                    return;
                }
                drop(returned);
                let _ = done.write(Ok(None)).await;
            });
            Ok((returned_reader, done_reader))
        }
    }

    export!(Component);
}
